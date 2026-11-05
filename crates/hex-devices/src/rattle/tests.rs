use super::*;
use hex_lattice::{EMPTY_DEVICE_RESOURCES, NativeParameterAutomation};

const RATE: u32 = 48_000;
const LATENCY: usize = LATENCY_FRAMES as usize;
const HALF_SECOND: usize = RATE as usize / 2;

fn state(drive: f32) -> NativeDeviceState {
    RATTLE_DEFINITION
        .patch_parameter(
            &RATTLE_DEFINITION.default_state().unwrap(),
            parameter::DRIVE,
            ParameterValue::Float(drive),
        )
        .unwrap()
}
fn runtime(drive: f32) -> Box<dyn NativeProcessorRuntime> {
    RattleDefinition::prepare_owned(&state(drive), RATE, &[])
        .unwrap()
        .create_runtime()
}
fn render(drive: f32, input: &[StereoFrame]) -> Vec<StereoFrame> {
    let mut output = input.to_vec();
    runtime(drive).process_block(&mut output, &EMPTY_DEVICE_RESOURCES);
    output
}
fn sine(hz: f64, amplitude: f64, count: usize) -> Vec<StereoFrame> {
    (0..count)
        .map(|i| {
            let phase = std::f64::consts::TAU * hz * i as f64 / f64::from(RATE);
            [
                (phase.sin() * amplitude) as f32,
                (phase.cos() * amplitude) as f32,
            ]
        })
        .collect()
}
fn rms(frames: &[StereoFrame], channel: usize) -> f64 {
    (frames
        .iter()
        .map(|f| f64::from(f[channel]).powi(2))
        .sum::<f64>()
        / frames.len() as f64)
        .sqrt()
}
fn mean(frames: &[StereoFrame], channel: usize) -> f64 {
    frames.iter().map(|f| f64::from(f[channel])).sum::<f64>() / frames.len() as f64
}

struct Values {
    ids: Vec<ParameterId>,
    value: Option<ParameterValue>,
}
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        &self.ids
    }
    fn value_at(&self, parameter: ParameterId, _offset: usize) -> Option<ParameterValue> {
        (self.ids.first() == Some(&parameter))
            .then_some(self.value)
            .flatten()
    }
}

#[test]
fn registered_codec_defaults_and_invalid_states() {
    let registry = crate::native_registry().unwrap();
    let default = registry.default_state(RATTLE_TYPE_ID).unwrap();
    assert_eq!(default.payload(), 0.4_f32.to_le_bytes());
    assert_eq!(registry.migrate_state(&default).unwrap(), default);
    assert_eq!(
        RATTLE_DEFINITION.read_parameter(&default, parameter::DRIVE),
        Ok(ParameterValue::Float(0.4))
    );
    for value in [0.0, 1.0] {
        let changed = state(value);
        assert_eq!(registry.migrate_state(&changed).unwrap(), changed);
    }
    for value in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        assert!(
            RATTLE_DEFINITION
                .patch_parameter(&default, parameter::DRIVE, ParameterValue::Float(value))
                .is_err()
        );
    }
    for (version, payload) in [
        (1, vec![0; 3]),
        (1, vec![0; 5]),
        (1, f32::NAN.to_le_bytes().to_vec()),
        (1, 2.0_f32.to_le_bytes().to_vec()),
        (2, vec![0; 4]),
    ] {
        let malformed = NativeDeviceState::new(RATTLE_TYPE_ID, version, payload).unwrap();
        assert!(RATTLE_DEFINITION.migrate_state(&malformed).is_err());
        assert!(RattleDefinition::prepare_owned(&malformed, RATE, &[]).is_err());
    }
    assert!(RattleDefinition::prepare_owned(&default, 7_999, &[]).is_err());
    assert!(RattleDefinition::prepare_owned(&default, 192_001, &[]).is_err());
}

