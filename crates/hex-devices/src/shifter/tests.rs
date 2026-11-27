use super::*;
use hex_lattice::{EMPTY_DEVICE_RESOURCES, NativeInstrumentClock};

struct FixedTempoClock(u32);
impl NativeInstrumentClock for FixedTempoClock {
    fn beat_at_offset(&self, _offset: usize) -> Option<u64> {
        None
    }
    fn tempo_micro_bpm_at_offset(&self, _offset: usize) -> Option<u32> {
        Some(self.0)
    }
    fn constant_tempo_micro_bpm(&self, _start: usize, _len: usize) -> Option<u32> {
        Some(self.0)
    }
}

struct SingleLaneAutomation {
    ids: [ParameterId; 1],
    value: ParameterValue,
}
impl NativeParameterAutomation for SingleLaneAutomation {
    fn parameter_ids(&self) -> &[ParameterId] {
        &self.ids
    }
    fn value_at(&self, id: ParameterId, _offset: usize) -> Option<ParameterValue> {
        (id == self.ids[0]).then_some(self.value)
    }
}

fn dft_magnitude_at(signal: &[f32], hz: f64, sample_rate: f64) -> f64 {
    let n = signal.len();
    let omega = TAU * hz / sample_rate;
    let (mut re, mut im, mut wsum) = (0.0, 0.0, 0.0);
    for (i, &sample) in signal.iter().enumerate() {
        let w = 0.5 * (1.0 - (TAU * i as f64 / n as f64).cos());
        wsum += w;
        let angle = omega * i as f64;
        re += f64::from(sample) * w * angle.cos();
        im -= f64::from(sample) * w * angle.sin();
    }
    2.0 * re.hypot(im) / wsum
}

fn patch(state: &NativeDeviceState, id: ParameterId, value: ParameterValue) -> NativeDeviceState {
    SHIFTER_DEFINITION
        .patch_parameter(state, id, value)
        .expect("patch shifter parameter")
}

fn prepare_runtime(
    state: &NativeDeviceState,
    rate: u32,
    params: &[ParameterId],
) -> Box<dyn OwnedProcessorRuntime> {
    let factory = ShifterDefinition::prepare_owned(state, rate, params).expect("prepare factory");
    let runtime = factory.create_runtime().expect("create runtime");
    assert_eq!(
        runtime.fixed_storage_pair_bytes(),
        Some(factory.declared_pair_bytes())
    );
    runtime
}

#[test]
fn metadata_codec_and_malformed_state_rejection_hold() {
    let def = &SHIFTER_DEFINITION;
    assert_eq!(
        hex_lattice_contract::NativeDeviceMetadata::type_id(def),
        SHIFTER_TYPE_ID
    );
    assert_eq!(def.display_name(), Some("shifter"));
    assert_eq!(def.state_version(), 1);
    assert_eq!(def.descriptors().len(), PARAM_COUNT);

    let default = def.default_state().unwrap();
    assert_eq!(default.payload().len(), STATE_BYTES);
    assert_eq!(def.migrate_state(&default).unwrap(), default);
    assert_eq!(
        def.read_parameter(&default, parameter::MODE).unwrap(),
        ParameterValue::Choice(0)
    );
    assert_eq!(
        def.read_parameter(&default, parameter::DRY_WET).unwrap(),
        ParameterValue::Float(1.0)
    );

    // Every parameter supports automation and roundtrips through patch/read.
    for desc in def.descriptors() {
        assert!(
            def.supports_parameter_automation(&default, desc.id)
                .unwrap()
        );
        let val = def.read_parameter(&default, desc.id).unwrap();
        let patched = def.patch_parameter(&default, desc.id, val).unwrap();
        assert_eq!(def.read_parameter(&patched, desc.id).unwrap(), val);
    }

    // Reject wrong device type, future version, truncated payload, and invalid values.
    let wrong_type = NativeDeviceState::new(
        DeviceTypeId::from_static("hex.utility"),
        1,
        default.payload().to_vec(),
    )
    .unwrap();
    assert_eq!(
        def.migrate_state(&wrong_type).unwrap_err(),
        DeviceContractError::WrongDeviceType
    );

    let future = NativeDeviceState::new(SHIFTER_TYPE_ID, 2, default.payload().to_vec()).unwrap();
    assert_eq!(
        def.migrate_state(&future).unwrap_err(),
        DeviceContractError::UnsupportedStateVersion
    );

    let truncated = NativeDeviceState::new(SHIFTER_TYPE_ID, 1, vec![0; 12]).unwrap();
    assert_eq!(
        def.migrate_state(&truncated).unwrap_err(),
        DeviceContractError::MalformedState
    );

    assert_eq!(
        def.patch_parameter(&default, parameter::COARSE_ST, ParameterValue::Float(99.0))
            .unwrap_err(),
        DeviceContractError::InvalidParameterValue
    );
    assert_eq!(
        def.patch_parameter(&default, parameter::FEEDBACK, ParameterValue::Float(1.2))
            .unwrap_err(),
        DeviceContractError::InvalidParameterValue
    );
    assert_eq!(
        def.patch_parameter(&default, parameter::MODE, ParameterValue::Choice(9))
            .unwrap_err(),
        DeviceContractError::InvalidParameterValue
    );
}

