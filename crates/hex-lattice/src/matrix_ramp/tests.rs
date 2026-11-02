use crate::{
    DeviceTypeId, NativeRuntimeChain, PreparedProcessorInstance, ProcessorChainOwner, StereoFrame,
    StereoMatrix,
};

const RATES: [u32; 4] = [8_000, 44_100, 48_000, 192_000];
const INPUT: StereoFrame = [1.0, 0.5];

fn chain(rate: u32, gain: f32, bypassed: bool) -> NativeRuntimeChain {
    NativeRuntimeChain::new(vec![PreparedProcessorInstance {
        meter: Default::default(),
        owner: ProcessorChainOwner::Track(1),
        device_id: 7,
        device_type: DeviceTypeId::from_static("test.matrix"),
        matrix: StereoMatrix::new([[gain, 0.0], [0.0, gain]]).unwrap(),
        bypassed,
        sample_rate: rate,
        configuration: None,
        definition: None,
    }])
    .unwrap()
}

fn render(chain: &mut NativeRuntimeChain, frames: usize, block: usize) -> Vec<StereoFrame> {
    let mut output = vec![INPUT; frames];
    for chunk in output.chunks_mut(block) {
        chain.begin_block();
        chain.process_block(chunk).unwrap();
        chain.commit_block();
    }
    output
}

/// Old gain 1.0 renders, then a direct edit to 0.25 hands off history.
fn edited(rate: u32) -> NativeRuntimeChain {
    let mut old = chain(rate, 1.0, false);
    render(&mut old, 16, 16);
    let mut candidate = chain(rate, 0.25, false);
    candidate.transfer_history_from(&mut old);
    candidate
}

#[test]
fn static_matrix_edit_ramps_for_five_ms_at_every_rate_and_block_size() {
    for rate in RATES {
        let ramp = rate.div_ceil(200) as usize;
        let total = ramp + 32;
        let reference = render(&mut edited(rate), total, 1);
        let mut previous = 1.0_f32;
        for (index, frame) in reference.iter().enumerate() {
            let step = (frame[0] - previous).abs();
            assert!(
                // Bound plus f32 rounding of the unit-scale output.
                step <= 0.75 / ramp as f32 + 4.0 * f32::EPSILON,
                "rate={rate}: step {step} at frame {index}"
            );
            assert_eq!(frame[1], frame[0] * 0.5, "rate={rate}: channel law");
            previous = frame[0];
        }
        let settled = reference
            .iter()
            .position(|frame| {
                *frame
                    == StereoMatrix::new([[0.25, 0.0], [0.0, 0.25]])
                        .unwrap()
                        .apply(INPUT)
            })
            .unwrap();
        // The target is reached exactly on the last ramp frame and held.
        assert_eq!(settled + 1, ramp, "rate={rate}");
        assert!((ramp as f64 - f64::from(rate) * 0.005).abs() <= 1.0);
        assert!(
            reference[settled..]
                .iter()
                .all(|frame| *frame == reference[settled])
        );
        for block in [7, 64, 128] {
            assert_eq!(
                render(&mut edited(rate), total, block),
                reference,
                "rate={rate} block={block}"
            );
        }
        // A twin handoff mid-ramp (same targets) continues the trajectory.
        let mut first = edited(rate);
        let mut joined = render(&mut first, ramp / 2, 3);
        let mut twin = chain(rate, 0.25, false);
        twin.transfer_history_from(&mut first);
        joined.extend(render(&mut twin, total - ramp / 2, 5));
        assert_eq!(joined, reference, "rate={rate}: twin continuation");
    }
}

#[test]
fn retarget_mid_ramp_starts_from_the_effective_matrix() {
    let rate = 48_000;
    let mut first = edited(rate);
    let before = render(&mut first, 120, 64);
    let mut retarget = chain(rate, 2.0, false);
    retarget.transfer_history_from(&mut first);
    let after = render(&mut retarget, 400, 64);
    let ramp = rate.div_ceil(200) as f32;
    let last = before[119][0];
    assert!((after[0][0] - last).abs() <= (2.0 - last) / ramp + 8.0 * f32::EPSILON);
    assert_eq!(after[239][0], 2.0);
}

#[test]
fn reset_bypass_excluded_and_rollback_apply_exact_laws() {
    let rate = 48_000;
    let target = StereoMatrix::new([[0.25, 0.0], [0.0, 0.25]])
        .unwrap()
        .apply(INPUT);

    let mut reset = edited(rate);
    reset.reset();
    assert!(
        render(&mut reset, 8, 8)
            .iter()
            .all(|frame| *frame == target)
    );

    let mut old = chain(rate, 1.0, false);
    let mut bypassed = chain(rate, 0.25, true);
    bypassed.transfer_history_from(&mut old);
    assert!(
        render(&mut bypassed, 8, 8)
            .iter()
            .all(|frame| *frame == INPUT)
    );

    let mut old = chain(rate, 1.0, false);
    let mut excluded = chain(rate, 0.25, false);
    excluded
        .transfer_history_excluding_old_range(&mut old, 0..1)
        .unwrap();
    assert!(
        render(&mut excluded, 8, 8)
            .iter()
            .all(|frame| *frame == target)
    );

    // A refused block leaves the committed ramp position unchanged.
    let mut rolled = edited(rate);
    let mut speculative = [INPUT; 32];
    rolled.begin_block();
    rolled.process_block(&mut speculative).unwrap();
    rolled.rollback_block();
    assert_eq!(
        render(&mut rolled, 64, 64),
        render(&mut edited(rate), 64, 64)
    );
}

/// Planners inherit a chain whose memoryless static matrix changed, so the
/// handoff ramp can run; identity, bypass and malformed ranges still refuse.
#[test]
fn parameter_retarget_gate_accepts_only_a_changed_unbypassed_static_matrix() {
    let old = chain(48_000, 1.0, false);
    assert!(chain(48_000, 0.25, false).can_retarget_parameters_from_range(&old, 0..1));
    assert!(chain(48_000, 1.0, false).can_retarget_parameters_from_range(&old, 0..1));
    assert!(!chain(48_000, 0.25, false).can_retarget_parameters_from_range(&old, 0..0));
    assert!(!chain(48_000, 0.25, false).can_retarget_parameters_from_range(&old, 0..2));
    assert!(!chain(44_100, 0.25, false).can_retarget_parameters_from_range(&old, 0..1));
    let bypassed = chain(48_000, 1.0, true);
    assert!(!chain(48_000, 0.25, true).can_retarget_parameters_from_range(&bypassed, 0..1));
    assert!(!chain(48_000, 0.25, false).can_retarget_parameters_from_range(&bypassed, 0..1));
    let mut other_device = chain(48_000, 0.25, false);
    other_device.entries[0].device_id = 8;
    assert!(!other_device.can_retarget_parameters_from_range(&old, 0..1));
    let mut other_type = chain(48_000, 0.25, false);
    other_type.entries[0].device_type = DeviceTypeId::from_static("test.other");
    assert!(!other_type.can_retarget_parameters_from_range(&old, 0..1));
}