#[test]
fn declares_latency_tail_and_drive_automation() {
    let default = state(0.4);
    assert_eq!(
        RATTLE_DEFINITION.supports_parameter_automation(&default, parameter::DRIVE),
        Ok(true)
    );
    assert_eq!(
        RATTLE_DEFINITION.supports_parameter_automation(&default, ParameterId::from_static(2)),
        Ok(false)
    );
    let plain = RattleDefinition::prepare_owned(&default, RATE, &[]).unwrap();
    let automated = RattleDefinition::prepare_owned(&default, RATE, &[parameter::DRIVE]).unwrap();
    assert_eq!(plain.algorithmic_latency_frames(), Some(LATENCY_FRAMES));
    assert!(!plain.supports_parameter_automation(parameter::DRIVE));
    assert!(automated.supports_parameter_automation(parameter::DRIVE));
    assert!(matches!(
        plain.output_tail(),
        OutputTail::Finite { max_tail_frames } if max_tail_frames > u64::from(LATENCY_FRAMES)
    ));
}

#[test]
fn zero_drive_is_the_bit_exact_latency_aligned_input() {
    let input: Vec<_> = sine(997.0, 0.9, 2_000)
        .into_iter()
        .enumerate()
        .map(|(i, f)| if i % 113 == 0 { [1.0, -1.0] } else { f })
        .collect();
    let output = render(0.0, &input);
    assert!(output[..LATENCY].iter().all(|f| *f == [0.0; 2]));
    assert_eq!(&output[LATENCY..], &input[..input.len() - LATENCY]);
}

#[test]
fn full_drive_is_bounded_finite_and_dc_free() {
    for amplitude in [1.0, 0.25] {
        // Whole periods in the settled window, so a symmetric wave has zero mean.
        let output = render(1.0, &sine(100.0, amplitude, HALF_SECOND));
        assert!(
            output
                .iter()
                .flatten()
                .all(|s| s.is_finite() && s.abs() <= 1.0)
        );
        let settled = &output[HALF_SECOND / 2..];
        for channel in 0..2 {
            assert!(
                mean(settled, channel).abs() < 2.0e-3,
                "dc {}",
                mean(settled, channel)
            );
        }
    }
    // Extreme, impulsive and poisoned input stays finite and bounded.
    let mut extreme = vec![[1.0e30_f32, -1.0e30]; 64];
    extreme[3] = [f32::NAN, 0.5];
    extreme[9] = [f32::INFINITY, f32::NEG_INFINITY];
    let output = render(1.0, &extreme);
    assert!(
        output
            .iter()
            .flatten()
            .all(|s| s.is_finite() && s.abs() <= 1.0)
    );
}

#[test]
fn rms_stays_within_one_and_a_half_db_of_input_across_drive() {
    // Calibration fixture: -12 dbfs 220 hz sine, settled second half. Louder or
    // quieter programme is compressed or expanded by the saturator by design.
    let input = sine(220.0, 0.25, HALF_SECOND);
    let reference = rms(&input[HALF_SECOND / 2..], 0);
    for drive in [0.1, 0.3, 0.5, 0.7, 1.0] {
        let output = render(drive, &input);
        let db = 20.0 * (rms(&output[HALF_SECOND / 2..], 0) / reference).log10();
        assert!(db.abs() < 1.5, "drive {drive}: {db:.2} db");
    }
}

#[test]
fn drive_adds_grit_and_harmonics_progressively() {
    // Residual energy after removing the latency-aligned fundamental grows with drive.
    let input = sine(220.0, 0.25, HALF_SECOND);
    let mut previous = 0.0;
    for drive in [0.2, 0.5, 1.0] {
        let output = render(drive, &input);
        let window = HALF_SECOND / 2..HALF_SECOND;
        let (mut dot, mut norm) = (0.0, 0.0);
        for i in window.clone() {
            let dry = f64::from(input[i - LATENCY][0]);
            dot += f64::from(output[i][0]) * dry;
            norm += dry * dry;
        }
        let scale = dot / norm;
        let residual = window
            .map(|i| (f64::from(output[i][0]) - scale * f64::from(input[i - LATENCY][0])).powi(2))
            .sum::<f64>();
        let total = (HALF_SECOND / 2..HALF_SECOND)
            .map(|i| f64::from(output[i][0]).powi(2))
            .sum::<f64>();
        let ratio = residual / total;
        assert!(ratio > previous, "drive {drive}: {ratio} <= {previous}");
        previous = ratio;
    }
    assert!(previous > 0.05);
}