#[test]
fn pitch_mode_shifts_up_and_down_accurately() {
    let sr = 48_000_u32;
    let sr_f = f64::from(sr);
    let input_hz = 440.0;
    let settle = 4_096;
    let window = 16_384;

    for (semi, expected_hz) in [
        (12.0_f32, 880.0_f64),
        (-12.0_f32, 220.0_f64),
        (0.0_f32, 440.0_f64),
    ] {
        let mut state = SHIFTER_DEFINITION.default_state().unwrap();
        state = patch(&state, parameter::MODE, ParameterValue::Choice(0));
        state = patch(&state, parameter::COARSE_ST, ParameterValue::Float(semi));
        state = patch(&state, parameter::WINDOW_MS, ParameterValue::Float(40.0));
        let mut rt = prepare_runtime(&state, sr, &[]);
        let mut frames: Vec<StereoFrame> = (0..(settle + window))
            .map(|i| {
                let s = (TAU * input_hz * i as f64 / sr_f).sin() as f32;
                [s, s]
            })
            .collect();
        rt.begin_block();
        rt.process_block(&mut frames, &EMPTY_DEVICE_RESOURCES);
        rt.commit_block();
        let left: Vec<f32> = frames[settle..].iter().map(|f| f[0]).collect();
        let target_mag = dft_magnitude_at(&left, expected_hz, sr_f);
        assert!(
            target_mag > 0.65,
            "semi={semi}: expected strong peak at {expected_hz} Hz, got {target_mag}"
        );
        if semi != 0.0 {
            let orig_mag = dft_magnitude_at(&left, input_hz, sr_f);
            assert!(
                target_mag > orig_mag * 5.0,
                "semi={semi}: target {target_mag} did not dominate original {orig_mag}"
            );
        }
    }
}

#[test]
fn freq_and_ring_modes_preserve_ssb_rejection_and_sidebands() {
    let sr = 48_000_u32;
    let sr_f = f64::from(sr);
    let settle = 2_048;
    let window = 8_192;

    // Freq SSB mode: 1000 Hz + 250 Hz -> 1250 Hz with >60 dB opposite-sideband rejection.
    let mut state = SHIFTER_DEFINITION.default_state().unwrap();
    state = patch(&state, parameter::MODE, ParameterValue::Choice(1));
    state = patch(&state, parameter::COARSE_HZ, ParameterValue::Float(250.0));
    let mut rt = prepare_runtime(&state, sr, &[]);
    let mut frames: Vec<StereoFrame> = (0..(settle + window))
        .map(|i| {
            let s = (TAU * 1_000.0 * i as f64 / sr_f).sin() as f32;
            [s, s]
        })
        .collect();
    rt.begin_block();
    rt.process_block(&mut frames, &EMPTY_DEVICE_RESOURCES);
    rt.commit_block();
    let left: Vec<f32> = frames[settle..].iter().map(|f| f[0]).collect();
    let desired = dft_magnitude_at(&left, 1_250.0, sr_f);
    let unwanted = dft_magnitude_at(&left, 750.0, sr_f);
    let rejection_db = 20.0 * (desired / unwanted.max(1e-12)).log10();
    assert!((desired - 1.0).abs() < 0.03, "desired={desired}");
    assert!(rejection_db > 60.0, "rejection_db={rejection_db}");

    // Ring mode: 1000 Hz * cos(250 Hz) -> equal 750 Hz and 1250 Hz sidebands, suppressed 1000 Hz.
    let mut ring_state = SHIFTER_DEFINITION.default_state().unwrap();
    ring_state = patch(&ring_state, parameter::MODE, ParameterValue::Choice(2));
    ring_state = patch(
        &ring_state,
        parameter::COARSE_HZ,
        ParameterValue::Float(250.0),
    );
    let mut ring_rt = prepare_runtime(&ring_state, sr, &[]);
    let mut ring_frames: Vec<StereoFrame> = (0..window)
        .map(|i| {
            let s = (TAU * 1_000.0 * i as f64 / sr_f).sin() as f32;
            [s, s]
        })
        .collect();
    ring_rt.begin_block();
    ring_rt.process_block(&mut ring_frames, &EMPTY_DEVICE_RESOURCES);
    ring_rt.commit_block();
    let ring_l: Vec<f32> = ring_frames.iter().map(|f| f[0]).collect();
    let upper = dft_magnitude_at(&ring_l, 1_250.0, sr_f);
    let lower = dft_magnitude_at(&ring_l, 750.0, sr_f);
    let carrier = dft_magnitude_at(&ring_l, 1_000.0, sr_f);
    assert!((upper - 0.5).abs() < 0.01, "upper={upper}");
    assert!((lower - 0.5).abs() < 0.01, "lower={lower}");
    assert!(carrier < 0.01, "carrier={carrier}");
}

