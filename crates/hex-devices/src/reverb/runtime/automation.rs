//! Borrowed output, delay and modulation controls. Authored values bypass
//! canonical edit ramps; room controls use the existing tank evaluation rate.
//! No feedback coefficient, ring capacity or tail-bound changes occur here.
use super::*;
use crate::parameter_binding::{Binding, SCALAR_ENVELOPE};
use hex_lattice_contract::ParameterSemantics;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct AutomatedSettings {
    dry_wet: f32,
    predelay: f32,
    spin_amount: f32,
    spin_rate: f32,
    shape: f32,
    scale: f32,
    chorus_amount: f32,
    chorus_rate: f32,
    stereo: f32,
    reflect: f32,
    diffuse: f32,
}
impl AutomatedSettings {
    pub(super) fn new(settings: Settings) -> Self {
        Self {
            dry_wet: settings.get(p::DRY_WET),
            predelay: settings.get(p::PREDELAY),
            spin_amount: settings.get(p::SPIN_AMOUNT),
            spin_rate: settings.get(p::SPIN_RATE),
            shape: settings.get(p::SHAPE),
            scale: settings.get(p::SCALE),
            chorus_amount: settings.get(p::CHORUS_AMOUNT),
            chorus_rate: settings.get(p::CHORUS_RATE),
            stereo: settings.get(p::STEREO),
            reflect: settings.get(p::REFLECT),
            diffuse: settings.get(p::DIFFUSE),
        }
    }
    pub(super) fn check_inline(&self) {
        let Self {
            dry_wet,
            predelay,
            spin_amount,
            spin_rate,
            shape,
            scale,
            chorus_amount,
            chorus_rate,
            stereo,
            reflect,
            diffuse,
        } = self;
        let _: [&f32; 11] = [
            dry_wet,
            predelay,
            spin_amount,
            spin_rate,
            shape,
            scale,
            chorus_amount,
            chorus_rate,
            stereo,
            reflect,
            diffuse,
        ];
    }
}
const SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: hex_lattice_contract::ParameterSmoothing::DeviceManaged,
    ..SCALAR_ENVELOPE
};
const LOG_SEMANTICS: ParameterSemantics = ParameterSemantics {
    normalization: hex_lattice_contract::ParameterNormalization::PositiveLogarithmic,
    ..SEMANTICS
};
const BINDINGS: [Binding<AutomatedSettings>; 11] = [
    Binding::float(
        crate::reverb::DESCRIPTORS[1],
        SEMANTICS,
        |s| s.dry_wet,
        |s, v| s.dry_wet = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[6],
        SEMANTICS,
        |s| s.predelay,
        |s, v| s.predelay = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[8],
        SEMANTICS,
        |s| s.spin_amount,
        |s, v| s.spin_amount = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[9],
        LOG_SEMANTICS,
        |s| s.spin_rate,
        |s, v| s.spin_rate = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[10],
        SEMANTICS,
        |s| s.shape,
        |s, v| s.shape = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[18],
        SEMANTICS,
        |s| s.scale,
        |s, v| s.scale = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[21],
        SEMANTICS,
        |s| s.chorus_amount,
        |s, v| s.chorus_amount = v,
    ),
    Binding::float(
        crate::reverb::DESCRIPTORS[22],
        LOG_SEMANTICS,
        |s| s.chorus_rate,
        |s, v| s.chorus_rate = v,
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
pub(super) struct AutomatedValues {
    pub dry_wet: Option<f32>,
    pub predelay: Option<f32>,
    pub spin_amount: Option<f32>,
    pub spin_rate: Option<f32>,
    pub shape: Option<f32>,
    pub scale: Option<f32>,
    pub chorus_amount: Option<f32>,
    pub chorus_rate: Option<f32>,
    pub stereo: Option<f32>,
    pub reflect: Option<f32>,
    pub diffuse: Option<f32>,
}
impl AutomatedValues {
    pub(super) fn check_inline(&self) {
        let Self {
            dry_wet,
            predelay,
            spin_amount,
            spin_rate,
            shape,
            scale,
            chorus_amount,
            chorus_rate,
            stereo,
            reflect,
            diffuse,
        } = self;
        let _: [&Option<f32>; 11] = [
            dry_wet,
            predelay,
            spin_amount,
            spin_rate,
            shape,
            scale,
            chorus_amount,
            chorus_rate,
            stereo,
            reflect,
            diffuse,
        ];
    }
}
impl ReverbRuntime {
    pub(super) fn process_automation(
        &mut self,
        frames: &mut [StereoFrame],
        values: &dyn NativeParameterAutomation,
    ) -> Result<(), DeviceContractError> {
        let mut resolved = self.automation_controls.resolve(&BINDINGS, values)?;
        for (offset, frame) in frames.iter_mut().enumerate() {
            if let Some(next) =
                self.automation_controls
                    .sample(&BINDINGS, &mut resolved, values, offset)?
            {
                let settings = next.effective();
                let old = self.automation_controls.effective();
                // Installed presence distinguishes authored values from the
                // canonical ramp fallback without another producer query.
                self.automation_controls.install(next);
                let present = |index| self.automation_controls.has_value(index);
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
                let output = AutomatedValues {
                    dry_wet: present(0).then_some(settings.dry_wet),
                    predelay: present(1).then_some(settings.predelay),
                    spin_amount: present(2).then_some(settings.spin_amount),
                    spin_rate: present(3).then_some(settings.spin_rate),
                    shape: present(4).then_some(settings.shape),
                    scale: present(5).then_some(settings.scale),
                    chorus_amount: present(6).then_some(settings.chorus_amount),
                    chorus_rate: present(7).then_some(settings.chorus_rate),
                    stereo: present(8).then_some(settings.stereo),
                    reflect: gain(
                        9,
                        settings.reflect,
                        old.reflect,
                        self.automated_values.reflect,
                    ),
                    diffuse: gain(
                        10,
                        settings.diffuse,
                        old.diffuse,
                        self.automated_values.diffuse,
                    ),
                };
                self.automated_values = output;
            }
            self.frame_outputs(frame, self.automated_values);
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
                (p::PREDELAY, 50.0),
                (p::SPIN_AMOUNT, 80.0),
                (p::SPIN_RATE, 2.0),
                (p::SHAPE, 0.1),
                (p::SCALE, 90.0),
                (p::CHORUS_AMOUNT, 3.0),
                (p::CHORUS_RATE, 5.0),
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
                    actual.process_automation(block, &input).unwrap();
                }
                assert_eq!(output, expected, "parameter {} at {rate}", id.value());
                assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
            }
        }
    }
    #[test]
    fn dense_controls_match_one_sample_canonical_edits_with_retained_phase() {
        struct Dense {
            ids: Vec<ParameterId>,
        }
        impl NativeParameterAutomation for Dense {
            fn parameter_ids(&self) -> &[ParameterId] {
                &self.ids
            }
            fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
                let binding = BINDINGS.iter().find(|b| b.descriptor.id == id)?;
                let hex_lattice_contract::ParameterKind::Float { min, max, .. } =
                    binding.descriptor.kind
                else {
                    return None;
                };
                let fraction = ((offset * 17 + id.value() as usize * 7) % 65) as f32 / 64.0;
                Some(ParameterValue::Float(min + (max - min) * fraction))
            }
        }
        let values = Dense {
            ids: BINDINGS.iter().map(|b| b.descriptor.id).collect(),
        };
        for rate in [8000, 96000] {
            let mut settings = Settings::default_values();
            settings.0[crate::reverb::index(p::SMOOTH).unwrap()] = 0.0;
            let prepared = PreparedReverb::new(rate, settings).unwrap();
            let mut actual = prepared.runtime();
            actual.process_block(&mut signal(8193), &R);
            let mut reference = prepared.runtime();
            reference.copy_history_from(&actual);
            let mut output = signal(128);
            let mut expected = output.clone();
            actual.process_automation(&mut output, &values).unwrap();
            for (offset, frame) in expected.iter_mut().enumerate() {
                let mut changed = settings;
                for id in values.parameter_ids() {
                    let Some(ParameterValue::Float(value)) = values.value_at(*id, offset) else {
                        panic!("float source");
                    };
                    changed.0[crate::reverb::index(*id).unwrap()] = value;
                }
                // Public canonical handoff law with one-sample smoothing is
                // an independent delivery oracle; no authored cache is used.
                let mut next = PreparedReverb::new(rate, changed).unwrap().runtime();
                next.copy_history_from(&reference);
                next.process_block(std::slice::from_mut(frame), &R);
                reference = next;
            }
            for (offset, (a, b)) in output.iter().zip(&expected).enumerate() {
                assert!(
                    (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6,
                    "rate {rate} frame {offset}: {a:?} != {b:?}"
                );
            }
            assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
        }
    }
    #[test]
    fn delay_and_modulation_endpoints_match_static_rooms() {
        for binding in &BINDINGS[1..8] {
            let hex_lattice_contract::ParameterKind::Float { min, max, .. } =
                binding.descriptor.kind
            else {
                panic!("float control");
            };
            for value in [min, max] {
                let settings = Settings::default_values();
                let mut changed = settings;
                changed.0[crate::reverb::index(binding.descriptor.id).unwrap()] = value;
                let mut actual = PreparedReverb::new(8000, settings).unwrap().runtime();
                let mut reference = PreparedReverb::new(8000, changed).unwrap().runtime();
                let values = Values {
                    ids: vec![binding.descriptor.id],
                    values: vec![Some(ParameterValue::Float(value))],
                };
                let mut output = signal(8192);
                let mut expected = output.clone();
                actual.process_automation(&mut output, &values).unwrap();
                reference.process_block(&mut expected, &R);
                assert_eq!(
                    output,
                    expected,
                    "{} endpoint {value}",
                    binding.descriptor.id.value()
                );
                assert!(
                    output
                        .iter()
                        .flatten()
                        .all(|v| v.is_finite() && v.abs() <= 16.0)
                );
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
            ids: BINDINGS.iter().map(|b| b.descriptor.id).collect(),
            values: vec![
                100.0,
                f32::NAN,
                80.0,
                2.0,
                0.1,
                90.0,
                3.0,
                5.0,
                0.0,
                -12.0,
                -12.0,
            ]
            .into_iter()
            .map(|v| Some(ParameterValue::Float(v)))
            .collect(),
        };
        let mut output = [[0.125; 2]; 17];
        assert!(source.process_automation(&mut output, &values).is_err());
        assert_eq!(output, [[0.125; 2]; 17]);
        values.values[1] = Some(ParameterValue::Float(25.0));
        let mut expected = output;
        source.process_automation(&mut output, &values).unwrap();
        reference
            .process_automation(&mut expected, &values)
            .unwrap();
        assert_eq!(output, expected);
        let mut copied = prepared.runtime();
        copied.copy_history_from(&source);
        let mut bypassed = prepared.runtime();
        bypassed.copy_history_from(&source);
        output.fill([0.125; 2]);
        expected = output;
        source.process_automation(&mut output, &values).unwrap();
        copied.process_automation(&mut expected, &values).unwrap();
        assert_eq!(output, expected);
        expected.fill([0.125; 2]);
        bypassed
            .process_bypassed_block_with_automation(&mut expected, None, &values, &R)
            .unwrap();
        assert_eq!(expected, [[0.125; 2]; 17]);
        output.fill([0.125; 2]);
        source.process_automation(&mut output, &values).unwrap();
        bypassed.process_automation(&mut expected, &values).unwrap();
        assert_eq!(output, expected);
        copied.reset();
        reference.reset();
        let absent = Values {
            ids: vec![],
            values: vec![],
        };
        output.fill([0.125; 2]);
        expected = output;
        copied.process_automation(&mut output, &absent).unwrap();
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
    #[test]
    fn clock_free_tail_holds_last_validated_automation_controls() {
        for rate in [8000, 96000] {
            let prepared = PreparedReverb::new(rate, Settings::default_values()).unwrap();
            let mut source = prepared.runtime();
            let values = Values {
                ids: BINDINGS.iter().map(|b| b.descriptor.id).collect(),
                values: vec![
                    Some(ParameterValue::Float(100.0)),
                    Some(ParameterValue::Float(50.0)),
                    Some(ParameterValue::Float(80.0)),
                    Some(ParameterValue::Float(2.0)),
                    Some(ParameterValue::Float(0.1)),
                    Some(ParameterValue::Float(90.0)),
                    Some(ParameterValue::Float(3.0)),
                    Some(ParameterValue::Float(5.0)),
                    Some(ParameterValue::Float(0.0)),
                    Some(ParameterValue::Float(-12.0)),
                    Some(ParameterValue::Float(-12.0)),
                ],
            };
            // Fill beyond the authored 50ms predelay even at 96kHz.
            source
                .process_automation(&mut signal(8193), &values)
                .unwrap();
            let mut reference = prepared.runtime();
            reference.copy_history_from(&source);
            source.begin_output_tail();
            let mut output = vec![[0.0; 2]; 513];
            let mut expected = output.clone();
            for block in output.chunks_mut(13) {
                source.process_block(block, &R);
            }
            reference
                .process_automation(&mut expected, &values)
                .unwrap();
            for (index, (a, b)) in output.iter().zip(&expected).enumerate() {
                for ch in 0..2 {
                    assert!(
                        (a[ch] - b[ch]).abs() < 1e-7,
                        "clock-free tail rate {rate} frame {index}: {a:?} != {b:?}"
                    );
                }
            }
            assert!(output.iter().flatten().any(|v| v.abs() > 1e-6));
            // A new active graph inherits DSP history, not tail admission or
            // the detached graph's override mode after lane removal.
            let mut canonical = prepared.runtime();
            canonical.copy_history_from(&source);
            let mut canonical_reference = prepared.runtime();
            canonical_reference.copy_history_from(&reference);
            output.fill([0.0; 2]);
            expected.fill([0.0; 2]);
            canonical.process_block(&mut output, &R);
            canonical_reference.process_block(&mut expected, &R);
            assert_eq!(output, expected);
            source.reset();
            output.fill([0.0; 2]);
            source.process_block(&mut output, &R);
            assert!(output.iter().all(|frame| *frame == [0.0; 2]));
        }
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
                p::PREDELAY => 250.0 * t,
                p::SPIN_AMOUNT => 100.0 * t,
                p::SPIN_RATE | p::CHORUS_RATE => 0.01 + 9.99 * t,
                p::SHAPE => t,
                p::SCALE => 100.0 * t,
                p::CHORUS_AMOUNT => 4.0 * t,
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
        assert!(!black_box(cfg!(debug_assertions)), "run with --release");
        for mode in 0..4 {
            let source = Source {
                ids: if mode < 2 {
                    vec![p::DRY_WET]
                } else {
                    BINDINGS.iter().map(|b| b.descriptor.id).collect()
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
                        runtime.process_automation(&mut block, &source).unwrap();
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
