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
