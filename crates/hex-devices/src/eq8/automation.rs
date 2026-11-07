//! Borrowed gain targets reuse EQ8's existing smoother and routing/audition DSP.
//! No coefficient Arc is replaced and no source view is retained. Admission and
//! Copy delivery state preserve the active target across compatible histories.
use super::*;
use crate::parameter_binding::{Binding, SCALAR_ENVELOPE};
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Gain(pub f32);
pub(super) const SEMANTICS: hex_lattice_contract::ParameterSemantics =
    hex_lattice_contract::ParameterSemantics {
        smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
        ..SCALAR_ENVELOPE
    };
const BINDINGS: [Binding<Gain>; 1] = [Binding::float(
    DESCRIPTORS[201],
    SEMANTICS,
    |g| g.0,
    |g, v| g.0 = v,
)];
impl Eq8Runtime {
    pub(super) fn retarget_gain(&mut self, gain: f64) {
        if self.target_gain != gain {
            self.target_gain = gain;
            self.smooth_remaining = coefficient_smooth_frames(self.sample_rate);
        }
    }
    pub(super) fn process_gain_automation(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn hex_lattice::NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.gain_controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) =
                self.gain_controls
                    .sample(&BINDINGS, &mut resolved, values, offset)?
            {
                let gain = f64::from(next.effective().0);
                self.gain_controls.install(next);
                self.retarget_gain(gain);
            }
            let input = *frame;
            let output = self.process_frame(input);
            if !bypassed {
                *frame = output;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{EMPTY_DEVICE_RESOURCES as R, NativeParameterAutomation};
    struct Values {
        ids: Vec<ParameterId>,
        value: Option<ParameterValue>,
        dense: bool,
        start: usize,
    }
    impl NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, _: ParameterId, offset: usize) -> Option<ParameterValue> {
            if self.dense {
                Some(ParameterValue::Float(
                    ((self.start + offset) % 65) as f32 / 16.0,
                ))
            } else {
                self.value
            }
        }
    }
    fn values(value: Option<f32>) -> Values {
        Values {
            ids: vec![parameter::OUTPUT_GAIN],
            value: value.map(ParameterValue::Float),
            dense: false,
            start: 0,
        }
    }
    fn room(state: &NativeDeviceState, rate: u32, admitted: bool) -> Eq8Runtime {
        let mut prepared = PreparedEq8::new(decode(state).unwrap(), rate).unwrap();
        prepared.gain_admitted = admitted;
        prepared.runtime()
    }
    fn signal(len: usize) -> Vec<StereoFrame> {
        (0..len)
            .map(|i| {
                if i % 31 < 7 {
                    [0.25, -0.125]
                } else {
                    [0.01, -0.02]
                }
            })
            .collect()
    }
    fn state(mode: u32, filtered: bool) -> NativeDeviceState {
        let mut state = EQ8_DEFINITION.default_state().unwrap();
        state = EQ8_DEFINITION
            .patch_parameter(&state, parameter::MODE, ParameterValue::Choice(mode))
            .unwrap();
        for bank in 0..5 {
            for band in 0..BAND_COUNT {
                state = EQ8_DEFINITION
                    .patch_parameter(
                        &state,
                        parameter::enabled(bank, band),
                        ParameterValue::Bool(filtered && band == 1),
                    )
                    .unwrap();
            }
            if filtered {
                state = EQ8_DEFINITION
                    .patch_parameter(&state, parameter::gain(bank, 1), ParameterValue::Float(6.0))
                    .unwrap();
            }
        }
        state
    }
    #[test]
    fn gain_held_dense_and_fallback_match_canonical_rooms_in_all_modes() {
        for rate in [8000, 96000] {
            for mode in 0..3 {
                let base = state(mode, true);
                let mut source = room(&base, rate, false);
                source.process_block(&mut signal(513), &R);
                for gain in [0.0, 0.25, 2.0, 16.0] {
                    let mut actual = room(&base, rate, true);
                    actual.inherit_history_from(&source);
                    let changed = EQ8_DEFINITION
                        .patch_parameter(&base, parameter::OUTPUT_GAIN, ParameterValue::Float(gain))
                        .unwrap();
                    let mut reference = room(&changed, rate, false);
                    reference.inherit_history_from(&source);
                    let values = values(Some(gain));
                    let mut output = signal(1024);
                    let mut expected = output.clone();
                    for block in output.chunks_mut(13) {
                        actual
                            .process_gain_automation(block, &values, false)
                            .unwrap();
                    }
                    reference.process_block(&mut expected, &R);
                    assert_eq!(output, expected);
                }
                let mut actual = room(&base, rate, true);
                actual.inherit_history_from(&source);
                let mut split = room(&base, rate, true);
                split.inherit_history_from(&source);
                let mut reference = source;
                let mut dense = values(Some(1.0));
                dense.dense = true;
                let mut output = signal(128);
                let mut expected = output.clone();
                let mut partitioned = output.clone();
                actual
                    .process_gain_automation(&mut output, &dense, false)
                    .unwrap();
                for (i, block) in partitioned.chunks_mut(13).enumerate() {
                    dense.start = i * 13;
                    split.process_gain_automation(block, &dense, false).unwrap();
                }
                assert_eq!(output, partitioned);
                dense.start = 0;
                for (i, frame) in expected.iter_mut().enumerate() {
                    let changed = EQ8_DEFINITION
                        .patch_parameter(
                            &base,
                            parameter::OUTPUT_GAIN,
                            dense.value_at(parameter::OUTPUT_GAIN, i).unwrap(),
                        )
                        .unwrap();
                    let mut next = room(&changed, rate, false);
                    next.inherit_history_from(&reference);
                    next.process_block(std::slice::from_mut(frame), &R);
                    reference = next;
                }
                assert_eq!(output, expected);
                let mut fallback = room(&base, rate, false);
                fallback.inherit_history_from(&reference);
                output = signal(1024);
                expected = output.clone();
                actual
                    .process_gain_automation(&mut output, &values(None), false)
                    .unwrap();
                fallback.process_block(&mut expected, &R);
                assert_eq!(output, expected);
            }
        }
    }
    #[test]
    fn gain_history_bypass_empty_calls_tails_and_canonical_successors_preserve_audio() {
        for rate in [8000, 96000] {
            let base = state(0, true);
            let held = values(Some(2.0));
            let mut source = room(&base, rate, true);
            source
                .process_gain_automation(&mut signal(1024), &held, false)
                .unwrap();
            let mut copied = room(&base, rate, true);
            copied.inherit_history_from(&source);
            let mut bypassed = room(&base, rate, true);
            bypassed.inherit_history_from(&source);
            let mut output = signal(17);
            let mut expected = output.clone();
            source
                .process_gain_automation(&mut output, &held, false)
                .unwrap();
            copied
                .process_gain_automation(&mut [], &values(Some(4.0)), false)
                .unwrap();
            copied
                .process_gain_automation(&mut expected, &held, false)
                .unwrap();
            assert_eq!(output, expected);
            let mut dry = signal(17);
            let original = dry.clone();
            bypassed
                .process_gain_automation(&mut dry, &held, true)
                .unwrap();
            assert_eq!(dry, original);
            output = signal(13);
            expected = output.clone();
            source
                .process_gain_automation(&mut output, &held, false)
                .unwrap();
            bypassed
                .process_gain_automation(&mut expected, &held, false)
                .unwrap();
            assert_eq!(output, expected);
            let mut tail_reference = room(&base, rate, true);
            tail_reference.inherit_history_from(&source);
            source.begin_output_tail();
            output = vec![[0.0; 2]; 64];
            expected = output.clone();
            source.process_block(&mut output, &R);
            tail_reference
                .process_gain_automation(&mut expected, &held, false)
                .unwrap();
            assert_eq!(output, expected);
            let changed = EQ8_DEFINITION
                .patch_parameter(&base, parameter::OUTPUT_GAIN, ParameterValue::Float(0.5))
                .unwrap();
            let mut canonical = room(&changed, rate, false);
            canonical.inherit_history_from(&source);
            let mut absent = room(&changed, rate, true);
            absent.inherit_history_from(&source);
            output = signal(1024);
            expected = output.clone();
            canonical.process_block(&mut output, &R);
            let empty = Values {
                ids: vec![],
                value: None,
                dense: false,
                start: 0,
            };
            absent
                .process_gain_automation(&mut expected, &empty, false)
                .unwrap();
            assert_eq!(output, expected);
            canonical.begin_output_tail();
            absent.begin_output_tail();
            output.fill([0.0; 2]);
            expected.fill([0.0; 2]);
            canonical.process_block(&mut output, &R);
            absent.process_block(&mut expected, &R);
            assert_eq!(output, expected);
            absent.reset();
            let mut fresh = room(&changed, rate, true);
            output = signal(128);
            expected = output.clone();
            absent
                .process_gain_automation(&mut output, &held, false)
                .unwrap();
            fresh
                .process_gain_automation(&mut expected, &held, false)
                .unwrap();
            assert_eq!(output, expected);
        }
    }
    #[test]
    fn routing_fade_keeps_outgoing_gain_and_initializes_the_incoming_control_law() {
        for rate in [8000, 96000] {
            for gain in [None, Some(2.0)] {
                let base = state(0, false);
                let mut source = room(&base, rate, true);
                source
                    .process_gain_automation(&mut signal(1024), &values(Some(2.0)), false)
                    .unwrap();
                let changed = EQ8_DEFINITION
                    .patch_parameter(&base, parameter::MODE, ParameterValue::Choice(2))
                    .unwrap();
                let mut next = room(&changed, rate, true);
                assert!(!next.inherit_history_from(&source));
                let input = [0.125, -0.0625];
                let mut output = [input];
                next.process_gain_automation(&mut output, &values(gain), false)
                    .unwrap();
                let frames = f64::from(coefficient_smooth_frames(rate));
                let incoming_gain = 1.0 + (gain.unwrap_or(1.0) as f64 - 1.0) / frames;
                for (c, &sample) in input.iter().enumerate() {
                    let incoming = (f64::from(sample) * incoming_gain) as f32;
                    let expected = (f64::from(incoming)
                        + (f64::from(sample) * 2.0 - f64::from(incoming)) * (frames - 1.0) / frames)
                        as f32;
                    assert!((output[0][c] - expected).abs() < 1e-8);
                }
            }
        }
    }
    #[test]
    fn gain_requires_admission_and_rejects_bad_values_without_touching_audio() {
        let base = state(0, false);
        let mut plain = room(&base, 8000, false);
        let held = values(Some(2.0));
        let mut output = [[0.25; 2]];
        assert!(
            plain
                .process_gain_automation(&mut output, &held, false)
                .is_err()
        );
        assert_eq!(output, [[0.25; 2]]);
        for invalid in [
            ParameterValue::Bool(true),
            ParameterValue::Float(f32::NAN),
            ParameterValue::Float(f32::INFINITY),
            ParameterValue::Float(-1.0),
            ParameterValue::Float(17.0),
        ] {
            let mut actual = room(&base, 8000, true);
            let bad = Values {
                ids: held.ids.clone(),
                value: Some(invalid),
                dense: false,
                start: 0,
            };
            assert!(
                actual
                    .process_gain_automation(&mut output, &bad, false)
                    .is_err()
            );
            assert_eq!(output, [[0.25; 2]]);
        }
        assert!(
            !EQ8_DEFINITION
                .supports_parameter_automation(&base, parameter::MODE)
                .unwrap()
        );
    }
    #[test]
    #[ignore = "release-only targeted delivery microbenchmark"]
    fn eq8_gain_delivery_cost() {
        use std::{hint::black_box, time::Instant};
        assert!(!black_box(cfg!(debug_assertions)), "run with --release");
        let state = state(0, false);
        for mode in 0..3 {
            let mut source = values(Some(2.0));
            source.dense = mode == 2;
            let mut runtime = room(&state, 48000, mode != 0);
            let mut block = [[0.125, -0.25]; 128];
            let mut run = || {
                for _ in 0..128 {
                    block.fill([0.125, -0.25]);
                    if mode == 0 {
                        runtime.process_block(&mut block, &R);
                    } else {
                        runtime
                            .process_gain_automation(&mut block, &source, false)
                            .unwrap();
                    }
                    black_box(&block);
                }
            };
            for _ in 0..4 {
                run();
            }
            let mut times = [0.0; 7];
            for time in &mut times {
                let start = Instant::now();
                run();
                *time = start.elapsed().as_nanos() as f64 / 16384.0;
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "mode={mode} rate=48000 block=128 frames=16384 warmup=4 repeats=7 median_ns_per_frame={:.3}",
                times[3]
            );
        }
    }
}