#[test]
fn silence_stays_silent_and_long_silence_clears_history() {
    let silence = vec![[0.0; 2]; RATE as usize];
    assert!(render(1.0, &silence).iter().all(|f| *f == [0.0; 2]));
    let mut runtime = runtime(1.0);
    let mut burst = sine(440.0, 0.8, 512);
    runtime.process_block(&mut burst, &EMPTY_DEVICE_RESOURCES);
    assert!(!runtime.is_quiescent());
    let mut tail = vec![[0.0; 2]; RATE as usize / 2];
    runtime.process_block(&mut tail, &EMPTY_DEVICE_RESOURCES);
    assert!(runtime.is_quiescent());
    assert!(tail.iter().flatten().all(|s| s.is_finite()));
    assert_eq!(*tail.last().unwrap(), [0.0; 2]);
}

#[test]
fn renders_are_deterministic_across_block_partitions_and_reset() {
    let input = sine(330.0, 0.6, 4_096);
    let whole = render(0.8, &input);
    let mut runtime = runtime(0.8);
    let mut split = input.clone();
    let mut from = 0;
    for size in [1, 7, 64, 13, 512, 3, 1_000].iter().cycle() {
        if from == split.len() {
            break;
        }
        let to = (from + size).min(split.len());
        runtime.process_block(&mut split[from..to], &EMPTY_DEVICE_RESOURCES);
        from = to;
    }
    assert_eq!(split, whole);
    runtime.reset();
    let mut again = input.clone();
    runtime.process_block(&mut again, &EMPTY_DEVICE_RESOURCES);
    assert_eq!(again, whole);
}

#[test]
fn automated_drive_is_smoothed_partition_independent_and_bypass_passes_through() {
    let ids = vec![parameter::DRIVE];
    let prepared = RattleDefinition::prepare_owned(&state(0.0), RATE, &ids).unwrap();
    let values = Values {
        ids,
        value: Some(ParameterValue::Float(1.0)),
    };
    let input = sine(220.0, 0.25, 4_800);
    let mut whole = input.clone();
    prepared
        .create_runtime()
        .process_block_with_automation(&mut whole, None, &values, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    let mut runtime = prepared.create_runtime();
    let mut split = input.clone();
    for chunk in split.chunks_mut(37) {
        runtime
            .process_block_with_automation(chunk, None, &values, &EMPTY_DEVICE_RESOURCES)
            .unwrap();
    }
    assert_eq!(split, whole);
    // The first frames move gradually away from the zero-drive dry path.
    let first_step = (f64::from(whole[LATENCY + 1][0]) - f64::from(input[1][0])).abs();
    assert!(first_step < 0.01, "{first_step}");
    let mut bypassed = input.clone();
    prepared
        .create_runtime()
        .process_bypassed_block_with_automation(
            &mut bypassed,
            None,
            &values,
            &EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    assert_eq!(bypassed, input);
}

#[test]
fn parameter_edit_inherits_history_and_glides_to_the_new_drive() {
    let input = sine(220.0, 0.5, 2_048);
    let mut old = runtime(0.0);
    let mut head = input[..1_024].to_vec();
    old.process_block(&mut head, &EMPTY_DEVICE_RESOURCES);
    let mut new = runtime(1.0);
    assert!(new.can_copy_history_from(old.as_ref()));
    new.copy_history_from(old.as_ref());
    let mut tail = input[1_024..].to_vec();
    new.process_block(&mut tail, &EMPTY_DEVICE_RESOURCES);
    // The first inherited frame still matches the old zero-drive dry path closely.
    let dry = f64::from(input[1_024 - LATENCY][0]);
    assert!((f64::from(tail[0][0]) - dry).abs() < 0.01);
    let other_rate = RattleDefinition::prepare_owned(&state(1.0), 44_100, &[])
        .unwrap()
        .create_runtime();
    assert!(!other_rate.can_copy_history_from(old.as_ref()));
}
