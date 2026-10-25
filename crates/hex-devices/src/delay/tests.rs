use super::*;
use hex_lattice::{DevicePreparationContext, EMPTY_DEVICE_RESOURCES, TuningTable};
fn factory(rate: u32, values: [f32; 3]) -> Arc<dyn PreparedOwnedProcessorRuntime> {
    let state = encode(Settings(values)).unwrap();
    let Some(PreparedProcessorFactory::Owned(factory)) = DEFINITION
        .prepare_runtime_factory(
            &state,
            &DevicePreparationContext {
                sample_rate: rate,
                tuning: &TuningTable::default(),
                resources: &EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
    else {
        panic!("owned delay")
    };
    factory
}
fn render(
    runtime: &mut dyn OwnedProcessorRuntime,
    input: &[StereoFrame],
    partitions: &[usize],
) -> Vec<StereoFrame> {
    let mut output = input.to_vec();
    let mut offset = 0;
    for count in partitions.iter().cycle() {
        if offset == output.len() {
            break;
        }
        let end = (offset + count).min(output.len());
        runtime.begin_block();
        runtime.process_block(&mut output[offset..end], &EMPTY_DEVICE_RESOURCES);
        runtime.commit_block();
        offset = end;
    }
    output
}
#[test]
fn exact_impulse_repeats_stereo_and_partition_oracles_at_supported_rates() {
    for rate in [8000, 44100, 48000, 96000, 192000] {
        let frames = rate as usize / 10;
        let factory = factory(rate, [100.0, 0.5, 1.0]);
        assert_eq!(factory.output_tail(), OutputTail::Unbounded);
        assert_eq!(factory.algorithmic_latency_frames(), Some(0));
        let mut input = vec![[0.0; 2]; frames * 5 + 1];
        input[0] = [1.0, -0.25];
        let mut a = factory.create_runtime().unwrap();
        let mut b = factory.create_runtime().unwrap();
        let output = render(a.as_mut(), &input, &[input.len()]);
        assert_eq!(output, render(b.as_mut(), &input, &[1, 63, 257, 19]));
        for (index, frame) in output.iter().enumerate() {
            let expected = if index > 0 && index % frames == 0 {
                let gain = 0.5_f32.powi((index / frames - 1) as i32);
                [gain, -0.25 * gain]
            } else {
                [0.0; 2]
            };
            assert_eq!(*frame, expected, "rate={rate} index={index}");
        }
        assert!(!a.committed_is_quiescent(), "dormant samples still matter");
        let silence = vec![[0.0; 2]; rate as usize * 8];
        render(a.as_mut(), &silence, &[257]);
        assert!(a.committed_is_quiescent());
    }
}
#[test]
fn fractional_tap_has_two_known_weights_and_no_channel_crosstalk() {
    let mut runtime = factory(8000, [100.0625, 0.0, 1.0])
        .create_runtime()
        .unwrap();
    let mut input = vec![[0.0; 2]; 900];
    input[0] = [1.0, 0.0];
    let output = render(runtime.as_mut(), &input, &[17]);
    assert_eq!(output[800], [0.5, 0.0]);
    assert_eq!(output[801], [0.5, 0.0]);
    assert_eq!(output.iter().map(|f| f[0]).sum::<f32>(), 1.0);
}
#[test]
fn transactions_restore_wrapped_histories_and_reopened_speculation() {
    let factory = factory(8000, [3.25, 0.85, 0.6]);
    let mut runtime = factory.create_runtime().unwrap();
    let mut reference = factory.create_runtime().unwrap();
    let input: Vec<_> = (0..20000)
        .map(|i| if i % 997 == 0 { [0.5, -0.75] } else { [0.0; 2] })
        .collect();
    assert_eq!(
        render(runtime.as_mut(), &input[..100], &[17]),
        render(reference.as_mut(), &input[..100], &[31])
    );
    runtime.begin_block();
    let mut speculative = input.clone();
    runtime.process_block(&mut speculative[..777], &EMPTY_DEVICE_RESOURCES);
    runtime.process_block(&mut speculative[777..], &EMPTY_DEVICE_RESOURCES);
    runtime.begin_block(); // abandoning a multiwrap transaction rolls back too
    runtime.process_block(&mut [[1.0; 2]; 63], &EMPTY_DEVICE_RESOURCES);
    runtime.rollback_block();
    assert_eq!(
        render(runtime.as_mut(), &input, &[20000]),
        render(reference.as_mut(), &input, &[1, 23, 255])
    );
}
#[test]
fn retarget_glide_is_partition_independent_preserves_history_and_rollback() {
    let mut old = factory(8000, [100.0, 0.7, 1.0]).create_runtime().unwrap();
    let mut impulse = [[0.0; 2]; 400];
    impulse[0] = [1.0, 0.5];
    render(old.as_mut(), &impulse, &[17]);
    let new = factory(8000, [110.0, 0.3, 0.5]);
    let mut a = new.create_runtime().unwrap();
    let mut b = new.create_runtime().unwrap();
    assert!(a.can_retarget_parameters_from(old.as_ref()));
    a.inherit_from(old.as_ref());
    b.inherit_from(old.as_ref());
    let input = [[0.0; 2]; 1000];
    a.begin_block();
    let mut discarded = input;
    a.process_block(&mut discarded, &EMPTY_DEVICE_RESOURCES);
    a.rollback_block();
    let output = render(a.as_mut(), &input, &[1000]);
    assert_eq!(output, render(b.as_mut(), &input, &[1, 19, 63]));
    assert!(
        output.iter().any(|f| f[0] != 0.0),
        "old impulse is retained"
    );
    let a = a.as_any().downcast_ref::<Runtime>().unwrap();
    assert_eq!(a.primary.current, a.target);
    assert_eq!(a.primary.remaining, 0);
    let wrong_rate = factory(48000, [110.0, 0.3, 0.5]).create_runtime().unwrap();
    assert!(!wrong_rate.can_inherit_from(a));
}
#[test]
fn bypass_is_exact_dry_but_warm_reset_clears_and_dry_endpoint_is_exact() {
    let mut runtime = factory(8000, [10.0, 0.0, 1.0]).create_runtime().unwrap();
    runtime.begin_block();
    let mut dry = [[0.0; 2]; 80];
    dry[0] = [1.0, 0.25];
    let original = dry;
    runtime.process_bypassed_block_with_clock(&mut dry, None, &EMPTY_DEVICE_RESOURCES);
    runtime.commit_block();
    assert_eq!(dry, original);
    assert_eq!(render(runtime.as_mut(), &[[0.0; 2]], &[1]), [[1.0, 0.25]]);
    runtime.reset();
    assert!(runtime.committed_is_quiescent());
    assert!(runtime.speculative_is_quiescent());
    assert!(
        render(runtime.as_mut(), &[[0.0; 2]; 500], &[63])
            .iter()
            .all(|f| *f == [0.0; 2])
    );
    let mut dry_only = factory(8000, [10.0, 0.85, 0.0]).create_runtime().unwrap();
    assert_eq!(render(dry_only.as_mut(), &original, &[17]), original);
}
#[test]
fn codec_and_preparation_refuse_malformed_values_and_memory_is_fully_declared() {
    let state = DEFINITION.default_state().unwrap();
    assert_eq!(state, DEFINITION.migrate_state(&state).unwrap());
    for (index, descriptor) in DESCRIPTORS.iter().enumerate() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 1001.0] {
            assert!(
                DEFINITION
                    .patch_parameter(&state, descriptor.id, ParameterValue::Float(bad))
                    .is_err()
            );
            let mut bytes = state.payload().to_vec();
            bytes[index * 4..index * 4 + 4].copy_from_slice(&bad.to_le_bytes());
            assert!(decode(&NativeDeviceState::new(TYPE_ID, 1, bytes).unwrap()).is_err());
        }
    }
    for value in [
        NativeDeviceState::new(TYPE_ID, 2, vec![0; 12]).unwrap(),
        NativeDeviceState::new(TYPE_ID, 1, vec![0; 11]).unwrap(),
        NativeDeviceState::new(DeviceTypeId::from_static("hex.wrong"), 1, vec![0; 12]).unwrap(),
    ] {
        assert!(decode(&value).is_err());
    }
    for rate in [0, 7999, 192001, u32::MAX] {
        assert!(
            DEFINITION
                .prepare_runtime_factory(
                    &state,
                    &DevicePreparationContext {
                        sample_rate: rate,
                        tuning: &TuningTable::default(),
                        resources: &EMPTY_DEVICE_RESOURCES
                    }
                )
                .is_err()
        );
    }
    let factory = factory(192000, [1000.0, 0.85, 1.0]);
    let mut runtime = factory.create_runtime().unwrap();
    assert_eq!(
        runtime.fixed_storage_pair_bytes(),
        Some(factory.declared_pair_bytes())
    );
    assert!(factory.declared_pair_bytes() < hex_lattice::MAX_OWNED_PROCESSOR_PAIR_BYTES);
    let output = render(
        runtime.as_mut(),
        &[[f32::NAN, f32::INFINITY], [f32::MAX, -f32::MAX]],
        &[1],
    );
    assert!(output.iter().flatten().all(|v| v.is_finite()));
}
