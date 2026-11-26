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
        ..GateSettings::default()
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
    assert_eq!(state.state_version(), GATE_STATE_VERSION);
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
    let future = NativeDeviceState::new(
        GATE_TYPE_ID,
        GATE_STATE_VERSION + 1,
        state.payload().to_vec(),
    )
    .expect("future version");
    assert_eq!(
        GATE_DEFINITION.migrate_state(&future),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
    for length in [STATE_BYTES - 1, STATE_BYTES + 1] {
        let malformed = NativeDeviceState::new(GATE_TYPE_ID, GATE_STATE_VERSION, vec![0; length])
            .expect("bounded malformed state");
        assert_eq!(
            GATE_DEFINITION.migrate_state(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }
    let mut invalid_number = state.payload().to_vec();
    invalid_number[0..4].copy_from_slice(&f32::INFINITY.to_le_bytes());
    let malformed = NativeDeviceState::new(GATE_TYPE_ID, GATE_STATE_VERSION, invalid_number)
        .expect("bounded non-finite state");
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
    assert!(old.gain_ramp.is_active() || old.open);

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
    assert!(closed_history.gain_ramp.is_active() && !closed_history.open);

    let base = state_with(0.0, -6.0, -20.0, 0.5, 0.0, 1.0);
    let floor_changed = patch(&base, parameter::FLOOR, -40.0);
    let mut floor_candidate = runtime(&floor_changed, 8_000);
    floor_candidate.copy_history_from(closing_source.as_ref());
    let floor_history = floor_candidate
        .as_any()
        .downcast_ref::<GateRuntime>()
        .unwrap()
        .history();
    assert_eq!(
        floor_history.gain_ramp.value(),
        closed_history.gain_ramp.value()
    );
    assert_eq!(floor_history.open, closed_history.open);
    assert_eq!(floor_history.gain_ramp.target(), db_gain(-40.0));
    assert_eq!(floor_history.gain_ramp.remaining(), 8);

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
    assert_eq!(
        after_attack_edit.gain_ramp.value(),
        before_attack_edit.gain_ramp.value()
    );
    assert_eq!(after_attack_edit.gain_ramp.target(), 1.0);
    assert_eq!(after_attack_edit.gain_ramp.remaining(), 16);

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
            .gain_ramp
            .remaining(),
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
    let mut silence = [[0.0, 0.0]];
    invalid_then_valid.process_block(&mut invalid, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    valid_only.process_block(&mut silence, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(invalid, [[0.0, 0.0]]);
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
    assert_eq!(history.gain_ramp.target(), db_gain(-40.0));
    assert_eq!(history.gain_ramp.remaining(), 8);
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

#[test]
fn v1_state_migrates_losslessly_to_v2_with_bit_exact_pcm() {
    let mut v1_bytes = Vec::with_capacity(24);
    for value in [-30.0_f32, -36.0, -24.0, 2.0, 20.0, 50.0] {
        v1_bytes.extend_from_slice(&value.to_le_bytes());
    }
    let v1 = NativeDeviceState::new(GATE_TYPE_ID, 1, v1_bytes).unwrap();
    let migrated = GATE_DEFINITION.migrate_state(&v1).unwrap();
    assert_eq!(migrated.state_version(), GATE_STATE_VERSION);
    assert_eq!(migrated.payload().len(), STATE_BYTES);
    assert_eq!(
        GATE_DEFINITION.read_parameter(&migrated, parameter::FLIP),
        Ok(ParameterValue::Bool(false))
    );
    assert_eq!(
        GATE_DEFINITION.read_parameter(&migrated, parameter::LOOKAHEAD),
        Ok(ParameterValue::Choice(0))
    );
    assert_eq!(
        GATE_DEFINITION.read_parameter(&migrated, parameter::SIDECHAIN_GAIN),
        Ok(ParameterValue::Float(0.0))
    );
    assert_eq!(
        GATE_DEFINITION.read_parameter(&migrated, parameter::SIDECHAIN_MIX),
        Ok(ParameterValue::Float(1.0))
    );
    assert_eq!(
        GATE_DEFINITION.read_parameter(&migrated, parameter::SIDECHAIN_EQ_MODE),
        Ok(ParameterValue::Choice(0))
    );
    let mut a = [[0.5, -0.25]; 256];
    let mut b = a;
    runtime(&v1, 48_000).process_block(&mut a, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    runtime(&migrated, 48_000).process_block(&mut b, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(a, b);
}

#[test]
fn flip_mode_inverts_gate_into_ducker_and_reports_dynamics_meter() {
    let state = GATE_DEFINITION
        .patch_parameter(
            &state_with(-12.0, -18.0, -20.0, 0.25, 5.0, 1.0),
            parameter::FLIP,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let mut gate = runtime(&state, 8_000);
    // Below threshold: flipped gate passes audio at unity (0 dB reduction).
    let mut quiet = [[0.1, -0.1]; 8];
    gate.process_block(&mut quiet, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(quiet[7], [0.1, -0.1]);
    let quiet_meter = gate.take_dynamics_meter().unwrap();
    assert!(quiet_meter.reduction_db[0] < 0.01);
    assert!((quiet_meter.post_input_peak - 0.1).abs() < 1e-5);

    // Above threshold: flipped gate ducks down to floor (-20 dB = 0.1 linear).
    let mut loud = [[0.5, -0.5]; 16];
    gate.process_block(&mut loud, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    let floor = db_gain(-20.0) as f32;
    assert!((loud[15][0] - 0.5 * floor).abs() < 1e-5);
    let loud_meter = gate.take_dynamics_meter().unwrap();
    assert!((loud_meter.reduction_db[0] - 20.0).abs() < 0.05);
}

#[test]
fn lookahead_declares_exact_latency_anticipates_opening_and_aligns_bypass() {
    let base = state_with(-12.0, -18.0, -60.0, 1.0, 10.0, 10.0);
    let la_1_5 = GATE_DEFINITION
        .patch_parameter(&base, parameter::LOOKAHEAD, ParameterValue::Choice(1))
        .unwrap();
    let la_10 = GATE_DEFINITION
        .patch_parameter(&base, parameter::LOOKAHEAD, ParameterValue::Choice(2))
        .unwrap();
    assert_eq!(
        prepared(&base, 48_000).algorithmic_latency_frames(),
        Some(0)
    );
    assert_eq!(
        prepared(&la_1_5, 48_000).algorithmic_latency_frames(),
        Some(72)
    );
    assert_eq!(
        prepared(&la_10, 48_000).algorithmic_latency_frames(),
        Some(480)
    );
    assert_eq!(
        prepared(&la_10, 192_000).algorithmic_latency_frames(),
        Some(1920)
    );
    assert_eq!(
        prepared(&la_10, 48_000).output_tail(),
        OutputTail::Finite {
            max_tail_frames: 480
        }
    );

    // 10 ms lookahead opens the gate BEFORE the delayed transient arrives -> full leading edge!
    let mut no_la = runtime(&base, 48_000);
    let mut with_la = runtime(&la_10, 48_000);
    let mut out_no_la = vec![[0.0; 2]; 1024];
    for frame in &mut out_no_la[100..] {
        *frame = [0.8, 0.8];
    }
    let mut out_la = out_no_la.clone();
    no_la.process_block(&mut out_no_la, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    with_la.process_block(&mut out_la, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert!(!with_la.is_quiescent());
    // Without lookahead, frame 100 is still ramping up from floor.
    assert!(out_no_la[100][0] < 0.1);
    // With 10 ms lookahead (480 frames), frame 100 + 480 = 580 arrives with gate already wide open!
    assert_eq!(out_la[579], [0.0, 0.0]);
    assert!((out_la[580][0] - 0.8).abs() < 1e-5);

    // Bypass preserves exact lookahead delay alignment.
    let mut bypassed = runtime(&la_1_5, 48_000);
    let mut impulse = vec![[0.0; 2]; 128];
    impulse[10] = [0.5, -0.25];
    bypassed.process_bypassed_block(&mut impulse, &hex_lattice::EMPTY_DEVICE_RESOURCES);
    assert_eq!(impulse[10], [0.0, 0.0]);
    assert_eq!(impulse[82], [0.5, -0.25]);

    assert!(with_la.can_copy_history_from(runtime(&la_10, 48_000).as_ref()));
    assert!(!with_la.can_copy_history_from(no_la.as_ref()));
}

#[test]
fn sidechain_auxiliary_gain_mix_and_eq_filter_drive_gate_detector() {
    assert_eq!(GATE_DEFINITION.auxiliary_input(), AuxiliaryInput::Stereo);
    let base = state_with(-12.0, -18.0, -40.0, 0.25, 5.0, 5.0);

    // 1. External loud key opens gate for a quiet main signal; sidechain mix = 0 keeps it closed.
    let sc_internal = patch(&base, parameter::SIDECHAIN_MIX, 0.0);
    let mut rt_ext = runtime(&base, 48_000);
    let mut rt_int = runtime(&sc_internal, 48_000);
    let mut main_ext = [[0.05; 2]; 512];
    let mut main_int = main_ext;
    let key = [[0.8; 2]; 512];
    let mut scratch = [];
    rt_ext
        .process_block_with_auxiliary(
            StereoAuxiliaryBlock::new(&mut main_ext, Some(&key), &mut scratch, 512).unwrap(),
            None,
            None,
            false,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    rt_int
        .process_block_with_auxiliary(
            StereoAuxiliaryBlock::new(&mut main_int, Some(&key), &mut scratch, 512).unwrap(),
            None,
            None,
            false,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    assert!((main_ext[511][0] - 0.05).abs() < 1e-5);
    assert!(main_int[511][0] < 0.001);

    // 2. Sidechain HP filter at 1000 Hz prevents a 60 Hz sub sine key from opening the gate.
    let mut sc_hp = GATE_DEFINITION
        .patch_parameter(
            &base,
            parameter::SIDECHAIN_EQ_MODE,
            ParameterValue::Choice(1),
        )
        .unwrap();
    sc_hp = patch(&sc_hp, parameter::SIDECHAIN_EQ_FREQ, 1_000.0);
    let mut rt_no_eq = runtime(&base, 48_000);
    let mut rt_hp = runtime(&sc_hp, 48_000);
    let sub_sine: Vec<StereoFrame> = (0..4096)
        .map(|i| {
            let s = (2.0 * std::f32::consts::PI * 60.0 * i as f32 / 48_000.0).sin() * 0.8;
            [s, s]
        })
        .collect();
    let mut out_no_eq = vec![[0.05; 2]; 4096];
    let mut out_hp = out_no_eq.clone();
    rt_no_eq
        .process_block_with_auxiliary(
            StereoAuxiliaryBlock::new(&mut out_no_eq, Some(&sub_sine), &mut scratch, 4096).unwrap(),
            None,
            None,
            false,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    rt_hp
        .process_block_with_auxiliary(
            StereoAuxiliaryBlock::new(&mut out_hp, Some(&sub_sine), &mut scratch, 4096).unwrap(),
            None,
            None,
            false,
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    assert!((out_no_eq[4095][0] - 0.05).abs() < 1e-5);
    assert!(out_hp[4095][0] < 0.001);
}
