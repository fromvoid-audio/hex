use super::*;
use hex_lattice::{EMPTY_DEVICE_RESOURCES, NativeParameterAutomation};

struct Values {
    ids: Vec<ParameterId>,
    values: Vec<Option<ParameterValue>>,
}
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        &self.ids
    }
    fn value_at(&self, parameter: ParameterId, _offset: usize) -> Option<ParameterValue> {
        self.ids
            .iter()
            .position(|id| *id == parameter)
            .and_then(|i| self.values[i])
    }
}
fn factory(state: &NativeDeviceState, ids: &[ParameterId]) -> PreparedAutoPan {
    AutoPanDefinition::prepare_owned(state, 8_000, ids).unwrap()
}
fn process(runtime: &mut dyn NativeProcessorRuntime, frames: &mut [StereoFrame], values: &Values) {
    runtime
        .process_block_with_automation(frames, None, values, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
}
fn close(actual: &[[f32; 2]], expected: &[[f32; 2]]) {
    for (i, (a, b)) in actual.iter().zip(expected).enumerate() {
        for channel in 0..2 {
            assert!(
                (a[channel] - b[channel]).abs() < 1e-7,
                "frame {i}: {a:?} != {b:?}"
            );
        }
    }
}

#[test]
fn all_four_targets_follow_independent_five_ms_pcm_law_across_partitions_and_bypass() {
    let base = AUTO_PAN_DEFINITION.default_state().unwrap();
    let ids = vec![
        parameter::AMOUNT,
        parameter::RATE,
        parameter::RIGHT_PHASE,
        parameter::WAVEFORM,
    ];
    let values = Values {
        ids: ids.clone(),
        values: vec![
            Some(ParameterValue::Float(1.0)),
            Some(ParameterValue::Float(2.0)),
            Some(ParameterValue::Float(0.0)),
            Some(ParameterValue::Choice(4)),
        ],
    };
    let prepared = factory(&base, &ids);
    let mut runtime = prepared.create_runtime();
    let mut actual = [[1.0, -0.5]; 97];
    let mut from = 0;
    for size in [1, 7, 31, 2, 56] {
        process(runtime.as_mut(), &mut actual[from..from + size], &values);
        from += size;
    }
    // Independent original law: transitions advance before sample; oscillator
    // advances afterwards. Held target requests must not restart these ramps.
    let mut phase: f64 = 0.0;
    let expected: Vec<_> = (0..97)
        .map(|frame| {
            let w = ((frame + 1) as f64 / 40.0).min(1.0);
            let amount = 0.5 + 0.5 * w;
            let phase_right = (phase + (180.0 + 180.0 * w) / 360.0).rem_euclid(1.0);
            let result = [phase, phase_right].map(|p| {
                let sine = (1.0 + (std::f64::consts::TAU * p).sin()) * 0.5;
                let square = if p < 0.5 { 1.0 } else { 0.0 };
                (1.0 - amount + amount * ((1.0 - w) * sine + w * square)) as f32
            });
            phase = (phase + (1.0 + w) / 8000.0).rem_euclid(1.0);
            [result[0], -0.5 * result[1]]
        })
        .collect();
    close(&actual, &expected);
    let mut bypassed = prepared.create_runtime();
    let mut dry = [[1.0, -0.5]; 47];
    bypassed
        .process_bypassed_block_with_automation(&mut dry, None, &values, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    assert_eq!(dry, [[1.0, -0.5]; 47]);
    let mut rest = [[1.0, -0.5]; 50];
    process(bypassed.as_mut(), &mut rest, &values);
    close(&rest, &expected[47..]);
    bypassed.reset();
    let mut reset = [[1.0, -0.5]; 97];
    process(bypassed.as_mut(), &mut reset, &values);
    close(&reset, &expected);
}

#[test]
fn invalid_first_values_and_unadmitted_ids_leave_effective_controls_and_dsp_pristine() {
    let base = AUTO_PAN_DEFINITION.default_state().unwrap();
    let ids = vec![parameter::AMOUNT, parameter::RATE];
    let prepared = factory(&base, &ids);
    let mut candidate = prepared.create_runtime();
    let invalid = Values {
        ids: ids.clone(),
        values: vec![
            Some(ParameterValue::Float(1.0)),
            Some(ParameterValue::Float(f32::NAN)),
        ],
    };
    let mut refused = [[1.0, -0.5]; 1];
    assert_eq!(
        candidate.process_block_with_automation(
            &mut refused,
            None,
            &invalid,
            &EMPTY_DEVICE_RESOURCES
        ),
        Err(DeviceContractError::InvalidParameterValue)
    );
    assert_eq!(refused, [[1.0, -0.5]; 1]);
    let unadmitted = Values {
        ids: vec![parameter::WAVEFORM],
        values: vec![Some(ParameterValue::Choice(4))],
    };
    assert!(
        candidate
            .process_block_with_automation(&mut refused, None, &unadmitted, &EMPTY_DEVICE_RESOURCES)
            .is_err()
    );
    let valid = Values {
        ids,
        values: vec![
            Some(ParameterValue::Float(1.0)),
            Some(ParameterValue::Float(2.0)),
        ],
    };
    let mut actual = [[1.0, -0.5]; 73];
    let mut expected = actual;
    process(candidate.as_mut(), &mut actual, &valid);
    process(prepared.create_runtime().as_mut(), &mut expected, &valid);
    assert_eq!(actual, expected);
}

#[test]
fn inheritance_rebuilds_changed_base_and_removed_targets_but_preserves_effective_ramps() {
    let base = AUTO_PAN_DEFINITION.default_state().unwrap();
    // Public superset admission with an actual one-parameter borrowed producer.
    let all = [
        parameter::AMOUNT,
        parameter::RATE,
        parameter::RIGHT_PHASE,
        parameter::WAVEFORM,
    ];
    let values = Values {
        ids: vec![parameter::AMOUNT],
        values: vec![Some(ParameterValue::Float(1.0))],
    };
    let prepared = factory(&base, &all);
    let mut previous = prepared.create_runtime();
    process(previous.as_mut(), &mut [[1.0, -0.5]; 13], &values);
    let snapshot = *previous.as_any().downcast_ref::<AutoPanRuntime>().unwrap();
    let mut inherited = prepared.create_runtime();
    inherited.copy_history_from(previous.as_ref());
    let mut actual = [[1.0, -0.5]; 83];
    let mut expected = actual;
    process(inherited.as_mut(), &mut actual, &values);
    process(previous.as_mut(), &mut expected, &values);
    assert_eq!(
        actual, expected,
        "unchanged cache must not retarget to latent base"
    );
    // Ordinary frozen processing advances the last committed transitions.
    let mut frozen = snapshot;
    let mut held = snapshot;
    frozen.process_block(&mut actual, &EMPTY_DEVICE_RESOURCES);
    process(&mut held, &mut expected, &values);
    assert_eq!(actual, expected);
    let changed_base = AUTO_PAN_DEFINITION
        .patch_parameter(&base, parameter::RATE, ParameterValue::Float(4.0))
        .unwrap();
    let mut changed = factory(&changed_base, &all).create_runtime();
    changed.copy_history_from(&snapshot);
    // Equivalent manual retarget, preserving the active amount instead of the
    // candidate's latent .5. The unchanged producer must still rebuild rate.
    let target = AUTO_PAN_DEFINITION
        .patch_parameter(&changed_base, parameter::AMOUNT, ParameterValue::Float(1.0))
        .unwrap();
    let mut manual = factory(&target, &[]).create_runtime();
    manual.copy_history_from(&snapshot);
    actual.fill([1.0, -0.5]);
    expected = actual;
    process(changed.as_mut(), &mut actual, &values);
    manual.process_block(&mut expected, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(actual, expected);
    // Missing origin restores base through the same transition. A last-lane
    // removal takes the ordinary path and must continue that identical history.
    let absent = Values {
        ids: values.ids.clone(),
        values: vec![None],
    };
    let mut missing = prepared.create_runtime();
    missing.copy_history_from(&snapshot);
    let mut removed = factory(&base, &[]).create_runtime();
    removed.copy_history_from(&snapshot);
    actual.fill([1.0, -0.5]);
    expected = actual;
    process(missing.as_mut(), &mut actual, &absent);
    removed.process_block(&mut expected, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(actual, expected);
    assert_eq!(AUTO_PAN_DEFINITION.default_state().unwrap(), base);
}
