use super::*;
use hex_lattice::{NativeDeviceRegistry, PreparedNativeProcessor};

static DEFINITIONS: [&dyn NativeDeviceDefinition; 1] = [&AUTO_PAN_DEFINITION];

fn registry() -> NativeDeviceRegistry<'static> {
    NativeDeviceRegistry::new(&DEFINITIONS).expect("valid Auto Pan registry")
}

fn patch(state: &NativeDeviceState, id: ParameterId, value: f32) -> NativeDeviceState {
    AUTO_PAN_DEFINITION
        .patch_parameter(state, id, ParameterValue::Float(value))
        .expect("valid Auto Pan parameter")
}

fn prepared(state: &NativeDeviceState, sample_rate: u32) -> Box<dyn PreparedNativeProcessor> {
    AUTO_PAN_DEFINITION
        .prepare_processor(
            state,
            &hex_lattice::DevicePreparationContext {
                sample_rate,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("valid Auto Pan preparation")
        .expect("stateful Auto Pan")
}

fn runtime(state: &NativeDeviceState, sample_rate: u32) -> Box<dyn NativeProcessorRuntime> {
    prepared(state, sample_rate).create_runtime()
}

#[test]
fn defaults_and_generic_codec_roundtrip_four_controls() {
    let registry = registry();
    let state = registry.default_state(AUTO_PAN_TYPE_ID).expect("defaults");
    assert_eq!(state.state_version(), AUTO_PAN_STATE_VERSION);
    assert_eq!(state.payload().len(), STATE_BYTES);
    assert_eq!(registry.migrate_state(&state).expect("migration"), state);
    for (id, expected) in [
        (parameter::AMOUNT, 0.5),
        (parameter::RATE, 1.0),
        (parameter::RIGHT_PHASE, 180.0),
    ] {
        assert_eq!(
            AUTO_PAN_DEFINITION.read_parameter(&state, id),
            Ok(ParameterValue::Float(expected))
        );
    }
    assert!(
        DESCRIPTORS
            .iter()
            .all(|descriptor| descriptor.validate_descriptor().is_ok())
    );
    assert_eq!(
        AUTO_PAN_DEFINITION.prepare_matrix(&state),
        Err(DeviceContractError::UnsupportedProcessingClass)
    );
}

#[test]
fn rejects_malformed_versions_payloads_nonfinite_parameters_and_sample_rates() {
    let state = registry()
        .default_state(AUTO_PAN_TYPE_ID)
        .expect("defaults");
    for (id, value) in [
        (parameter::AMOUNT, f32::NAN),
        (parameter::AMOUNT, -0.01),
        (parameter::AMOUNT, 1.01),
        (parameter::RATE, 0.049),
        (parameter::RATE, 20.01),
        (parameter::RIGHT_PHASE, f32::INFINITY),
        (parameter::RIGHT_PHASE, -0.01),
        (parameter::RIGHT_PHASE, 360.01),
    ] {
        assert!(
            AUTO_PAN_DEFINITION
                .patch_parameter(&state, id, ParameterValue::Float(value))
                .is_err()
        );
    }
    for length in [STATE_BYTES - 1, STATE_BYTES + 1] {
        let malformed =
            NativeDeviceState::new(AUTO_PAN_TYPE_ID, AUTO_PAN_STATE_VERSION, vec![0; length])
                .expect("bounded malformed payload");
        assert_eq!(
            AUTO_PAN_DEFINITION.migrate_state(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }
    let wrong = NativeDeviceState::new(
        DeviceTypeId::from_static("hex.gate"),
        1,
        state.payload().to_vec(),
    )
    .expect("wrong type state");
    assert_eq!(
        AUTO_PAN_DEFINITION.migrate_state(&wrong),
        Err(DeviceContractError::WrongDeviceType)
    );
    let future = NativeDeviceState::new(
        AUTO_PAN_TYPE_ID,
        AUTO_PAN_STATE_VERSION + 1,
        state.payload().to_vec(),
    )
    .expect("future version");
    assert_eq!(
        AUTO_PAN_DEFINITION.migrate_state(&future),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
    let mut nonfinite = state.payload().to_vec();
    nonfinite[4..8].copy_from_slice(&f32::NAN.to_le_bytes());
    let nonfinite = NativeDeviceState::new(AUTO_PAN_TYPE_ID, AUTO_PAN_STATE_VERSION, nonfinite)
        .expect("bounded nonfinite state");
    assert_eq!(
        AUTO_PAN_DEFINITION.migrate_state(&nonfinite),
        Err(DeviceContractError::InvalidParameterValue)
    );
    for sample_rate in [0, 7_999, 192_001] {
        assert!(
            AUTO_PAN_DEFINITION
                .prepare_processor(
                    &state,
                    &hex_lattice::DevicePreparationContext {
                        sample_rate,
                        tuning: &hex_lattice::TuningTable::default(),
                        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES
                    }
                )
                .is_err()
        );
    }
    for sample_rate in [8_000, 48_000, 192_000] {
        assert!(
            AUTO_PAN_DEFINITION
                .prepare_processor(
                    &state,
                    &hex_lattice::DevicePreparationContext {
                        sample_rate,
                        tuning: &hex_lattice::TuningTable::default(),
                        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES
                    }
                )
                .is_ok()
        );
    }
}

#[test]
fn sine_pan_has_complementary_channels_and_zero_amount_is_bit_exact_unity() {
    let state = registry()
        .default_state(AUTO_PAN_TYPE_ID)
        .expect("defaults");
    let mut pan = runtime(&state, 8_000);
    let mut samples = [[1.0, 1.0]; 6_001];
    pan.process_block(&mut samples[..1], &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(samples[0], [0.75, 0.75]);
    pan.process_block(&mut samples[1..2_001], &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!((samples[2_000][0] - 1.0).abs() < 1.0e-12);
    assert!((samples[2_000][1] - 0.5).abs() < 1.0e-12);
    pan.process_block(
        &mut samples[2_001..4_001],
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    assert_eq!(samples[4_000], [0.75, 0.75]);
    pan.process_block(&mut samples[4_001..], &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!((samples[6_000][0] - 0.5).abs() < 1.0e-12);
    assert!((samples[6_000][1] - 1.0).abs() < 1.0e-12);

    let unity = patch(&state, parameter::AMOUNT, 0.0);
    let mut unity_runtime = runtime(&unity, 48_000);
    let mut values = [[f32::from_bits(0x3e80_0001), -0.75], [0.0, -0.0]];
    let original = values;
    unity_runtime.process_block(&mut values, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(
        values.map(|frame| frame.map(f32::to_bits)),
        original.map(|frame| frame.map(f32::to_bits))
    );
}

#[test]
fn zero_offset_tremolo_reset_and_block_partition_are_deterministic() {
    let defaults = registry()
        .default_state(AUTO_PAN_TYPE_ID)
        .expect("defaults");
    let tremolo = patch(&defaults, parameter::RIGHT_PHASE, 0.0);
    let mut whole = runtime(&tremolo, 8_000);
    let mut split = runtime(&tremolo, 8_000);
    let mut a = [[1.0, 1.0]; 4_017];
    let mut b = a;
    whole.process_block(&mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    for chunk in b.chunks_mut(113) {
        split.process_block(chunk, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    }
    assert_eq!(a, b);
    assert_eq!(a[0][0], 0.75);
    assert_eq!(a[0][0], a[0][1]);
    assert!((a[2_000][0] - 1.0).abs() < 1.0e-7);
    assert_eq!(a[2_000][0], a[2_000][1]);
    whole.reset();
    let mut restarted = [[1.0, 1.0]; 2];
    whole.process_block(&mut restarted, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(restarted[0], [0.75, 0.75]);
    assert_eq!(restarted[1], [0.750_196_35, 0.750_196_35]);
}

#[test]
fn bypass_advances_clock_and_nonfinite_frames_skip_clock_and_ramps() {
    let state = registry()
        .default_state(AUTO_PAN_TYPE_ID)
        .expect("defaults");
    let mut bypassed = runtime(&state, 8_000);
    let mut continuous = runtime(&state, 8_000);
    let mut dry = [[0.25, -0.75]; 2_000];
    let original = dry;
    bypassed.process_bypassed_block(&mut dry, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(dry, original);
    let mut after_bypass = [[1.0, 1.0]; 1];
    let mut after_silence = [[1.0, 1.0]; 1];
    bypassed.process_block(&mut after_bypass, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    continuous.process_block(
        &mut [[1.0, 1.0]; 2_000],
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    continuous.process_block(&mut after_silence, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(after_bypass, after_silence);

    let mut with_bad = runtime(&state, 8_000);
    let mut untouched = runtime(&state, 8_000);
    let mut bad = [[f32::NAN, 0.25], [1.0, 1.0]];
    with_bad.process_block(&mut bad, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let mut baseline = [[1.0, 1.0]];
    untouched.process_block(&mut baseline, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(bad[0][0].is_nan());
    assert_eq!(bad[0][1], 0.25);
    assert_eq!(bad[1], baseline[0]);
}

#[test]
fn compatible_transfer_preserves_phase_and_unchanged_ramps_while_retargeting_changed_controls() {
    let initial = registry()
        .default_state(AUTO_PAN_TYPE_ID)
        .expect("defaults");
    let old = runtime(&initial, 8_000);
    let edit = patch(&initial, parameter::AMOUNT, 1.0);
    let mut target = runtime(&edit, 8_000);
    target.copy_history_from(old.as_ref());
    let old_pan = old
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("old runtime");
    let target_pan = target
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("new runtime");
    assert_eq!(target_pan.dsp.amount.remaining, 40);

    let mut first = [[1.0, 1.0]; 10];
    target.process_block(&mut first, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let in_flight = target
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert_eq!(in_flight.dsp.amount.remaining, 30);
    let transferred = runtime(&edit, 8_000);
    let mut same = transferred;
    same.copy_history_from(target.as_ref());
    let same_state = same
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert_eq!(same_state.dsp.phase_cycles, in_flight.dsp.phase_cycles);
    assert_eq!(same_state.dsp.amount.current, in_flight.dsp.amount.current);
    assert_eq!(same_state.dsp.amount.step, in_flight.dsp.amount.step);
    assert_eq!(
        same_state.dsp.amount.remaining,
        in_flight.dsp.amount.remaining
    );

    let second_edit = patch(&edit, parameter::RATE, 2.0);
    let mut retargeted = runtime(&second_edit, 8_000);
    retargeted.copy_history_from(target.as_ref());
    let retargeted = retargeted
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert_eq!(
        retargeted.dsp.amount.remaining,
        in_flight.dsp.amount.remaining
    );
    assert_eq!(retargeted.dsp.amount.step, in_flight.dsp.amount.step);
    assert_eq!(retargeted.dsp.rate_hz.remaining, 40);
    assert_ne!(retargeted.dsp.phase_cycles, old_pan.dsp.phase_cycles);

    let phase_tie = shortest_degrees(0.0, 180.0);
    assert_eq!(phase_tie, 180.0);
    assert_eq!(shortest_degrees(350.0, 10.0), 20.0);
    assert_eq!(shortest_degrees(10.0, 350.0), -20.0);
}

#[test]
fn edits_reach_targets_on_exactly_five_milliseconds_and_phase_retargets_by_shortest_arc() {
    let defaults = registry()
        .default_state(AUTO_PAN_TYPE_ID)
        .expect("defaults");
    let amount_one = patch(&defaults, parameter::AMOUNT, 1.0);
    let source = runtime(&defaults, 8_000);
    let mut edited = runtime(&amount_one, 8_000);
    edited.copy_history_from(source.as_ref());

    let mut first_ramp_frames = [[1.0, 1.0]; 39];
    edited.process_block(&mut first_ramp_frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let ramp = edited
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert_eq!(ramp.dsp.amount.remaining, 1);
    assert!((ramp.dsp.amount.current - (0.5 + 0.5 * 39.0 / 40.0)).abs() < 1.0e-14);
    let mut final_ramp_frame = [[1.0, 1.0]];
    edited.process_block(&mut final_ramp_frame, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let ramp = edited
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert_eq!(ramp.dsp.amount.current, 1.0);
    assert_eq!(ramp.dsp.amount.remaining, 0);
    let mut held_frame = [[1.0, 1.0]];
    edited.process_block(&mut held_frame, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(
        edited
            .as_any()
            .downcast_ref::<AutoPanRuntime>()
            .expect("runtime")
            .dsp
            .amount
            .current,
        1.0
    );

    let rate_two = patch(&defaults, parameter::RATE, 2.0);
    let rate_source = runtime(&defaults, 8_000);
    let mut rate_edit = runtime(&rate_two, 8_000);
    rate_edit.copy_history_from(rate_source.as_ref());
    let mut rate_ramp = [[1.0, 1.0]; 40];
    rate_edit.process_block(&mut rate_ramp, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let rate_state = rate_edit
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert_eq!(rate_state.dsp.rate_hz.current, 2.0);
    assert_eq!(rate_state.dsp.rate_hz.remaining, 0);
    assert!((rate_state.dsp.phase_cycles - 60.5 / 8_000.0).abs() < 1.0e-14);
    let phase = rate_state.dsp.phase_cycles;
    let mut after_rate_ramp = [[1.0, 1.0]];
    rate_edit.process_block(&mut after_rate_ramp, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let expected_left = (0.5 + 0.5 * (1.0 + (std::f64::consts::TAU * phase).sin()) * 0.5) as f32;
    assert_eq!(after_rate_ramp[0][0], expected_left);

    let phase_zero = patch(&defaults, parameter::RIGHT_PHASE, 0.0);
    let mut phase_edit = runtime(&phase_zero, 8_000);
    let phase_source = runtime(&defaults, 8_000);
    phase_edit.copy_history_from(phase_source.as_ref());
    let mut first = [[1.0, 1.0]; 20];
    phase_edit.process_block(&mut first, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let phase = phase_edit
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert!((phase.dsp.right_phase_degrees.current - 270.0).abs() < 1.0e-12);
    assert_eq!(phase.dsp.right_phase_degrees.remaining, 20);
    let phase_at_sample_20 = 19.0 / 8_000.0;
    let expected_lfo =
        (1.0 + (std::f64::consts::TAU * (phase_at_sample_20 + 270.0 / 360.0)).sin()) * 0.5;
    let expected_right = (0.5 + 0.5 * expected_lfo) as f32;
    assert_eq!(first[19][1], expected_right);

    let phase_ninety = patch(&phase_zero, parameter::RIGHT_PHASE, 90.0);
    let mut retargeted = runtime(&phase_ninety, 8_000);
    retargeted.copy_history_from(phase_edit.as_ref());
    let mut retarget_first = [[1.0, 1.0]];
    retargeted.process_block(&mut retarget_first, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let retargeted = retargeted
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert!((retargeted.dsp.right_phase_degrees.current - 274.5).abs() < 1.0e-12);
    assert_eq!(retargeted.dsp.right_phase_degrees.remaining, 39);

    // 360 and 0 are equivalent targets, so an unrelated in-flight phase ramp
    // must be inherited unchanged instead of restarted.
    let phase_360 = patch(&phase_zero, parameter::RIGHT_PHASE, 360.0);
    let mut equivalent = runtime(&phase_360, 8_000);
    equivalent.copy_history_from(phase_edit.as_ref());
    let equivalent = equivalent
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    let in_flight = phase_edit
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert_eq!(
        equivalent.dsp.right_phase_degrees.current,
        in_flight.dsp.right_phase_degrees.current
    );
    assert_eq!(
        equivalent.dsp.right_phase_degrees.step,
        in_flight.dsp.right_phase_degrees.step
    );
    assert_eq!(
        equivalent.dsp.right_phase_degrees.remaining,
        in_flight.dsp.right_phase_degrees.remaining
    );
}

#[test]
fn reset_clears_phase_and_ramps_and_360_degrees_matches_zero() {
    let defaults = registry()
        .default_state(AUTO_PAN_TYPE_ID)
        .expect("defaults");
    let mut phase_zero = runtime(&patch(&defaults, parameter::RIGHT_PHASE, 0.0), 8_000);
    let mut phase_full = runtime(&patch(&defaults, parameter::RIGHT_PHASE, 360.0), 8_000);
    let mut a = [[1.0, 1.0]; 17];
    let mut b = a;
    phase_zero.process_block(&mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    phase_full.process_block(&mut b, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(a, b);

    let changed = patch(&defaults, parameter::AMOUNT, 1.0);
    let mut source = runtime(&defaults, 8_000);
    let mut candidate = runtime(&changed, 8_000);
    candidate.copy_history_from(source.as_ref());
    candidate.reset();
    let candidate = candidate
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert_eq!(candidate.dsp.phase_cycles, 0.0);
    assert_eq!(candidate.dsp.amount.current, 1.0);
    assert_eq!(candidate.dsp.amount.target, 1.0);
    assert_eq!(candidate.dsp.amount.remaining, 0);
    source.reset();
}

#[test]
fn stateful_auto_pan_declares_zero_latency_and_tail_and_is_quiescent() {
    let state = registry()
        .default_state(AUTO_PAN_TYPE_ID)
        .expect("defaults");
    let processor = prepared(&state, 48_000);
    assert_eq!(processor.algorithmic_latency_frames(), Some(0));
    assert_eq!(
        processor.output_tail(),
        OutputTail::Finite { max_tail_frames: 0 }
    );
    let runtime = processor.create_runtime();
    assert!(runtime.is_quiescent());
    assert_eq!(
        processor.declared_runtime_bytes(),
        std::mem::size_of::<AutoPanRuntime>()
    );
}

#[test]
fn original_sine_pcm_fingerprint() {
    for (rate, expected) in [
        (8000, 0xedd6697d940d64a7_u64),
        (22050, 0x590f0440b4abe9a6_u64),
        (44100, 0xde98d6caef67dc78_u64),
        (48000, 0xd5118d20611337d6_u64),
        (96000, 0xd98e1558236b1ca7_u64),
        (192000, 0x898e485e23dd7ec4_u64),
    ] {
        let state = AUTO_PAN_DEFINITION.default_state().unwrap();
        let mut frames: Vec<_> = (0..2049)
            .map(|n| {
                let x = ((n * 97 % 521) as f32 - 260.0) / 270.0;
                [x, x * -0.173 + (n % 7) as f32 * 0.013]
            })
            .collect();
        runtime(&state, rate).process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        let mut hash = 0xcbf29ce484222325_u64;
        for value in frames.iter().flatten() {
            for byte in value.to_bits().to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
        assert_eq!(hash, expected, "pre-waveform Sine PCM {rate}");
    }
}

#[test]
fn selectable_square_changes_actual_pcm_at_quarter_cycle() {
    let state = patch(
        &AUTO_PAN_DEFINITION.default_state().unwrap(),
        parameter::AMOUNT,
        1.0,
    );
    let square = AUTO_PAN_DEFINITION
        .patch_parameter(
            &state,
            ParameterId::from_static(4),
            ParameterValue::Choice(4),
        )
        .expect("actual waveform choice");
    let mut signal = [[1.0, -0.25]; 2001];
    runtime(&square, 8000).process_block(&mut signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(signal[2000], [1.0, -0.0]);
}

fn choice(state: &NativeDeviceState, shape: u32) -> NativeDeviceState {
    AUTO_PAN_DEFINITION
        .patch_parameter(state, parameter::WAVEFORM, ParameterValue::Choice(shape))
        .unwrap()
}

#[test]
fn independent_quarter_cycle_channel_oracles_and_zero_amount_cover_all_shapes() {
    let levels = [
        [0.5, 1.0, 0.5, 0.0],
        [0.5, 1.0, 0.5, 0.0],
        [0.0, 0.25, 0.5, 0.75],
        [1.0, 0.75, 0.5, 0.25],
        [1.0, 1.0, 0.0, 0.0],
    ];
    for (shape, expected) in levels.iter().enumerate() {
        for (degrees, offset) in [(0.0, 0), (180.0, 2), (360.0, 0)] {
            for amount in [0.0_f32, 0.3, 1.0] {
                let state = patch(
                    &patch(
                        &choice(&AUTO_PAN_DEFINITION.default_state().unwrap(), shape as u32),
                        parameter::AMOUNT,
                        amount,
                    ),
                    parameter::RIGHT_PHASE,
                    degrees,
                );
                for quarter in 0..4 {
                    // Seed exact rational phases to test signed step boundaries
                    // independently of floating point clock accumulation.
                    let prepared = prepared(&state, 48000);
                    let boxed = prepared.create_runtime();
                    let initial = boxed.as_any().downcast_ref::<AutoPanRuntime>().unwrap();
                    let mut pan = copy_runtime(initial);
                    pan.dsp.phase_cycles = quarter as f64 / 4.0;
                    let input = [0.8, -0.25];
                    let mut sample = [input];
                    pan.process_block(&mut sample, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                    for (channel, lfo) in [expected[quarter], expected[(quarter + offset) % 4]]
                        .into_iter()
                        .enumerate()
                    {
                        let gain = 1.0 - f64::from(amount) + f64::from(amount) * lfo;
                        assert!(
                            (f64::from(sample[0][channel]) - f64::from(input[channel]) * gain)
                                .abs()
                                < 8.0e-8
                        );
                        if amount == 0.0 {
                            assert_eq!(sample[0][channel].to_bits(), input[channel].to_bits());
                        }
                    }
                }
            }
        }
    }
}
fn copy_runtime(p: &AutoPanRuntime) -> AutoPanRuntime {
    *p
}

#[test]
fn shape_changes_crossfade_independent_steady_streams_without_clock_reset_or_block_restart() {
    for rate in [8000_u32, 22050, 44100, 48000, 96000, 192000] {
        for (from, to) in [(0, 4), (4, 2), (2, 1), (1, 3), (3, 0)] {
            let base = patch(
                &AUTO_PAN_DEFINITION.default_state().unwrap(),
                parameter::RATE,
                20.0,
            );
            let old_state = choice(&base, from);
            let new_state = choice(&base, to);
            let source = runtime(&old_state, rate);
            let mut actual = runtime(&new_state, rate);
            actual.copy_history_from(source.as_ref());
            let count = rate.div_ceil(200) as usize;
            let input = [0.8, -0.21];
            let mut samples = vec![input; count + 17];
            let mut a = samples.clone();
            let mut b = samples.clone();
            runtime(&old_state, rate).process_block(&mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            runtime(&new_state, rate).process_block(&mut b, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            let mut start = 0;
            for length in [1, 7, 31, 2, 503].into_iter().cycle() {
                if start == samples.len() {
                    break;
                }
                let end = (start + length).min(samples.len());
                let mut twin = runtime(&new_state, rate);
                twin.copy_history_from(actual.as_ref());
                twin.process_block(
                    &mut samples[start..end],
                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                );
                actual = twin;
                start = end;
            }
            for (index, sample) in samples.iter().enumerate() {
                let t = ((index + 1) as f64 / count as f64).min(1.0);
                for (channel, value) in sample.iter().enumerate() {
                    let expected =
                        (1.0 - t) * f64::from(a[index][channel]) + t * f64::from(b[index][channel]);
                    assert!(
                        (f64::from(*value) - expected).abs() < 1.3e-7,
                        "rate{rate} pair{from}->{to} frame{index}"
                    );
                }
            }
            let state = actual.as_any().downcast_ref::<AutoPanRuntime>().unwrap();
            let expected_phase = ((count + 17) as f64 * 20.0 / f64::from(rate)).rem_euclid(1.0);
            assert!((state.dsp.phase_cycles - expected_phase).abs() < 1.0e-12);
            assert!(state.dsp.waveform_weights.iter().all(|w| w.remaining == 0));
            assert_eq!(
                source
                    .as_any()
                    .downcast_ref::<AutoPanRuntime>()
                    .unwrap()
                    .dsp
                    .phase_cycles,
                0.0,
                "speculative copies never mutate committed source"
            );
            let mut bypass = runtime(&new_state, rate);
            bypass.copy_history_from(source.as_ref());
            let mut dry = vec![input; count + 17];
            bypass.process_bypassed_block(&mut dry, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            assert!(dry.iter().all(|x| *x == input));
            let mut next = [input; 11];
            let mut other = next;
            bypass.process_block(&mut next, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            actual.process_block(&mut other, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            assert_eq!(next, other);
            actual.reset();
            let mut reset = [input; 11];
            let mut fresh = reset;
            actual.process_block(&mut reset, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            runtime(&new_state, rate)
                .process_block(&mut fresh, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            assert_eq!(reset, fresh);
        }
    }
}

#[test]
fn shape_codec_semantics_preview_and_rapid_retarget_remain_validated() {
    let base = AUTO_PAN_DEFINITION.default_state().unwrap();
    // v1 decodes (crate::state_migration_tests); its values are still validated.
    let legacy = NativeDeviceState::new(AUTO_PAN_TYPE_ID, 1, vec![0; 12]).unwrap();
    assert_eq!(
        AUTO_PAN_DEFINITION.migrate_state(&legacy),
        Err(DeviceContractError::InvalidParameterValue)
    );
    for value in [
        ParameterValue::Choice(5),
        ParameterValue::Choice(u32::MAX),
        ParameterValue::Float(f32::NAN),
        ParameterValue::Float(1.0),
        ParameterValue::Bool(true),
    ] {
        assert!(
            AUTO_PAN_DEFINITION
                .patch_parameter(&base, parameter::WAVEFORM, value)
                .is_err()
        );
    }
    let mut corrupt = base.payload().to_vec();
    corrupt[12..16].copy_from_slice(&5_u32.to_le_bytes());
    assert!(
        waveform_preview(
            &NativeDeviceState::new(AUTO_PAN_TYPE_ID, AUTO_PAN_STATE_VERSION, corrupt).unwrap()
        )
        .is_err()
    );
    for shape in 0..5 {
        let state = choice(&base, shape);
        assert_eq!(AUTO_PAN_DEFINITION.migrate_state(&state).unwrap(), state);
        assert_eq!(
            AUTO_PAN_DEFINITION
                .read_parameter(&state, parameter::WAVEFORM)
                .unwrap(),
            ParameterValue::Choice(shape)
        );
        let semantics = AUTO_PAN_DEFINITION
            .parameter_semantics_for_state(&state, parameter::WAVEFORM)
            .unwrap()
            .unwrap();
        assert_eq!(
            semantics.stepping,
            hex_lattice_contract::ParameterStepping::Choice
        );
        let preview = waveform_preview(&state).unwrap();
        for points in [preview.left, preview.right] {
            assert!(
                points
                    .iter()
                    .all(|[x, y]| (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y))
            );
        }
        assert_eq!(
            waveform_preview(&patch(&state, parameter::RATE, 20.0)).unwrap(),
            preview,
            "one normalized cycle is rate-independent"
        );
    }
    let old = runtime(&base, 8000);
    let square = choice(&base, 4);
    let triangle = choice(&base, 1);
    let mut change = runtime(&square, 8000);
    change.copy_history_from(old.as_ref());
    change.process_block(&mut [[1.0, 1.0]; 20], &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let before = change.as_any().downcast_ref::<AutoPanRuntime>().unwrap();
    let mut rapid = runtime(&triangle, 8000);
    rapid.copy_history_from(change.as_ref());
    let after = rapid.as_any().downcast_ref::<AutoPanRuntime>().unwrap();
    assert_eq!(after.dsp.phase_cycles, before.dsp.phase_cycles);
    assert_eq!(
        after.dsp.waveform_weights.map(|w| w.current),
        before.dsp.waveform_weights.map(|w| w.current)
    );
    rapid.process_block(&mut [[1.0, 1.0]; 40], &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let settled = rapid.as_any().downcast_ref::<AutoPanRuntime>().unwrap();
    assert_eq!(settled.dsp.waveform_weights[1].current, 1.0);
}

#[test]
fn intrinsic_edges_and_harmonic_content_are_explicit_not_alias_free_claims() {
    let e = 1.0e-9;
    assert_eq!(Waveform::Square.lfo(0.5 - e), 1.0);
    assert_eq!(Waveform::Square.lfo(0.5), 0.0);
    assert!(Waveform::SawUp.lfo(1.0 - e) > 0.999999);
    assert_eq!(Waveform::SawUp.lfo(0.0), 0.0);
    assert!((Waveform::Triangle.lfo(0.25 - e) - Waveform::Triangle.lfo(0.25 + e)).abs() < 1.0e-8);
    // Independent discrete Fourier measurement: intended square has harmonics;
    // triangle has substantially less third-harmonic energy, Sine virtually none.
    let energy = |shape: Waveform, k: f64| {
        let (mut re, mut im) = (0.0, 0.0);
        for n in 0..1024 {
            let phase = n as f64 / 1024.0;
            let y = shape.lfo(phase) - 0.5;
            let angle = std::f64::consts::TAU * k * phase;
            re += y * angle.cos();
            im += y * angle.sin();
        }
        re * re + im * im
    };
    let square = energy(Waveform::Square, 3.0) / energy(Waveform::Square, 1.0);
    let triangle = energy(Waveform::Triangle, 3.0) / energy(Waveform::Triangle, 1.0);
    assert!((square - 1.0 / 9.0).abs() < 1.0e-4);
    assert!((triangle - 1.0 / 81.0).abs() < 1.0e-4);
    assert!(energy(Waveform::Sine, 3.0) / energy(Waveform::Sine, 1.0) < 1.0e-20);
}

#[test]
fn wrapped_and_inflight_phase_retargets_follow_the_shortest_pcm_arc() {
    const RATE: u32 = 8_000;
    const RAMP_FRAMES: usize = 40; // Existing 5 ms transition at 8 kHz.
    let input = [0.8_f32, -0.25];
    let base = patch(
        &patch(
            &AUTO_PAN_DEFINITION.default_state().unwrap(),
            parameter::AMOUNT,
            1.0,
        ),
        parameter::RIGHT_PHASE,
        10.0,
    );
    let phase_350 = patch(&base, parameter::RIGHT_PHASE, 350.0);
    for (elapsed, next_degrees, normalized_start) in [(40, 0.0, 350.0), (30, 5.0, 355.0)] {
        let source = runtime(&base, RATE);
        let mut wrapped = runtime(&phase_350, RATE);
        wrapped.copy_history_from(source.as_ref());
        // 10 -> 350 takes the existing negative 20-degree shortest arc.
        // The second case interrupts that transition after 30 finite samples.
        wrapped.process_block(
            &mut vec![input; elapsed],
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        let next = patch(&base, parameter::RIGHT_PHASE, next_degrees);
        let mut edited = runtime(&next, RATE);
        edited.copy_history_from(wrapped.as_ref());
        let mut actual = [input; 49];
        let mut from = 0;
        for frames in [1, 7, 2, 13, 26] {
            let to = from + frames;
            edited.process_block(&mut actual[from..to], &hex_lattice::EMPTY_DEVICE_RESOURCES);
            from = to;
        }
        for (offset, frame) in actual.iter().enumerate() {
            // Both second edits move only +10 degrees across zero. Controls
            // advance before each sample; the oscillator is sampled before its
            // unchanged 1 Hz clock advances. After 40 samples the offset holds.
            let degrees = normalized_start
                + 10.0 * ((offset + 1).min(RAMP_FRAMES) as f64 / RAMP_FRAMES as f64);
            let oscillator = (elapsed + offset) as f64 / f64::from(RATE);
            for channel in 0..2 {
                let phase = if channel == 0 {
                    oscillator
                } else {
                    (oscillator + degrees / 360.0).rem_euclid(1.0)
                };
                let expected =
                    f64::from(input[channel]) * (1.0 + (std::f64::consts::TAU * phase).sin()) * 0.5;
                assert!(
                    (f64::from(frame[channel]) - expected).abs() < 8.0e-8,
                    "elapsed {elapsed}, sample {offset}, channel {channel}: {} != {expected}",
                    frame[channel]
                );
            }
        }
    }
}

/// Historical states keep opening (Pulse #3956). The v1 layout (12 bytes)
/// comes from commit 65e935227.
#[test]
fn auto_pan_v1_state_migrates_to_a_sine_waveform() {
    let payload = [0.75_f32, 2.0, 90.0]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let v1 = NativeDeviceState::new(AUTO_PAN_TYPE_ID, 1, payload).unwrap();
    let registry = crate::native_registry().unwrap();
    let migrated = registry.migrate_state(&v1).unwrap();
    assert_eq!(
        registry.migrate_state(&migrated).unwrap(),
        migrated,
        "idempotent"
    );
    assert_eq!(migrated.state_version(), AUTO_PAN_STATE_VERSION);
    let read = |id| AUTO_PAN_DEFINITION.read_parameter(&migrated, id).unwrap();
    assert_eq!(read(parameter::AMOUNT), ParameterValue::Float(0.75));
    assert_eq!(
        read(parameter::WAVEFORM),
        ParameterValue::Choice(0),
        "v1 was a sine LFO"
    );
    assert_eq!(
        AUTO_PAN_DEFINITION
            .migrate_state(&NativeDeviceState::new(AUTO_PAN_TYPE_ID, 1, vec![0; 16]).unwrap()),
        Err(DeviceContractError::MalformedState)
    );
    assert_eq!(
        AUTO_PAN_DEFINITION
            .migrate_state(&NativeDeviceState::new(AUTO_PAN_TYPE_ID, 3, vec![0; 16]).unwrap()),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
}

/// Pulse #3959: square and saw edges at full depth become a 1 ms linear
/// transition at every rate, so no sample steps the gain by more than the
/// transition slope (formerly a full 0 to 1 step every cycle).
#[test]
fn square_and_saw_edges_ramp_over_one_millisecond_at_full_depth() {
    for sample_rate in [44_100_u32, 192_000] {
        for shape in [2_u32, 3, 4] {
            for rate_hz in [1.0_f32, 20.0] {
                let state = patch(
                    &patch(
                        &choice(&AUTO_PAN_DEFINITION.default_state().unwrap(), shape),
                        parameter::AMOUNT,
                        1.0,
                    ),
                    parameter::RATE,
                    rate_hz,
                );
                let frames = (f64::from(sample_rate) / f64::from(rate_hz) * 1.2) as usize;
                let mut signal = vec![[1.0_f32, 1.0]; frames];
                runtime(&state, sample_rate)
                    .process_block(&mut signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
                let edge_slope = 1.0 / (EDGE_SECONDS * f64::from(sample_rate));
                let bound = edge_slope + f64::from(rate_hz) / f64::from(sample_rate) + 1.0e-6;
                for channel in 0..2 {
                    let largest = signal
                        .windows(2)
                        .map(|pair| {
                            (f64::from(pair[1][channel]) - f64::from(pair[0][channel])).abs()
                        })
                        .fold(0.0, f64::max);
                    let context = format!("rate={sample_rate} shape={shape} lfo={rate_hz}");
                    assert!(largest <= bound, "{context}: step {largest} > {bound}");
                    // The transition is about 1 ms, not a slower smear of the edge.
                    assert!(largest >= 0.9 * edge_slope, "{context}: step {largest}");
                    let (low, high) = signal.iter().fold((1.0_f32, 0.0_f32), |(l, h), f| {
                        (l.min(f[channel]), h.max(f[channel]))
                    });
                    // A saw turns where its edge transition starts, one edge width
                    // inside its full range; a square still reaches 0 and 1.
                    let inset = f64::from(rate_hz) * EDGE_SECONDS + 1.0e-3;
                    assert!(
                        f64::from(low) < inset && f64::from(high) > 1.0 - inset,
                        "{context}: depth {low}..{high}"
                    );
                }
            }
        }
    }
}
