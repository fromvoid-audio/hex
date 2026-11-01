use super::*;
use hex_lattice::{NativeDeviceRegistry, PreparedNativeProcessor};

static DEFINITIONS: [&dyn NativeDeviceDefinition; 1] = [&GATE_DEFINITION];

fn registry() -> NativeDeviceRegistry<'static> {
    NativeDeviceRegistry::new(&DEFINITIONS).expect("valid gate registry")
}

fn patch(state: &NativeDeviceState, id: ParameterId, value: f32) -> NativeDeviceState {
    GATE_DEFINITION
        .patch_parameter(state, id, ParameterValue::Float(value))
        .expect("valid gate parameter")
}

fn prepared(state: &NativeDeviceState, sample_rate: u32) -> Box<dyn PreparedNativeProcessor> {
    GATE_DEFINITION
        .prepare_processor(
            state,
            &hex_lattice::DevicePreparationContext {
                sample_rate,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("valid gate preparation")
        .expect("stateful gate")
}

fn runtime(state: &NativeDeviceState, sample_rate: u32) -> Box<dyn NativeProcessorRuntime> {
    prepared(state, sample_rate).create_runtime()
}

fn state_with(
    threshold: f32,
    return_level: f32,
    floor: f32,
    attack: f32,
    hold: f32,
    release: f32,
) -> NativeDeviceState {
    encode(GateSettings {
        threshold_db: threshold,
        return_db: return_level,
        floor_db: floor,
        attack_ms: attack,
        hold_ms: hold,
        release_ms: release,
    })
    .expect("valid settings")
}

fn db_gain(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

#[test]
fn defaults_and_generic_codec_roundtrip_six_controls() {
    let registry = registry();
    let state = registry.default_state(GATE_TYPE_ID).expect("gate defaults");
    assert_eq!(state.state_version(), 1);
    assert_eq!(state.payload().len(), STATE_BYTES);
    assert_eq!(registry.migrate_state(&state).expect("migration"), state);
    for (id, expected) in [
        (parameter::THRESHOLD, -40.0),
        (parameter::RETURN, -45.0),
        (parameter::FLOOR, -60.0),
        (parameter::ATTACK, 1.0),
        (parameter::HOLD, 50.0),
        (parameter::RELEASE, 100.0),
    ] {
        assert_eq!(
            GATE_DEFINITION.read_parameter(&state, id),
            Ok(ParameterValue::Float(expected))
        );
    }
    assert!(
        DESCRIPTORS
            .iter()
            .all(|descriptor| descriptor.validate_descriptor().is_ok())
    );
    assert_eq!(
        GATE_DEFINITION.prepare_matrix(&state),
        Err(DeviceContractError::UnsupportedProcessingClass)
    );
}

#[test]
fn malformed_state_parameter_cross_control_and_rate_values_are_rejected() {
    let state = registry().default_state(GATE_TYPE_ID).expect("defaults");
    for (id, value) in [
        (parameter::THRESHOLD, f32::NAN),
        (parameter::THRESHOLD, -72.1),
        (parameter::RETURN, 0.1),
        (parameter::FLOOR, -80.1),
        (parameter::ATTACK, 0.09),
        (parameter::HOLD, 2_000.1),
        (parameter::RELEASE, 0.99),
    ] {
        assert!(
            GATE_DEFINITION
                .patch_parameter(&state, id, ParameterValue::Float(value))
                .is_err()
        );
    }
    assert_eq!(
        GATE_DEFINITION
            .patch_parameter(&state, parameter::RETURN, ParameterValue::Float(-39.0),)
            .unwrap_err(),
        DeviceContractError::InvalidParameterValue
    );
    let wrong_type = NativeDeviceState::new(
        DeviceTypeId::from_static("hex.utility"),
        1,
        state.payload().to_vec(),
    )
    .expect("bounded wrong type");
    assert_eq!(
        GATE_DEFINITION.migrate_state(&wrong_type),
        Err(DeviceContractError::WrongDeviceType)
    );
    let future =
        NativeDeviceState::new(GATE_TYPE_ID, 2, state.payload().to_vec()).expect("future version");
    assert_eq!(
        GATE_DEFINITION.migrate_state(&future),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
    for length in [STATE_BYTES - 1, STATE_BYTES + 1] {
        let malformed = NativeDeviceState::new(GATE_TYPE_ID, 1, vec![0; length])
            .expect("bounded malformed state");
        assert_eq!(
            GATE_DEFINITION.migrate_state(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }
    let mut invalid_number = state.payload().to_vec();
    invalid_number[0..4].copy_from_slice(&f32::INFINITY.to_le_bytes());
    let malformed =
        NativeDeviceState::new(GATE_TYPE_ID, 1, invalid_number).expect("bounded non-finite state");
    assert_eq!(
        GATE_DEFINITION.migrate_state(&malformed),
        Err(DeviceContractError::InvalidParameterValue)
    );
    for sample_rate in [0, 7_999, 192_001] {
        assert!(
            GATE_DEFINITION
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
}

#[test]
fn linked_peak_threshold_hysteresis_and_exact_ramp_boundaries() {
    // Threshold=0 dBFS, return=-6 dBFS, floor=-20 dB. At 8 kHz,
    // attack=.25 ms is two frames, hold=5.25 ms (above the 5 ms minimum)
    // is 42 frames and release=1 ms is eight frames.
    let state = state_with(0.0, -6.0, -20.0, 0.25, 5.25, 1.0);
    let mut gate = runtime(&state, 8_000);
    let mut high = [[1.0, -0.25], [0.75, -0.1875]];
    gate.process_block(&mut high, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let start = db_gain(-20.0);
    let first_attack = start + (1.0 - start) / 2.0;
    assert!((f64::from(high[0][0]) - first_attack).abs() < 1.0e-7);
    assert!((f64::from(high[0][1]) + 0.25 * first_attack).abs() < 1.0e-7);
    assert_eq!(high[1], [0.75, -0.1875]);

    let mut middle = [[0.6, -0.1]]; // Above return, below threshold: remain open.
    gate.process_block(&mut middle, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(middle, [[0.6, -0.1]]);
    let mut below = [[0.4, -0.1]; 43];
    gate.process_block(&mut below, &hex_lattice::EMPTY_DEVICE_RESOURCES); // Hold 42 full qualifying frames, then close.
    assert!(below[..42].iter().all(|frame| *frame == [0.4, -0.1]));
    let releasing = 1.0 - (1.0 - start) / 8.0;
    assert!((f64::from(below[42][0]) - 0.4 * releasing).abs() < 1.0e-7);
    let mut release = [[0.4, -0.1]; 7];
    gate.process_block(&mut release, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    for (index, frame) in release.iter().enumerate() {
        let gain = if index == 6 {
            start
        } else {
            1.0 - (1.0 - start) * (index as f64 + 2.0) / 8.0
        };
        assert!((f64::from(frame[0]) - 0.4 * gain).abs() < 1.0e-7);
        assert!((f64::from(frame[1]) + 0.1 * gain).abs() < 1.0e-7);
    }
    let mut closed = [[0.4, -0.1]];
    gate.process_block(&mut closed, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!((f64::from(closed[0][0]) - 0.4 * start).abs() < 1.0e-7);
}

#[test]
fn threshold_equality_opens_hold_zero_closes_after_minimum_hold_and_reopen_attacks_from_release_gain()
 {
    let mut zero_hold = runtime(&state_with(0.0, 0.0, -20.0, 0.25, 0.0, 1.0), 8_000);
    let mut equality = [[1.0, -0.5], [1.0, -0.5]];
    zero_hold.process_block(&mut equality, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(f64::from(equality[0][0]) > 0.1); // threshold precedence opens at equality
    assert_eq!(equality[1], [1.0, -0.5]); // threshold precedence keeps equality open
    // Hold 0 acts as the 5 ms minimum hold (40 frames at 8 kHz, Pulse #4107).
    let mut low = [[0.4, -0.2]; 41];
    low[0] = [0.0, 0.0];
    zero_hold.process_block(&mut low, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(low[0], [0.0, 0.0]);
    assert!(low[1..40].iter().all(|frame| *frame == [0.4, -0.2]));
    assert!(f64::from(low[40][0]) < 0.4); // below-return after the hold begins release

    let mut reopening = runtime(&state_with(0.0, -6.0, -20.0, 0.5, 0.0, 1.0), 8_000);
    let mut open = [[1.0, 0.0]; 4];
    reopening.process_block(&mut open, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let mut closing = [[0.4, 0.0]; 41];
    reopening.process_block(&mut closing, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let before_reopen = f64::from(closing[40][0]) / 0.4;
    assert!(before_reopen < 1.0 && before_reopen > db_gain(-20.0));
    let mut reopen = [[1.0, 0.0]];
    reopening.process_block(&mut reopen, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let after_reopen = f64::from(reopen[0][0]);
    assert!(after_reopen > before_reopen);
    assert!(after_reopen < 1.0);
}

#[test]
fn same_settings_history_copy_is_partition_exact_and_changed_controls_retarget_only_their_stage() {
    let original = state_with(0.0, -6.0, -20.0, 0.5, 0.25, 1.0);
    let mut source = runtime(&original, 8_000);
    let mut priming = [[1.0, 0.0]; 4];
    source.process_block(&mut priming, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let old = source
        .as_any()
        .downcast_ref::<GateRuntime>()
        .unwrap()
        .history();
    assert!(old.ramp_remaining > 0 || old.open);

    let mut same = runtime(&original, 8_000);
    assert!(same.can_copy_history_from(source.as_ref()));
    same.copy_history_from(source.as_ref());
    assert_eq!(
        same.as_any()
            .downcast_ref::<GateRuntime>()
            .unwrap()
            .history(),
        old
    );

    let mut one_block = [[1.0, 0.0]; 37];
    let mut partitioned = one_block;
    source.process_block(&mut one_block, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    same.process_block(&mut partitioned[..3], &hex_lattice::EMPTY_DEVICE_RESOURCES);
    same.process_block(
        &mut partitioned[3..19],
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    same.process_block(&mut partitioned[19..], &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(one_block, partitioned);

    let mut closing_source = runtime(&state_with(0.0, -6.0, -20.0, 0.5, 0.0, 1.0), 8_000);
    let mut opened = [[1.0, 0.0]; 4];
    closing_source.process_block(&mut opened, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    // Hold 0 acts as the 5 ms minimum hold: 40 held frames, then release.
    let mut low = [[0.4, 0.0]; 41];
    closing_source.process_block(&mut low, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let closed_history = closing_source
        .as_any()
        .downcast_ref::<GateRuntime>()
        .unwrap()
        .history();
    assert!(closed_history.ramp_remaining > 0 && !closed_history.open);

    let base = state_with(0.0, -6.0, -20.0, 0.5, 0.0, 1.0);
    let floor_changed = patch(&base, parameter::FLOOR, -40.0);
    let mut floor_candidate = runtime(&floor_changed, 8_000);
    floor_candidate.copy_history_from(closing_source.as_ref());
    let floor_history = floor_candidate
        .as_any()
        .downcast_ref::<GateRuntime>()
        .unwrap()
        .history();
    assert_eq!(floor_history.gain, closed_history.gain);
    assert_eq!(floor_history.open, closed_history.open);
    assert_eq!(floor_history.ramp_target, db_gain(-40.0));
    assert_eq!(floor_history.ramp_remaining, 8);

    let threshold_changed = patch(&base, parameter::THRESHOLD, -3.0);
    let mut threshold_candidate = runtime(&threshold_changed, 8_000);
    threshold_candidate.copy_history_from(closing_source.as_ref());
    assert_eq!(
        threshold_candidate
            .as_any()
            .downcast_ref::<GateRuntime>()
            .unwrap()
            .history(),
        closed_history
    );

    let opening_state = state_with(0.0, -6.0, -20.0, 1.0, 0.5, 1.0);
    let mut opening = runtime(&opening_state, 8_000);
    let mut partial_attack = [[1.0, 0.0]; 3];
    opening.process_block(&mut partial_attack, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let before_attack_edit = opening
        .as_any()
        .downcast_ref::<GateRuntime>()
        .unwrap()
        .history();
    let attack_changed = patch(&opening_state, parameter::ATTACK, 2.0);
    let mut attack_candidate = runtime(&attack_changed, 8_000);
    attack_candidate.copy_history_from(opening.as_ref());
    let after_attack_edit = attack_candidate
        .as_any()
        .downcast_ref::<GateRuntime>()
        .unwrap()
        .history();
    assert_eq!(after_attack_edit.gain, before_attack_edit.gain);
    assert_eq!(after_attack_edit.ramp_target, 1.0);
    assert_eq!(after_attack_edit.ramp_remaining, 16);

    let hold_state = state_with(0.0, -6.0, -20.0, 0.5, 6.0, 1.0);
    let mut holding = runtime(&hold_state, 8_000);
    let mut first_low = [[1.0, 0.0], [0.4, 0.0]];
    holding.process_block(&mut first_low, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let before_hold_edit = holding
        .as_any()
        .downcast_ref::<GateRuntime>()
        .unwrap()
        .history();
    assert_eq!(before_hold_edit.hold_elapsed, 1);
    // Both holds are distinct above the 5 ms minimum.
    let hold_changed = patch(&hold_state, parameter::HOLD, 10.0);
    let mut hold_candidate = runtime(&hold_changed, 8_000);
    hold_candidate.copy_history_from(holding.as_ref());
    assert_eq!(
        hold_candidate
            .as_any()
            .downcast_ref::<GateRuntime>()
            .unwrap()
            .history(),
        before_hold_edit
    );

    let release_changed = patch(&base, parameter::RELEASE, 2.0);
    let mut release_candidate = runtime(&release_changed, 8_000);
    release_candidate.copy_history_from(closing_source.as_ref());
    assert_eq!(
        release_candidate
            .as_any()
            .downcast_ref::<GateRuntime>()
            .unwrap()
            .history()
            .ramp_remaining,
        16
    );

    let different_rate = runtime(&base, 44_100);
    assert!(!different_rate.can_copy_history_from(closing_source.as_ref()));
    assert!(!different_rate.can_retarget_parameters_from(closing_source.as_ref()));
}

#[test]
fn bypass_advances_history_bitwise_and_invalid_frames_do_not_poison_it() {
    let state = state_with(0.0, -6.0, -20.0, 0.5, 0.0, 1.0);
    let mut bypassed = runtime(&state, 8_000);
    let mut active = runtime(&state, 8_000);
    let mut prime = [[1.0, 0.0]; 4];
    bypassed.process_block(&mut prime, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let mut prime = [[1.0, 0.0]; 4];
    active.process_block(&mut prime, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let mut dry = [[f32::from_bits(0x3f123456), -0.25]];
    let original = dry;
    bypassed.process_bypassed_block(&mut dry, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(dry[0][0].to_bits(), original[0][0].to_bits());
    assert_eq!(dry[0][1].to_bits(), original[0][1].to_bits());
    let mut active_frame = dry;
    active.process_block(&mut active_frame, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(
        bypassed
            .as_any()
            .downcast_ref::<GateRuntime>()
            .unwrap()
            .history(),
        active
            .as_any()
            .downcast_ref::<GateRuntime>()
            .unwrap()
            .history()
    );

    let mut invalid_then_valid = runtime(&state, 8_000);
    let mut valid_only = runtime(&state, 8_000);
    let mut invalid = [[f32::NAN, f32::INFINITY]];
    invalid_then_valid.process_block(&mut invalid, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(invalid[0][0].is_nan());
    assert_eq!(invalid[0][1], f32::INFINITY);
    let mut a = [[0.4, -0.2]];
    let mut b = a;
    invalid_then_valid.process_block(&mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    valid_only.process_block(&mut b, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(a, b);
}

#[test]
fn floor_edit_when_settled_closed_ramps_and_runtime_has_no_output_tail() {
    let original = state_with(0.0, -6.0, -20.0, 0.5, 0.0, 1.0);
    let mut closed = runtime(&original, 8_000);
    let mut low = [[0.4, 0.0]; 8];
    closed.process_block(&mut low, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let floor_changed = patch(&original, parameter::FLOOR, -40.0);
    let mut candidate = runtime(&floor_changed, 8_000);
    candidate.copy_history_from(closed.as_ref());
    let history = candidate
        .as_any()
        .downcast_ref::<GateRuntime>()
        .unwrap()
        .history();
    assert_eq!(history.ramp_target, db_gain(-40.0));
    assert_eq!(history.ramp_remaining, 8);
    assert!(candidate.is_quiescent());
    assert_eq!(
        prepared(&floor_changed, 8_000).algorithmic_latency_frames(),
        Some(0)
    );
    assert_eq!(
        prepared(&floor_changed, 8_000).output_tail(),
        OutputTail::Finite { max_tail_frames: 0 }
    );
    let mut silence = [[0.0, -0.0]; 16];
    candidate.process_block(&mut silence, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(
        silence
            .iter()
            .all(|frame| frame[0] == 0.0 && frame[1] == 0.0)
    );

    let mut reset = runtime(&original, 8_000);
    let mut fresh = runtime(&original, 8_000);
    let mut signal = [[1.0, 0.0]; 3];
    reset.process_block(&mut signal, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    reset.reset();
    let mut after_reset = [[0.4, 0.0]];
    let mut after_fresh = after_reset;
    reset.process_block(&mut after_reset, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    fresh.process_block(&mut after_fresh, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(after_reset, after_fresh);
}

/// Pulse #4107: with hold 0 the instantaneous detector dipped below return at
/// every zero crossing of a low tone and the gate closed there (chatter). The
/// minimum internal hold keeps a 40 Hz sine at -6 dBFS fully open.
#[test]
fn hold_zero_rides_through_low_frequency_zero_crossings() {
    for (threshold, return_level) in [(-40.0, -45.0), (-6.2, -11.2)] {
        let state = state_with(threshold, return_level, -60.0, 1.0, 0.0, 1.0);
        for sample_rate in [8_000_u32, 44_100, 48_000, 192_000] {
            let amplitude = db_gain(-6.0);
            let mut frames: Vec<StereoFrame> = (0..sample_rate as usize)
                .map(|frame| {
                    let phase =
                        std::f64::consts::TAU * 40.0 * frame as f64 / f64::from(sample_rate);
                    let value = (amplitude * phase.sin()) as f32;
                    [value, -value]
                })
                .collect();
            let input = frames.clone();
            runtime(&state, sample_rate)
                .process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            // Allow the first cycle to cross threshold and complete the attack.
            let settled = sample_rate as usize / 40;
            for (index, (out, original)) in frames.iter().zip(&input).enumerate().skip(settled) {
                assert_eq!(
                    out, original,
                    "threshold={threshold} rate={sample_rate} frame={index}: gate moved"
                );
            }
        }
    }
}