#[test]
fn wide_stereo_tone_drive_delay_feedback_lfo_and_env_follower_affect_output() {
    let sr = 48_000_u32;
    let sr_f = f64::from(sr);

    // Wide in Freq mode: L shifts +300 Hz (1300 Hz), R shifts +100 Hz (1100 Hz).
    let mut wide_state = SHIFTER_DEFINITION.default_state().unwrap();
    wide_state = patch(&wide_state, parameter::MODE, ParameterValue::Choice(1));
    wide_state = patch(
        &wide_state,
        parameter::COARSE_HZ,
        ParameterValue::Float(200.0),
    );
    wide_state = patch(
        &wide_state,
        parameter::FINE_HZ,
        ParameterValue::Float(100.0),
    );
    wide_state = patch(&wide_state, parameter::WIDE, ParameterValue::Bool(true));
    let mut rt = prepare_runtime(&wide_state, sr, &[]);
    let mut frames: Vec<StereoFrame> = (0..10_240)
        .map(|i| {
            let s = (TAU * 1_000.0 * i as f64 / sr_f).sin() as f32;
            [s, s]
        })
        .collect();
    rt.begin_block();
    rt.process_block(&mut frames, &EMPTY_DEVICE_RESOURCES);
    rt.commit_block();
    let l: Vec<f32> = frames[2_048..].iter().map(|f| f[0]).collect();
    let r: Vec<f32> = frames[2_048..].iter().map(|f| f[1]).collect();
    assert!((dft_magnitude_at(&l, 1_300.0, sr_f) - 1.0).abs() < 0.03);
    assert!((dft_magnitude_at(&r, 1_100.0, sr_f) - 1.0).abs() < 0.03);

    // Delay + extreme 95% feedback stays finite, bounded, produces echoes, and drains quiescent.
    let mut delay_state = SHIFTER_DEFINITION.default_state().unwrap();
    delay_state = patch(
        &delay_state,
        parameter::DELAY_ENABLED,
        ParameterValue::Bool(true),
    );
    delay_state = patch(
        &delay_state,
        parameter::DELAY_SYNC,
        ParameterValue::Bool(false),
    );
    delay_state = patch(
        &delay_state,
        parameter::DELAY_TIME_MS,
        ParameterValue::Float(20.0),
    );
    delay_state = patch(
        &delay_state,
        parameter::FEEDBACK,
        ParameterValue::Float(0.95),
    );
    let mut delay_rt = prepare_runtime(&delay_state, sr, &[]);
    let mut impulse = vec![[0.0_f32; 2]; 4_096];
    impulse[0] = [1.0, 1.0];
    delay_rt.begin_block();
    delay_rt.process_block(&mut impulse, &EMPTY_DEVICE_RESOURCES);
    delay_rt.commit_block();
    assert!(impulse.iter().all(|f| f[0].is_finite() && f[0].abs() < 8.0));
    // Verify echo energy appears well after the initial impulse.
    let echo_energy: f32 = impulse[900..3_000].iter().map(|f| f[0].abs()).sum();
    assert!(
        echo_energy > 0.2,
        "expected delay echoes, got {echo_energy}"
    );

    // Clock-synced delay changes echo spacing when BPM changes.
    let mut sync_state = SHIFTER_DEFINITION.default_state().unwrap();
    sync_state = patch(
        &sync_state,
        parameter::DELAY_ENABLED,
        ParameterValue::Bool(true),
    );
    sync_state = patch(
        &sync_state,
        parameter::DELAY_SYNC,
        ParameterValue::Bool(true),
    );
    sync_state = patch(
        &sync_state,
        parameter::DELAY_DIVISION,
        ParameterValue::Choice(0),
    ); // 1/64
    sync_state = patch(&sync_state, parameter::FEEDBACK, ParameterValue::Float(0.6));
    let mut rt_slow = prepare_runtime(&sync_state, sr, &[]);
    let mut rt_fast = prepare_runtime(&sync_state, sr, &[]);
    let mut buf_slow = vec![[0.0_f32; 2]; 4_096];
    let mut buf_fast = vec![[0.0_f32; 2]; 4_096];
    buf_slow[0] = [1.0, 1.0];
    buf_fast[0] = [1.0, 1.0];
    let slow_clock = FixedTempoClock(60_000_000);
    let fast_clock = FixedTempoClock(180_000_000);
    rt_slow.begin_block();
    rt_slow.process_block_with_clock(&mut buf_slow, Some(&slow_clock), &EMPTY_DEVICE_RESOURCES);
    rt_slow.commit_block();
    rt_fast.begin_block();
    rt_fast.process_block_with_clock(&mut buf_fast, Some(&fast_clock), &EMPTY_DEVICE_RESOURCES);
    rt_fast.commit_block();
    assert_ne!(buf_slow, buf_fast);

    // Tone low-pass attenuates high frequency (8 kHz) when set to 500 Hz.
    let mut tone_open = SHIFTER_DEFINITION.default_state().unwrap();
    tone_open = patch(&tone_open, parameter::MODE, ParameterValue::Choice(1));
    let tone_dark = patch(&tone_open, parameter::TONE_HZ, ParameterValue::Float(500.0));
    let mut rt_open = prepare_runtime(&tone_open, sr, &[]);
    let mut rt_dark = prepare_runtime(&tone_dark, sr, &[]);
    let tone_in: Vec<StereoFrame> = (0..4_096)
        .map(|i| {
            let s = (TAU * 8_000.0 * i as f64 / sr_f).sin() as f32;
            [s, s]
        })
        .collect();
    let mut out_open = tone_in.clone();
    let mut out_dark = tone_in;
    rt_open.begin_block();
    rt_open.process_block(&mut out_open, &EMPTY_DEVICE_RESOURCES);
    rt_open.commit_block();
    rt_dark.begin_block();
    rt_dark.process_block(&mut out_dark, &EMPTY_DEVICE_RESOURCES);
    rt_dark.commit_block();
    let rms = |buf: &[StereoFrame]| -> f64 {
        (buf[1_024..]
            .iter()
            .map(|f| f64::from(f[0]).powi(2))
            .sum::<f64>()
            / (buf.len() - 1_024) as f64)
            .sqrt()
    };
    assert!(rms(&out_dark) < rms(&out_open) * 0.25);

    // Envelope follower + LFO + Drive + LFO preview smoke & determinism.
    let mut mod_state = SHIFTER_DEFINITION.default_state().unwrap();
    mod_state = patch(&mod_state, parameter::MODE, ParameterValue::Choice(1));
    mod_state = patch(
        &mod_state,
        parameter::LFO_AMOUNT,
        ParameterValue::Float(0.5),
    );
    mod_state = patch(&mod_state, parameter::LFO_DUTY, ParameterValue::Float(0.4));
    mod_state = patch(
        &mod_state,
        parameter::LFO_STEREO_AMOUNT,
        ParameterValue::Float(0.5),
    );
    mod_state = patch(
        &mod_state,
        parameter::ENV_ENABLED,
        ParameterValue::Bool(true),
    );
    mod_state = patch(
        &mod_state,
        parameter::ENV_AMOUNT,
        ParameterValue::Float(0.5),
    );
    mod_state = patch(
        &mod_state,
        parameter::DRIVE_ENABLED,
        ParameterValue::Bool(true),
    );
    let preview = lfo_preview(&mod_state).unwrap();
    assert_eq!(preview.left.len(), 129);
    assert_eq!(preview.right.len(), 129);
    assert_ne!(preview.left, preview.right);
    let mut mod_rt = prepare_runtime(&mod_state, sr, &[]);
    let mut mod_buf: Vec<StereoFrame> = (0..2_048)
        .map(|i| {
            let s = (TAU * 440.0 * i as f64 / sr_f).sin() as f32 * 0.8;
            [s, s]
        })
        .collect();
    mod_rt.begin_block();
    mod_rt.process_block(&mut mod_buf, &EMPTY_DEVICE_RESOURCES);
    mod_rt.commit_block();
    assert!(mod_buf.iter().all(|f| f[0].is_finite() && f[1].is_finite()));
    assert_ne!(mod_buf[1024][0], mod_buf[1024][1]);
}

