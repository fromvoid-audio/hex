//! Borrowed output-only controls: authored values bypass canonical edit ramps,
//! matching the existing dry/wet law. Room stereo/gains are sampled at the
//! existing tank evaluations. They never change feedback, capacities or tails.
use super::*;
use crate::parameter_binding::{Binding, SCALAR_ENVELOPE};
use hex_lattice_contract::ParameterSemantics;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct OutputSettings {
    dry_wet: f32,
    stereo: f32,
    reflect: f32,
    diffuse: f32,
}
impl OutputSettings {
    pub(super) fn new(settings: Settings) -> Self {
        Self {
            dry_wet: settings.get(p::DRY_WET),
            stereo: settings.get(p::STEREO),
            reflect: settings.get(p::REFLECT),
            diffuse: settings.get(p::DIFFUSE),
        }
    }
    pub(super) fn check_inline(&self) {
        let Self {
            dry_wet,
            stereo,
            reflect,
            diffuse,
        } = self;
        let _: (&f32, &f32, &f32, &f32) = (dry_wet, stereo, reflect, diffuse);
    }
}
const SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
    ..SCALAR_ENVELOPE
};
const BINDINGS: [Binding<OutputSettings>; 4] = [
    Binding::float(
        crate::reverb::DESCRIPTORS[1],
        SEMANTICS,
        |s| s.dry_wet,
        |s, v| s.dry_wet = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[28],
        SEMANTICS,
        |s| s.stereo,
        |s, v| s.stereo = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[29],
        SEMANTICS,
        |s| s.reflect,
        |s, v| s.reflect = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[30],
        SEMANTICS,
        |s| s.diffuse,
        |s, v| s.diffuse = v,
    ),
];
pub(in crate::reverb) fn supports(id: ParameterId) -> bool {
    crate::parameter_binding::binding(&BINDINGS, id).is_ok()
}
pub(in crate::reverb) fn semantics(id: ParameterId) -> Option<ParameterSemantics> {
    crate::parameter_binding::binding(&BINDINGS, id)
        .ok()
        .map(|b| b.semantics)
}
pub(in crate::reverb) fn validate_ids(ids: &[ParameterId]) -> Result<(), DeviceContractError> {
    if ids.is_empty() || ids.windows(2).any(|p| p[0] >= p[1]) || ids.iter().any(|&id| !supports(id))
    {
        return Err(DeviceContractError::UnsupportedProcessingClass);
    }
    Ok(())
}
#[derive(Clone, Copy, Default)]
pub(super) struct OutputValues {
    pub dry_wet: Option<f32>,
    pub stereo: Option<f32>,
    pub reflect: Option<f32>,
    pub diffuse: Option<f32>,
}
impl OutputValues {
    pub(super) fn check_inline(&self) {
        let Self {
            dry_wet,
            stereo,
            reflect,
            diffuse,
        } = self;
        let _: (&Option<f32>, &Option<f32>, &Option<f32>, &Option<f32>) =
            (dry_wet, stereo, reflect, diffuse);
    }
}
impl ReverbRuntime {
    pub(super) fn process_output_automation(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn NativeParameterAutomation,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.output_controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) =
                self.output_controls
                    .sample(&BINDINGS, &mut resolved, values, offset)?
            {
                let settings = next.effective();
                let old = self.output_controls.effective();
                // Installed presence distinguishes authored values from the
                // canonical ramp fallback without another producer query.
                self.output_controls.install(next);
                let present = |index| self.output_controls.has_value(index);
                let gain = |index: usize, db: f32, old_db: f32, previous: Option<f32>| {
                    if present(index) {
                        Some(if old_db == db {
                            previous.unwrap_or_else(|| 10.0_f32.powf(db / 20.0))
                        } else {
                            10.0_f32.powf(db / 20.0)
                        })
                    } else {
                        None
                    }
                };
                let output = OutputValues {
                    dry_wet: present(0).then_some(settings.dry_wet),
                    stereo: present(1).then_some(settings.stereo),
                    reflect: gain(2, settings.reflect, old.reflect, self.output_values.reflect),
                    diffuse: gain(3, settings.diffuse, old.diffuse, self.output_values.diffuse),
                };
                self.output_values = output;
            }
            self.frame_outputs(frame, self.output_values);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{EMPTY_DEVICE_RESOURCES as R, NativeDeviceDefinition};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    struct Values {
        ids: Vec<ParameterId>,
        values: Vec<Option<ParameterValue>>,
    }
    impl NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
            self.ids
                .iter()
                .position(|&p| p == id)
                .and_then(|i| self.values[i])
        }
    }
    fn signal(len: usize) -> Vec<StereoFrame> {
        (0..len)
            .map(|i| {
                if i % 97 < 31 {
                    [0.125, -0.25]
                } else {
                    [0.0; 2]
                }
            })
            .collect()
    }
    #[test]
    fn each_output_matches_static_pcm_at_host_and_downsampled_tank_rates() {
        let settings = Settings::default_values();
        for rate in [8000, 96000] {
            for (id, value) in [
                (p::DRY_WET, 100.0),
                (p::STEREO, 0.0),
                (p::REFLECT, -12.0),
                (p::DIFFUSE, -12.0),
            ] {
                let input = Values {
                    ids: vec![id],
                    values: vec![Some(ParameterValue::Float(value))],
                };
                let mut changed = settings;
                changed.0[crate::reverb::index(id).unwrap()] = value;
                let mut actual = PreparedReverb::new(rate, settings).unwrap().runtime();
                let mut reference = PreparedReverb::new(rate, changed).unwrap().runtime();
                let mut output = signal(4096);
                let mut expected = output.clone();
                reference.process_block(&mut expected, &R);
                for block in output.chunks_mut(13) {
                    actual.process_output_automation(block, &input).unwrap();
                }
                assert_eq!(output, expected, "parameter {} at {rate}", id.value());
                assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
            }
        }
    }
    #[test]
    fn malformed_atomic_delivery_history_bypass_reset_and_storage_are_preserved() {
        let settings = Settings::default_values();
        let prepared = PreparedReverb::new(8000, settings).unwrap();
        let mut source = prepared.runtime();
        let mut reference = prepared.runtime();
        let mut values = Values {
            ids: vec![p::DRY_WET, p::STEREO],
            values: vec![
                Some(ParameterValue::Float(100.0)),
                Some(ParameterValue::Float(f32::NAN)),
            ],
        };
        let mut output = [[0.125; 2]; 17];
        assert!(
            source
                .process_output_automation(&mut output, &values)
                .is_err()
        );
        assert_eq!(output, [[0.125; 2]; 17]);
        values.values[1] = Some(ParameterValue::Float(25.0));
        let mut expected = output;
        source
            .process_output_automation(&mut output, &values)
            .unwrap();
        reference
            .process_output_automation(&mut expected, &values)
            .unwrap();
        assert_eq!(output, expected);
        let mut copied = prepared.runtime();
        copied.copy_history_from(&source);
        let mut bypassed = prepared.runtime();
        bypassed.copy_history_from(&source);
        output.fill([0.125; 2]);
        expected = output;
        source
            .process_output_automation(&mut output, &values)
            .unwrap();
        copied
            .process_output_automation(&mut expected, &values)
            .unwrap();
        assert_eq!(output, expected);
        expected.fill([0.125; 2]);
        bypassed
            .process_bypassed_block_with_automation(&mut expected, None, &values, &R)
            .unwrap();
        assert_eq!(expected, [[0.125; 2]; 17]);
        output.fill([0.125; 2]);
        source
            .process_output_automation(&mut output, &values)
            .unwrap();
        bypassed
            .process_output_automation(&mut expected, &values)
            .unwrap();
        assert_eq!(output, expected);
        copied.reset();
        reference.reset();
        let absent = Values {
            ids: vec![],
            values: vec![],
        };
        output.fill([0.125; 2]);
        expected = output;
        copied
            .process_output_automation(&mut output, &absent)
            .unwrap();
        reference.process_block(&mut expected, &R);
        assert_eq!(output, expected);
        let storage = copied.control_storage().unwrap();
        assert_eq!(storage.inline_bytes, std::mem::size_of::<ReverbRuntime>());
        assert_eq!(storage.heap_bytes.len(), 14);
        assert!(validate_ids(&[p::DECAY]).is_err());
        assert!(validate_ids(&[p::STEREO, p::DRY_WET]).is_err());
        assert!(
            crate::reverb::REVERB_DEFINITION
                .supports_parameter_automation(
                    &crate::reverb::REVERB_DEFINITION.default_state().unwrap(),
                    p::REFLECT
                )
                .unwrap()
        );
    }
}

