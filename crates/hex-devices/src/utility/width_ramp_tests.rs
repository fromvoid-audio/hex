use super::*;
use hex_lattice::{NativeParameterAutomation, NativeProcessorRuntime, PreparedNativeProcessor};

struct NeutralBalance;
impl NativeParameterAutomation for NeutralBalance {
    fn parameter_ids(&self) -> &[ParameterId] {
        &[parameter::BALANCE]
    }
    fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
        (id == parameter::BALANCE).then_some(ParameterValue::Float(0.0))
    }
}

fn runtime(width: f32, rate: u32) -> Box<dyn NativeProcessorRuntime> {
    let base = UTILITY_DEFINITION.default_state().unwrap();
    let state = UTILITY_DEFINITION
        .patch_parameter(&base, parameter::WIDTH, ParameterValue::Float(width))
        .unwrap();
    UtilityDefinition::prepare_owned_automated_filter_for_parameters(
        &state,
        rate,
        &[parameter::BALANCE],
    )
    .unwrap()
    .create_runtime()
}
fn render(
    runtime: &mut dyn NativeProcessorRuntime,
    count: usize,
    block: usize,
) -> Vec<StereoFrame> {
    let mut out = vec![[0.25, -0.125]; count];
    for part in out.chunks_mut(block) {
        runtime
            .process_block_with_automation(
                part,
                None,
                &NeutralBalance,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
    }
    out
}

#[test]
fn manual_width_ramps_over_five_ms_without_changing_authored_balance() {
    for rate in [48_000, 192_000] {
        let total = rate as usize / 200;
        for block in [7, 64] {
            let mut old = runtime(1.0, rate);
            render(old.as_mut(), 16, block);
            let mut next = runtime(0.0, rate);
            next.copy_history_from(old.as_ref());
            let output = render(next.as_mut(), total + 1, block);
            for (i, actual) in output.iter().enumerate() {
                let width = 1.0 - ((i + 1).min(total) as f32 / total as f32);
                let expected = [0.0625 + 0.1875 * width, 0.0625 - 0.1875 * width];
                for channel in 0..2 {
                    assert!(
                        (actual[channel] - expected[channel]).abs() < 2.0e-6,
                        "rate={rate} block={block} frame={i}: {actual:?} vs {expected:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn same_target_handoff_keeps_progress_and_retarget_starts_at_current_width() {
    let mut old = runtime(1.0, 48_000);
    render(old.as_mut(), 1, 1);
    let mut first = runtime(0.0, 48_000);
    first.copy_history_from(old.as_ref());
    let initial = render(first.as_mut(), 64, 64);
    let mut same = runtime(0.0, 48_000);
    same.copy_history_from(first.as_ref());
    let continued = render(same.as_mut(), 1, 1)[0];
    assert!((continued[0] - initial[63][0]).abs() < 0.001);
    let mut reverse = runtime(2.0, 48_000);
    reverse.copy_history_from(same.as_ref());
    let next = render(reverse.as_mut(), 1, 1)[0];
    assert!(next[0] > continued[0] && next[0] - continued[0] < 0.005);
    reverse.reset();
    let reset = render(reverse.as_mut(), 1, 1)[0];
    assert!((reset[0] - 0.4375).abs() < 1.0e-6);
}

#[test]
fn repeated_width_retargets_remain_continuous_at_supported_rates() {
    for rate in [48_000, 192_000] {
        let mut current = runtime(1.0, rate);
        let mut last = render(current.as_mut(), 1, 1)[0];
        for width in [4.0, 0.0, 3.0, 1.0, 4.0, 0.0] {
            let mut next = runtime(width, rate);
            next.copy_history_from(current.as_ref());
            for frame in render(next.as_mut(), 64, 7) {
                for channel in 0..2 {
                    assert!(
                        (frame[channel] - last[channel]).abs() < 0.005,
                        "rate={rate} width={width} last={last:?} next={frame:?}"
                    );
                }
                last = frame;
            }
            current = next;
        }
        let settled = render(current.as_mut(), rate as usize / 200, 64);
        let last = settled.last().unwrap();
        assert!((last[0] - 0.0625).abs() < 1.0e-6 && (last[1] - 0.0625).abs() < 1.0e-6);
    }
}

/// Opt-in timing for repeated 64-frame retargets; no device deadline claim.
#[test]
#[ignore = "opt-in optimized width transition timing"]
fn measure_manual_width_retarget() {
    let mut a = runtime(0.0, 48_000);
    let mut b = runtime(2.0, 48_000);
    for phase in 0..4 {
        let count = if phase == 0 { 500 } else { 5_000 };
        let start = std::time::Instant::now();
        for index in 0..count {
            let (next, old) = if index % 2 == 0 {
                (&mut a, &b)
            } else {
                (&mut b, &a)
            };
            next.copy_history_from(old.as_ref());
            let mut block = [[0.25, -0.125]; 64];
            next.process_block_with_automation(
                &mut block,
                None,
                &NeutralBalance,
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            )
            .unwrap();
            std::hint::black_box(block);
        }
        if phase > 0 {
            eprintln!(
                "width_bench_ns_per_frame={}",
                start.elapsed().as_nanos() as f64 / (count * 64) as f64
            );
        }
    }
}
