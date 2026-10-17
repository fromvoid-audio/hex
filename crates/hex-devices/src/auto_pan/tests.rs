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
fn defaults_and_generic_codec_roundtrip_three_controls() {
    let registry = registry();
    let state = registry.default_state(AUTO_PAN_TYPE_ID).expect("defaults");
    assert_eq!(state.state_version(), 1);
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
        let malformed = NativeDeviceState::new(AUTO_PAN_TYPE_ID, 1, vec![0; length])
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
    let future = NativeDeviceState::new(AUTO_PAN_TYPE_ID, 2, state.payload().to_vec())
        .expect("future version");
    assert_eq!(
        AUTO_PAN_DEFINITION.migrate_state(&future),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
    let mut nonfinite = state.payload().to_vec();
    nonfinite[4..8].copy_from_slice(&f32::NAN.to_le_bytes());
    let nonfinite =
        NativeDeviceState::new(AUTO_PAN_TYPE_ID, 1, nonfinite).expect("bounded nonfinite state");
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
    assert_eq!(target_pan.amount.remaining, 40);

    let mut first = [[1.0, 1.0]; 10];
    target.process_block(&mut first, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let in_flight = target
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert_eq!(in_flight.amount.remaining, 30);
    let transferred = runtime(&edit, 8_000);
    let mut same = transferred;
    same.copy_history_from(target.as_ref());
    let same_state = same
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert_eq!(same_state.phase_cycles, in_flight.phase_cycles);
    assert_eq!(same_state.amount.current, in_flight.amount.current);
    assert_eq!(same_state.amount.step, in_flight.amount.step);
    assert_eq!(same_state.amount.remaining, in_flight.amount.remaining);

    let second_edit = patch(&edit, parameter::RATE, 2.0);
    let mut retargeted = runtime(&second_edit, 8_000);
    retargeted.copy_history_from(target.as_ref());
    let retargeted = retargeted
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert_eq!(retargeted.amount.remaining, in_flight.amount.remaining);
    assert_eq!(retargeted.amount.step, in_flight.amount.step);
    assert_eq!(retargeted.rate_hz.remaining, 40);
    assert_ne!(retargeted.phase_cycles, old_pan.phase_cycles);

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
    assert_eq!(ramp.amount.remaining, 1);
    assert!((ramp.amount.current - (0.5 + 0.5 * 39.0 / 40.0)).abs() < 1.0e-14);
    let mut final_ramp_frame = [[1.0, 1.0]];
    edited.process_block(&mut final_ramp_frame, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let ramp = edited
        .as_any()
        .downcast_ref::<AutoPanRuntime>()
        .expect("runtime");
    assert_eq!(ramp.amount.current, 1.0);
    assert_eq!(ramp.amount.remaining, 0);
    let mut held_frame = [[1.0, 1.0]];
    edited.process_block(&mut held_frame, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(
        edited
            .as_any()
            .downcast_ref::<AutoPanRuntime>()
            .expect("runtime")
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
    assert_eq!(rate_state.rate_hz.current, 2.0);
    assert_eq!(rate_state.rate_hz.remaining, 0);
    assert!((rate_state.phase_cycles - 60.5 / 8_000.0).abs() < 1.0e-14);
    let phase = rate_state.phase_cycles;
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
    assert!((phase.right_phase_degrees.current - 270.0).abs() < 1.0e-12);
    assert_eq!(phase.right_phase_degrees.remaining, 20);
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
    assert!((retargeted.right_phase_degrees.current - 274.5).abs() < 1.0e-12);
    assert_eq!(retargeted.right_phase_degrees.remaining, 39);

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
        equivalent.right_phase_degrees.current,
        in_flight.right_phase_degrees.current
    );
    assert_eq!(
        equivalent.right_phase_degrees.step,
        in_flight.right_phase_degrees.step
    );
    assert_eq!(
        equivalent.right_phase_degrees.remaining,
        in_flight.right_phase_degrees.remaining
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
    assert_eq!(candidate.phase_cycles, 0.0);
    assert_eq!(candidate.amount.current, 1.0);
    assert_eq!(candidate.amount.target, 1.0);
    assert_eq!(candidate.amount.remaining, 0);
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