#[cfg(test)]
mod benchmark {
    use super::*;
    use std::{hint::black_box, time::Instant};
    struct Source {
        ids: Vec<ParameterId>,
        dense: bool,
    }
    impl NativeParameterAutomation for Source {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(
            &self,
            id: ParameterId,
            offset: usize,
        ) -> Option<hex_lattice_contract::ParameterValue> {
            let t = if self.dense {
                offset as f32 / 128.0
            } else {
                0.5
            };
            Some(hex_lattice_contract::ParameterValue::Float(match id {
                p::DRY_WET => 25.0 + 50.0 * t,
                p::STEREO => 120.0 * t,
                p::REFLECT => -12.0 + 12.0 * t,
                p::DIFFUSE => -6.0 * t,
                _ => return None,
            }))
        }
    }
    // The pre-adapter scalar delivery body, measured against the same current
    // DSP/ring implementation. This is a loop baseline, not an old-binary run.
    fn legacy(runtime: &mut ReverbRuntime, frames: &mut [StereoFrame], source: &Source) {
        for (offset, frame) in frames.iter_mut().enumerate() {
            let mut values = [None];
            source.values_at(offset, &mut values).unwrap();
            let wet = match values[0] {
                Some(hex_lattice_contract::ParameterValue::Float(v))
                    if (0.0..=100.0).contains(&v) =>
                {
                    Some(v)
                }
                None => None,
                _ => panic!("bounded benchmark source"),
            };
            runtime.frame(frame, wet);
        }
    }
    #[test]
    #[ignore = "release-only targeted delivery microbenchmark"]
    fn output_delivery_cost_against_legacy_scalar_loop() {
        assert!(!cfg!(debug_assertions), "run with --release");
        for mode in 0..4 {
            let source = Source {
                ids: if mode < 2 {
                    vec![p::DRY_WET]
                } else {
                    vec![p::DRY_WET, p::STEREO, p::REFLECT, p::DIFFUSE]
                },
                dense: mode == 3,
            };
            let mut runtime = PreparedReverb::new(48000, Settings::default_values())
                .unwrap()
                .runtime();
            let mut block = [[0.125, -0.25]; 128];
            let mut run = || {
                for _ in 0..128 {
                    block.fill([0.125, -0.25]);
                    if mode == 0 {
                        legacy(&mut runtime, &mut block, &source);
                    } else {
                        runtime
                            .process_output_automation(&mut block, &source)
                            .unwrap();
                    }
                    black_box(&block);
                }
            };
            for _ in 0..4 {
                run();
            }
            let mut timings = Vec::with_capacity(7);
            for _ in 0..7 {
                let start = Instant::now();
                run();
                timings.push(start.elapsed().as_nanos());
            }
            timings.sort_unstable();
            println!(
                "mode={mode} rate=48000 block=128 frames=16384 warmup=4 repeats=7 median_ns_per_frame={:.3}",
                timings[3] as f64 / 16384.0
            );
        }
    }
}