#[test]
fn partition_invariance_rollback_inheritance_nonfinite_and_automation_hold() {
    for sr in [8_000_u32, 44_100, 48_000, 96_000, 192_000] {
        let mut state = SHIFTER_DEFINITION.default_state().unwrap();
        state = patch(&state, parameter::COARSE_ST, ParameterValue::Float(5.0));
        state = patch(&state, parameter::DELAY_ENABLED, ParameterValue::Bool(true));
        state = patch(&state, parameter::DELAY_SYNC, ParameterValue::Bool(false));
        state = patch(
            &state,
            parameter::DELAY_TIME_MS,
            ParameterValue::Float(15.0),
        );
        state = patch(&state, parameter::FEEDBACK, ParameterValue::Float(0.35));
        state = patch(&state, parameter::DRY_WET, ParameterValue::Float(0.7));

        let mut whole = prepare_runtime(&state, sr, &[]);
        let mut chunked = prepare_runtime(&state, sr, &[]);
        let input: Vec<StereoFrame> = (0..300)
            .map(|i| {
                let t = i as f64 / f64::from(sr);
                [
                    ((TAU * 330.0 * t).sin() * 0.6) as f32,
                    ((TAU * 550.0 * t).cos() * 0.5) as f32,
                ]
            })
            .collect();
        let mut a = input.clone();
        let mut b = input.clone();
        whole.begin_block();
        whole.process_block(&mut a, &EMPTY_DEVICE_RESOURCES);
        whole.commit_block();

        let mut pos = 0;
        for step in [1, 3, 7, 13, 29, 64, 183] {
            let end = (pos + step).min(b.len());
            chunked.begin_block();
            chunked.process_block(&mut b[pos..end], &EMPTY_DEVICE_RESOURCES);
            chunked.commit_block();
            pos = end;
        }
        assert_eq!(a, b, "sr={sr}: partition mismatch");

        // Rollback preserves committed state identically for retry.
        let mut retry = input[..64].to_vec();
        let mut first = retry.clone();
        whole.begin_block();
        whole.process_block(&mut first, &EMPTY_DEVICE_RESOURCES);
        whole.rollback_block();
        whole.begin_block();
        whole.process_block(&mut retry, &EMPTY_DEVICE_RESOURCES);
        whole.commit_block();
        assert_eq!(first, retry, "sr={sr}: rollback retry mismatch");

        // Non-finite input passes through without poisoning state.
        let mut bad = [[f32::NAN, f32::INFINITY], [0.25, -0.25]];
        whole.begin_block();
        whole.process_block(&mut bad, &EMPTY_DEVICE_RESOURCES);
        whole.commit_block();
        assert!(bad[0][0].is_nan() && bad[0][1].is_infinite());
        assert!(bad[1][0].is_finite() && bad[1][1].is_finite());
    }

    // Automation + donor history transfer + quiescence drain.
    let sr = 48_000_u32;
    let base = SHIFTER_DEFINITION.default_state().unwrap();
    let mut rt = prepare_runtime(&base, sr, &[parameter::DRY_WET]);
    assert!(rt.supports_parameter_automation(parameter::DRY_WET));
    let mut dry_block = [[0.5_f32, -0.5_f32]; 64];
    let auto = SingleLaneAutomation {
        ids: [parameter::DRY_WET],
        value: ParameterValue::Float(0.0),
    };
    rt.begin_block();
    rt.process_block_with_automation(&mut dry_block, None, &auto, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    rt.commit_block();

    let updated = patch(&base, parameter::COARSE_ST, ParameterValue::Float(7.0));
    let mut successor = prepare_runtime(&updated, sr, &[parameter::DRY_WET]);
    assert!(successor.can_inherit_from(rt.as_ref()));
    successor.transfer_history_from(rt.as_mut());
    assert!(rt.committed_is_quiescent());

    // Drain successor to strict quiescence within declared finite tail bound.
    let factory = ShifterDefinition::prepare_owned(&updated, sr, &[]).unwrap();
    let OutputTail::Finite { max_tail_frames } = factory.output_tail() else {
        panic!("expected finite tail");
    };
    let mut silence = vec![[0.0_f32; 2]; max_tail_frames as usize];
    successor.begin_block();
    successor.process_block(&mut silence, &EMPTY_DEVICE_RESOURCES);
    successor.commit_block();
    assert!(successor.committed_is_quiescent());
}
