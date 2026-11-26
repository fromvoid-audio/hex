use super::*;
use hex_lattice::{EMPTY_DEVICE_RESOURCES, NativeDeviceRegistry};

static DEFINITIONS: [&dyn NativeDeviceDefinition; 1] = [&CHORUS_DEFINITION];

fn registry() -> NativeDeviceRegistry<'static> {
    NativeDeviceRegistry::new(&DEFINITIONS).expect("valid chorus registry")
}

fn patch(state: &NativeDeviceState, id: ParameterId, value: f32) -> NativeDeviceState {
    CHORUS_DEFINITION
        .patch_parameter(state, id, ParameterValue::Float(value))
        .expect("valid chorus parameter")
}

fn make_runtime(state: &NativeDeviceState, rate: u32) -> Box<dyn OwnedProcessorRuntime> {
    ChorusDefinition::prepare_owned(state, rate, &[])
        .expect("valid preparation")
        .create_runtime()
        .expect("bounded runtime")
}

#[test]
fn codec_descriptors_validation_and_malformed_states() {
    let reg = registry();
    let state = reg.default_state(CHORUS_TYPE_ID).expect("default state");
    assert_eq!(state.state_version(), CHORUS_STATE_VERSION);
    assert_eq!(state.payload().len(), STATE_BYTES);
    assert_eq!(reg.migrate_state(&state).unwrap(), state);
    assert!(DESCRIPTORS.iter().all(|d| d.validate_descriptor().is_ok()));
    assert_eq!(
        CHORUS_DEFINITION.prepare_matrix(&state),
        Err(DeviceContractError::UnsupportedProcessingClass)
    );
    assert_eq!(
        CHORUS_DEFINITION.prepare_effect_matrix(&state),
        Ok(StereoMatrix::identity())
    );

    // Reject invalid parameter values & malformed states.
    for (id, value) in [
        (parameter::RATE, 0.01),
        (parameter::RATE, 15.0),
        (parameter::RATE, f32::NAN),
        (parameter::AMOUNT, -0.1),
        (parameter::AMOUNT, 1.1),
        (parameter::DELAY_MS, 0.5),
        (parameter::DELAY_MS, 30.0),
        (parameter::FEEDBACK, -0.8),
        (parameter::FEEDBACK, 0.8),
        (parameter::STEREO_WIDTH, -0.01),
        (parameter::HPF_HZ, 10.0),
        (parameter::WARMTH, 1.5),
        (parameter::DRY_WET, f32::INFINITY),
    ] {
        assert!(
            CHORUS_DEFINITION
                .patch_parameter(&state, id, ParameterValue::Float(value))
                .is_err()
        );
    }
    for len in [0, STATE_BYTES - 1, STATE_BYTES + 1] {
        let bad = NativeDeviceState::new(CHORUS_TYPE_ID, CHORUS_STATE_VERSION, vec![0; len])
            .expect("wrapper");
        assert_eq!(
            CHORUS_DEFINITION.migrate_state(&bad),
            Err(DeviceContractError::MalformedState)
        );
    }
    let future = NativeDeviceState::new(
        CHORUS_TYPE_ID,
        CHORUS_STATE_VERSION + 1,
        state.payload().to_vec(),
    )
    .unwrap();
    assert_eq!(
        CHORUS_DEFINITION.migrate_state(&future),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
    let wrong = NativeDeviceState::new(
        DeviceTypeId::from_static("hex.delay"),
        1,
        state.payload().to_vec(),
    )
    .unwrap();
    assert_eq!(
        CHORUS_DEFINITION.migrate_state(&wrong),
        Err(DeviceContractError::WrongDeviceType)
    );
}

#[test]
fn unmodulated_impulse_lands_at_exact_declared_delay_ms() {
    let base = CHORUS_DEFINITION.default_state().unwrap();
    // Zero modulation, zero feedback, 100% wet, open warmth, minimum HPF.
    let state = patch(
        &patch(
            &patch(
                &patch(
                    &patch(
                        &patch(&base, parameter::AMOUNT, 0.0),
                        parameter::DELAY_MS,
                        10.0,
                    ),
                    parameter::FEEDBACK,
                    0.0,
                ),
                parameter::HPF_HZ,
                20.0,
            ),
            parameter::WARMTH,
            0.0,
        ),
        parameter::DRY_WET,
        1.0,
    );
    let rate = 48_000_u32;
    let mut rt = make_runtime(&state, rate);
    let mut frames = vec![[0.0_f32; 2]; 1_000];
    frames[0] = [1.0, -0.5];
    rt.begin_block();
    rt.process_block(&mut frames, &EMPTY_DEVICE_RESOURCES);
    rt.commit_block();
    // 10 ms at 48 kHz = 480 frames exactly.
    assert!(
        (frames[480][0] - 1.0).abs() < 1e-5,
        "expected left impulse at frame 480, got {}",
        frames[480][0]
    );
    assert!(
        (frames[480][1] + 0.5).abs() < 1e-5,
        "expected right impulse at frame 480, got {}",
        frames[480][1]
    );
    assert!(frames[0][0].abs() < 1e-6);
    assert!(frames[479][0].abs() < 1e-6);
}

#[test]
fn stereo_width_dry_wet_feedback_quiescence_and_irregular_partitions_hold() {
    let base = CHORUS_DEFINITION.default_state().unwrap();

    // 0% wet is bit-exact dry.
    let dry_state = patch(&base, parameter::DRY_WET, 0.0);
    let input: Vec<StereoFrame> = (0..512)
        .map(|i| {
            let t = i as f64 / 48_000.0;
            [
                (TAU * 440.0 * t).sin() as f32,
                (TAU * 660.0 * t).cos() as f32,
            ]
        })
        .collect();
    let mut dry_out = input.clone();
    let mut rt = make_runtime(&dry_state, 48_000);
    rt.begin_block();
    rt.process_block(&mut dry_out, &EMPTY_DEVICE_RESOURCES);
    rt.commit_block();
    assert_eq!(dry_out, input);

    // Stereo width 0 => identical L/R on mono input; width 1 => wide quadrature stereo separation.
    let mono_in: Vec<StereoFrame> = (0..2_048)
        .map(|i| {
            let s = (TAU * 520.0 * i as f64 / 48_000.0).sin() as f32;
            [s, s]
        })
        .collect();
    let mono_chorus = patch(&base, parameter::STEREO_WIDTH, 0.0);
    let wide_chorus = patch(&base, parameter::STEREO_WIDTH, 1.0);
    let mut out_mono = mono_in.clone();
    let mut out_wide = mono_in.clone();
    let mut rt_m = make_runtime(&mono_chorus, 48_000);
    rt_m.begin_block();
    rt_m.process_block(&mut out_mono, &EMPTY_DEVICE_RESOURCES);
    rt_m.commit_block();
    let mut rt_w = make_runtime(&wide_chorus, 48_000);
    rt_w.begin_block();
    rt_w.process_block(&mut out_wide, &EMPTY_DEVICE_RESOURCES);
    rt_w.commit_block();
    assert!(out_mono.iter().all(|f| f[0] == f[1]));
    assert!(
        out_wide.iter().any(|f| (f[0] - f[1]).abs() > 0.1),
        "stereo width 1.0 must separate L/R channels"
    );

    // Positive feedback creates echo repeats and drains to strict quiescence within declared tail.
    let fb_state = patch(
        &patch(&base, parameter::FEEDBACK, 0.7),
        parameter::DRY_WET,
        1.0,
    );
    let factory = ChorusDefinition::prepare_owned(&fb_state, 48_000, &[]).unwrap();
    let OutputTail::Finite { max_tail_frames } = factory.output_tail() else {
        panic!("expected finite tail");
    };
    let mut rt_fb = factory.create_runtime().unwrap();
    rt_fb.begin_block();
    rt_fb.process_block(&mut [[0.8, -0.8]; 128], &EMPTY_DEVICE_RESOURCES);
    rt_fb.commit_block();
    assert!(!rt_fb.committed_is_quiescent());
    let mut drain = vec![[0.0_f32; 2]; max_tail_frames as usize];
    rt_fb.begin_block();
    rt_fb.process_block(&mut drain, &EMPTY_DEVICE_RESOURCES);
    rt_fb.commit_block();
    assert!(rt_fb.committed_is_quiescent());

    // Non-finite inputs never poison subsequent finite frames; rollback/commit & irregular partitions are exact.
    let mut whole = mono_in[..400].to_vec();
    let mut rt_whole = make_runtime(&wide_chorus, 48_000);
    rt_whole.begin_block();
    rt_whole.process_block(&mut whole, &EMPTY_DEVICE_RESOURCES);
    rt_whole.commit_block();

    let mut chunked = mono_in[..400].to_vec();
    let mut rt_chunk = make_runtime(&wide_chorus, 48_000);
    let mut pos = 0;
    for step in [1, 3, 7, 19, 64, 100, 206] {
        let end = (pos + step).min(chunked.len());
        // Exercise rollback before commit to prove O(dirty_len) speculative safety.
        let mut trial = chunked[pos..end].to_vec();
        rt_chunk.begin_block();
        rt_chunk.process_block(&mut trial, &EMPTY_DEVICE_RESOURCES);
        rt_chunk.rollback_block();

        rt_chunk.begin_block();
        rt_chunk.process_block(&mut chunked[pos..end], &EMPTY_DEVICE_RESOURCES);
        rt_chunk.commit_block();
        pos = end;
    }
    assert_eq!(whole, chunked);

    // Non-finite fault boundary check.
    let mut bad = [[f32::NAN, f32::INFINITY]];
    rt_whole.begin_block();
    rt_whole.process_block(&mut bad, &EMPTY_DEVICE_RESOURCES);
    rt_whole.commit_block();
    assert!(bad[0][0].is_nan() && bad[0][1].is_infinite());
    let mut after = [[0.25_f32, -0.25_f32]; 32];
    rt_whole.begin_block();
    rt_whole.process_block(&mut after, &EMPTY_DEVICE_RESOURCES);
    rt_whole.commit_block();
    assert!(after.iter().all(|f| f[0].is_finite() && f[1].is_finite()));

    // Reset restores deterministic start.
    rt_whole.reset();
    let mut after_reset = mono_in[..400].to_vec();
    rt_whole.begin_block();
    rt_whole.process_block(&mut after_reset, &EMPTY_DEVICE_RESOURCES);
    rt_whole.commit_block();
    assert_eq!(whole, after_reset);
}

#[test]
fn automation_and_history_transfer_preserve_state_and_storage_accounting() {
    struct AutoWet;
    impl NativeParameterAutomation for AutoWet {
        fn parameter_ids(&self) -> &[ParameterId] {
            std::slice::from_ref(&parameter::DRY_WET)
        }
        fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
            (id == parameter::DRY_WET).then_some(ParameterValue::Float(1.0))
        }
    }
    let state = CHORUS_DEFINITION.default_state().unwrap();
    let factory = ChorusDefinition::prepare_owned(&state, 48_000, &[parameter::DRY_WET]).unwrap();
    let mut rt = factory.create_runtime().unwrap();
    assert_eq!(
        rt.fixed_storage_pair_bytes(),
        Some(factory.declared_pair_bytes())
    );
    let mut frames = [[0.5_f32, -0.5_f32]; 64];
    rt.begin_block();
    rt.process_block_with_automation(&mut frames, None, &AutoWet, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    rt.commit_block();

    let mut next_rt = factory.create_runtime().unwrap();
    assert!(next_rt.can_inherit_from(rt.as_ref()));
    next_rt.transfer_history_from(rt.as_mut());
    assert!(rt.committed_is_quiescent());
    assert!(!next_rt.committed_is_quiescent());
}
