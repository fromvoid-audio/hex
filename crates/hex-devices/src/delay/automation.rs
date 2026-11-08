//! Shared feedback/mix targets live in the owned committed/speculative scalar.
//! Admission/base stay with the incoming factory; cached targets follow history.
//! Rollback restores both DSP and control caches. Missing views use canonical
//! targets; detached tails keep their last targets and existing Freeze release.
use super::*;
use crate::parameter_binding::{Binding, SCALAR_ENVELOPE};
use hex_lattice_contract::{
    ParameterDescriptor, ParameterId, ParameterSemantics, ParameterSmoothing,
};

pub(super) const COUNT: usize = 2;
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Mix {
    pub feedback: f32,
    pub mix: f32,
}
impl Mix {
    pub(super) fn new(settings: Settings) -> Self {
        Self {
            feedback: settings.value(FEEDBACK),
            mix: settings.value(DRY_WET),
        }
    }
}
const SEMANTICS: ParameterSemantics = ParameterSemantics {
    smoothing: ParameterSmoothing::DeviceManaged,
    ..SCALAR_ENVELOPE
};
pub(super) const BINDINGS: [Binding<Mix>; COUNT] = [
    Binding::float(
        state::DESCRIPTORS[1],
        SEMANTICS,
        |s| s.feedback,
        |s, v| s.feedback = v,
    ),
    Binding::float(
        state::DESCRIPTORS[2],
        SEMANTICS,
        |s| s.mix,
        |s, v| s.mix = v,
    ),
];
pub(super) fn slot(id: ParameterId) -> Option<usize> {
    BINDINGS
        .binary_search_by_key(&id, |binding| binding.descriptor.id)
        .ok()
}
pub(super) fn descriptor(id: ParameterId) -> Option<ParameterDescriptor> {
    slot(id).map(|i| BINDINGS[i].descriptor)
}
pub(super) fn semantics(id: ParameterId) -> Option<ParameterSemantics> {
    slot(id).map(|i| BINDINGS[i].semantics)
}
pub(super) fn prepare(
    state: &NativeDeviceState,
    context: &hex_lattice::DevicePreparationContext<'_>,
    ids: &[ParameterId],
) -> Result<hex_lattice::PreparedRuntimeDevice, DeviceContractError> {
    state.require_asset_free_processing()?;
    if !(8000..=192000).contains(&context.sample_rate) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    if ids.is_empty() || ids.windows(2).any(|p| p[0] >= p[1]) {
        return Err(DeviceContractError::UnsupportedProcessingClass);
    }
    let mut admitted = [false; COUNT];
    for &id in ids {
        admitted[slot(id).ok_or(DeviceContractError::UnsupportedProcessingClass)?] = true;
    }
    Ok(hex_lattice::PreparedRuntimeDevice {
        matrix: StereoMatrix::identity(),
        processor: Some(PreparedProcessorFactory::Owned(Arc::new(Factory {
            settings: decode(state)?,
            rate: context.sample_rate,
            admitted,
        }))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{
        DevicePreparationContext, EMPTY_DEVICE_RESOURCES as R, NativeParameterAutomation,
        ProcessorChainOwner, ProcessorInstanceIdentity, TuningTable,
    };
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    fn state(feedback: f32, mix: f32) -> NativeDeviceState {
        state::encode(Settings::from_legacy([1.0, feedback, mix])).unwrap()
    }
    fn runtime(
        state: &NativeDeviceState,
        rate: u32,
        ids: Option<&[ParameterId]>,
    ) -> Box<dyn OwnedProcessorRuntime> {
        let tuning = TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: rate,
            tuning: &tuning,
            resources: &R,
        };
        let factory = if let Some(ids) = ids {
            DEFINITION
                .prepare_automated_runtime_instance(
                    ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(1), 1).unwrap(),
                    state,
                    &context,
                    ids,
                )
                .unwrap()
                .processor
                .unwrap()
        } else {
            DEFINITION
                .prepare_runtime_factory(state, &context)
                .unwrap()
                .unwrap()
        };
        let PreparedProcessorFactory::Owned(factory) = factory else {
            panic!("owned delay")
        };
        factory.create_runtime().unwrap()
    }
    struct Values {
        start: usize,
        held: Option<[f32; 2]>,
        missing: bool,
        bad: Option<usize>,
    }
    impl NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[FEEDBACK, DRY_WET]
        }
        fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
            if self.missing {
                return None;
            }
            let i = self.start + offset;
            if self.bad == Some(i) && id == FEEDBACK {
                return Some(ParameterValue::Float(f32::NAN));
            }
            let pair = self
                .held
                .unwrap_or([0.2 + (i % 37) as f32 / 36.0 * 0.6, (i % 43) as f32 / 42.0]);
            Some(ParameterValue::Float(pair[usize::from(id == DRY_WET)]))
        }
    }
    fn values(held: Option<[f32; 2]>) -> Values {
        Values {
            start: 0,
            held,
            missing: false,
            bad: None,
        }
    }
    fn signal(n: usize) -> Vec<StereoFrame> {
        (0..n)
            .map(|i| {
                if i % 31 < 7 {
                    [0.25, -0.125]
                } else {
                    [0.01, -0.02]
                }
            })
            .collect()
    }
    fn auto(
        rt: &mut dyn OwnedProcessorRuntime,
        input: &[StereoFrame],
        v: &mut Values,
        width: usize,
    ) -> Vec<StereoFrame> {
        let mut out = input.to_vec();
        for (i, block) in out.chunks_mut(width).enumerate() {
            v.start = i * width;
            rt.begin_block();
            rt.process_block_with_automation(block, None, v, &R)
                .unwrap();
            rt.commit_block();
        }
        out
    }
    fn plain(rt: &mut dyn OwnedProcessorRuntime, input: &[StereoFrame]) -> Vec<StereoFrame> {
        let mut out = input.to_vec();
        rt.begin_block();
        rt.process_block(&mut out, &R);
        rt.commit_block();
        out
    }
    #[test]
    fn held_targets_match_canonical_and_shadowed_base_edits_preserve_the_ramp() {
        for rate in [8000, 96000] {
            let base = state(0.5, 0.4);
            let ids = [FEEDBACK, DRY_WET];
            let mut old = runtime(&base, rate, None);
            plain(old.as_mut(), &signal(257));
            let mut actual = runtime(&base, rate, Some(&ids));
            actual.inherit_from(old.as_ref());
            let mut canonical = runtime(&state(0.8, 0.2), rate, None);
            canonical.inherit_from(old.as_ref());
            assert_eq!(
                auto(
                    actual.as_mut(),
                    &signal(1024),
                    &mut values(Some([0.8, 0.2])),
                    13
                ),
                plain(canonical.as_mut(), &signal(1024))
            );
            let mut source = runtime(&base, rate, Some(&ids));
            auto(
                source.as_mut(),
                &signal(7),
                &mut values(Some([0.8, 0.2])),
                7,
            );
            let changed = state(0.3, 0.9);
            let mut a = runtime(&changed, rate, Some(&ids));
            a.inherit_from(source.as_ref());
            let mut b = runtime(&base, rate, Some(&ids));
            b.inherit_from(source.as_ref());
            assert_eq!(
                auto(a.as_mut(), &signal(31), &mut values(Some([0.8, 0.2])), 31),
                auto(b.as_mut(), &signal(31), &mut values(Some([0.8, 0.2])), 13)
            );
            let mut absent = runtime(&changed, rate, Some(&ids));
            absent.inherit_from(a.as_ref());
            let mut canonical = runtime(&changed, rate, None);
            canonical.inherit_from(a.as_ref());
            assert!(!canonical.supports_parameter_automation(DRY_WET));
            let mut missing = values(None);
            missing.missing = true;
            assert_eq!(
                auto(absent.as_mut(), &signal(1024), &mut missing, 13),
                plain(canonical.as_mut(), &signal(1024))
            );
        }
    }
    #[test]
    fn dense_targets_match_an_independent_recurrence_and_uneven_blocks() {
        for rate in [8000, 96000] {
            let base = state(0.5, 0.4);
            let ids = [FEEDBACK, DRY_WET];
            let mut whole = runtime(&base, rate, Some(&ids));
            let mut split = runtime(&base, rate, Some(&ids));
            let input = signal(2048);
            let out = auto(whole.as_mut(), &input, &mut values(None), input.len());
            assert_eq!(out, auto(split.as_mut(), &input, &mut values(None), 13));
            let lag = rate as usize / 1000;
            let mut ring = vec![[0.0_f32; 2]; lag];
            let (mut feedback, mut mix) = (0.5_f64, 0.4_f64);
            let n = f64::from(rate.div_ceil(100));
            let v = values(None);
            for (i, frame) in out.iter().enumerate() {
                let Some(ParameterValue::Float(f)) = v.value_at(FEEDBACK, i) else {
                    panic!("feedback")
                };
                let Some(ParameterValue::Float(m)) = v.value_at(DRY_WET, i) else {
                    panic!("mix")
                };
                feedback += (f64::from(f) - feedback) / n;
                mix += (f64::from(m) - mix) / n;
                let wet = ring[i % lag];
                for c in 0..2 {
                    let expected =
                        (f64::from(input[i][c]) * (1.0 - mix) + f64::from(wet[c]) * mix) as f32;
                    assert!((frame[c] - expected).abs() < 2e-7, "rate {rate}, frame {i}");
                    ring[i % lag][c] =
                        (f64::from(input[i][c]) + f64::from(wet[c]) * feedback) as f32;
                }
            }
        }
    }
    #[test]
    fn rollback_wrap_bypass_tails_donation_and_timing_fork_keep_cache_custody() {
        let rate = 8000;
        let base = state(0.5, 0.4);
        let ids = [FEEDBACK, DRY_WET];
        let mut source = runtime(&base, rate, Some(&ids));
        // Place the cursor four frames before the actual sixteen-second ring wraps.
        auto(
            source.as_mut(),
            &vec![[0.0; 2]; rate as usize * 16],
            &mut values(Some([0.7, 0.8])),
            4096,
        );
        let mut reference = runtime(&base, rate, Some(&ids));
        reference.inherit_from(source.as_ref());
        let mut bad = values(None);
        bad.bad = Some(17);
        source.begin_block();
        assert!(
            source
                .process_block_with_automation(&mut signal(31), None, &bad, &R)
                .is_err()
        );
        source.rollback_block();
        assert_eq!(
            auto(source.as_mut(), &signal(73), &mut values(None), 73),
            auto(reference.as_mut(), &signal(73), &mut values(None), 13)
        );
        let mut bypassed = runtime(&base, rate, Some(&ids));
        bypassed.inherit_from(source.as_ref());
        let mut copy = runtime(&base, rate, Some(&ids));
        copy.inherit_from(source.as_ref());
        let mut dry = signal(17);
        let original = dry.clone();
        bypassed.begin_block();
        bypassed
            .process_bypassed_block_with_automation(&mut dry, None, &values(Some([0.7, 0.8])), &R)
            .unwrap();
        bypassed.commit_block();
        assert_eq!(dry, original);
        auto(copy.as_mut(), &original, &mut values(Some([0.7, 0.8])), 17);
        assert_eq!(
            auto(
                bypassed.as_mut(),
                &signal(97),
                &mut values(Some([0.7, 0.8])),
                13
            ),
            auto(
                copy.as_mut(),
                &signal(97),
                &mut values(Some([0.7, 0.8])),
                97
            )
        );
        let mut donated = runtime(&base, rate, Some(&ids));
        donated.transfer_history_from(bypassed.as_mut());
        assert!(bypassed.committed_is_quiescent());
        let mut live = runtime(&base, rate, Some(&ids));
        live.inherit_from(donated.as_ref());
        donated.begin_output_tail();
        assert_eq!(
            plain(donated.as_mut(), &vec![[0.0; 2]; 2048]),
            auto(
                live.as_mut(),
                &vec![[0.0; 2]; 2048],
                &mut values(Some([0.7, 0.8])),
                13
            )
        );
        let mut silent = runtime(&base, rate, Some(&ids));
        auto(
            silent.as_mut(),
            &vec![[0.0; 2]; 80],
            &mut values(Some([0.7, 0.8])),
            80,
        );
        let mut fork = runtime(&base, rate, None);
        assert!(fork.fork_timing_without_signal_history_from(silent.as_ref()));
        assert!(!fork.supports_parameter_automation(DRY_WET));
        let out = plain(fork.as_mut(), &[[0.25, -0.125]]);
        let m = 0.8_f64 + (0.4 - 0.8) / 80.0;
        assert!((out[0][0] - (0.25 * (1.0 - m)) as f32).abs() < 1e-7);
        fork.reset();
        assert!(fork.committed_is_quiescent());
    }
    #[test]
    #[ignore = "release-only targeted delivery microbenchmark"]
    fn delay_mix_delivery_cost() {
        use std::{hint::black_box, time::Instant};
        assert!(!black_box(cfg!(debug_assertions)), "run with --release");
        for kind in 0..3 {
            let mut rt = runtime(
                &state(0.5, 0.4),
                48000,
                (kind != 0).then_some(&[FEEDBACK, DRY_WET][..]),
            );
            let mut v = values((kind == 1).then_some([0.7, 0.8]));
            let mut block = [[0.125, -0.25]; 128];
            let mut run = || {
                for i in 0..128 {
                    block.fill([0.125, -0.25]);
                    v.start = i * 128;
                    rt.begin_block();
                    if kind == 0 {
                        rt.process_block(&mut block, &R);
                    } else {
                        rt.process_block_with_automation(&mut block, None, &v, &R)
                            .unwrap();
                    }
                    rt.commit_block();
                    black_box(&block);
                }
            };
            for _ in 0..4 {
                run();
            }
            let mut samples = [0.0_f64; 7];
            for sample in &mut samples {
                let t = Instant::now();
                run();
                *sample = t.elapsed().as_nanos() as f64 / 16384.0;
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "mode={kind} rate=48000 block=128 frames=16384 warmup=4 repeats=7 median_ns_per_frame={:.3}",
                samples[3]
            );
        }
    }
}
