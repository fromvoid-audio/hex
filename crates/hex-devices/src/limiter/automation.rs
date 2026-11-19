//! Nine value controls use their existing owned history and latency. Lookahead
//! or routing eligibility selects the bounded structural factory at fixed latency.
//! Sample-rate input gain mappings use the same 5 ms gain response as authored
//! automation, before input enters the delayed limiting stages.
use super::{
    decode,
    runtime::Factory,
    state::{self, Settings, parameter::*},
};
use crate::parameter_binding::{BOOLEAN_ENVELOPE, Binding, SCALAR_ENVELOPE};
use hex_lattice::{
    DevicePreparationContext, PreparedProcessorFactory, PreparedRuntimeDevice, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, NativeDeviceState, ParameterDescriptor, ParameterId, ParameterModulation,
    ParameterNormalization, ParameterSemantics, ParameterSmoothing,
};
use std::sync::Arc;
pub(super) const COUNT: usize = 9;
pub(super) const BINDINGS: [Binding<Settings>; COUNT] = [
    Binding::float(
        state::DESCRIPTORS[0],
        ParameterSemantics {
            modulation: ParameterModulation::SampleRate,
            smoothing: ParameterSmoothing::DeviceManaged,
            ..SCALAR_ENVELOPE
        },
        |s| s.value(GAIN),
        |s, v| s.0[0] = v,
    ),
    Binding::float(
        state::DESCRIPTORS[1],
        SCALAR_ENVELOPE,
        |s| s.value(CEILING),
        |s, v| s.0[1] = v,
    ),
    Binding::float(
        state::DESCRIPTORS[2],
        ParameterSemantics {
            normalization: ParameterNormalization::PositiveLogarithmic,
            ..SCALAR_ENVELOPE
        },
        |s| s.value(RELEASE),
        |s, v| s.0[2] = v,
    ),
    Binding::boolean(
        state::DESCRIPTORS[3],
        BOOLEAN_ENVELOPE,
        |s| s.flag(AUTO_RELEASE),
        |s, v| s.0[3] = f32::from(v),
    ),
    Binding::float(
        state::DESCRIPTORS[4],
        SCALAR_ENVELOPE,
        |s| s.value(LINK),
        |s, v| s.0[4] = v,
    ),
    Binding::boolean(
        state::DESCRIPTORS[8],
        BOOLEAN_ENVELOPE,
        |s| s.flag(MAXIMIZE),
        |s, v| s.0[8] = f32::from(v),
    ),
    Binding::float(
        state::DESCRIPTORS[9],
        ParameterSemantics {
            smoothing: ParameterSmoothing::DeviceManaged,
            ..SCALAR_ENVELOPE
        },
        |s| s.value(THRESHOLD),
        |s, v| s.0[9] = v,
    ),
    Binding::float(
        state::DESCRIPTORS[10],
        ParameterSemantics {
            smoothing: ParameterSmoothing::DeviceManaged,
            ..SCALAR_ENVELOPE
        },
        |s| s.value(OUTPUT),
        |s, v| s.0[10] = v,
    ),
    Binding::float(
        state::DESCRIPTORS[11],
        SCALAR_ENVELOPE,
        |s| s.value(LINK_MID_SIDE),
        |s, v| s.0[11] = v,
    ),
];
pub(super) fn slot(id: ParameterId) -> Option<usize> {
    BINDINGS.binary_search_by_key(&id, |b| b.descriptor.id).ok()
}
pub(super) fn descriptor(id: ParameterId) -> Option<ParameterDescriptor> {
    slot(id).map(|i| BINDINGS[i].descriptor)
}
pub(super) fn semantics(id: ParameterId) -> Option<ParameterSemantics> {
    if matches!(id, LOOKAHEAD | ROUTING | MODE) {
        super::runtime::structural::semantics(id)
    } else {
        slot(id).map(|i| BINDINGS[i].semantics)
    }
}
pub(super) fn modulation_semantics(id: ParameterId) -> Option<ParameterSemantics> {
    semantics(id).filter(|semantics| semantics.modulation == ParameterModulation::SampleRate)
}

pub(super) fn prepare_fixed_factory(
    state: &NativeDeviceState,
    rate: u32,
    ids: &[ParameterId],
) -> Result<Factory, DeviceContractError> {
    if !(8000..=192000).contains(&rate) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    if ids.windows(2).any(|w| w[0] >= w[1]) {
        return Err(DeviceContractError::UnsupportedProcessingClass);
    }
    // Match the registry: targeted automation validates asset-free metadata;
    // the default factory decodes state, with source admission owned by the host.
    if !ids.is_empty() {
        state.require_asset_free_processing()?;
    }
    let mut admitted = [false; COUNT];
    for &id in ids {
        admitted[slot(id).ok_or(DeviceContractError::UnsupportedProcessingClass)?] = true;
    }
    Ok(Factory::automated(decode(state)?, rate, admitted))
}

pub(super) fn prepare(
    state: &NativeDeviceState,
    context: &DevicePreparationContext<'_>,
    ids: &[ParameterId],
) -> Result<PreparedRuntimeDevice, DeviceContractError> {
    state.require_asset_free_processing()?;
    if !(8000..=192000).contains(&context.sample_rate) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    if ids.is_empty() || ids.windows(2).any(|w| w[0] >= w[1]) {
        return Err(DeviceContractError::UnsupportedProcessingClass);
    }
    if ids
        .iter()
        .any(|&id| !matches!(id, LOOKAHEAD | ROUTING | MODE) && slot(id).is_none())
    {
        return Err(DeviceContractError::UnsupportedProcessingClass);
    }
    if ids
        .iter()
        .any(|id| matches!(*id, LOOKAHEAD | ROUTING | MODE))
    {
        return Ok(PreparedRuntimeDevice {
            matrix: StereoMatrix::identity(),
            processor: Some(PreparedProcessorFactory::Owned(Arc::new(
                super::LimiterDefinition::prepare_owned_structural_for_parameters(
                    state,
                    context.sample_rate,
                    ids,
                )?,
            ))),
        });
    }
    Ok(PreparedRuntimeDevice {
        matrix: StereoMatrix::identity(),
        processor: Some(PreparedProcessorFactory::Owned(Arc::new(
            prepare_fixed_factory(state, context.sample_rate, ids)?,
        ))),
    })
}

#[cfg(test)]
mod tests {
    use super::super::DEFINITION;
    use super::*;
    use hex_lattice::{
        EMPTY_DEVICE_RESOURCES as R, NativeDeviceDefinition, NativeParameterAutomation,
        OwnedProcessorRuntime, ProcessorChainOwner, ProcessorInstanceIdentity, TuningTable,
    };
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    pub(super) fn state(changes: &[(ParameterId, ParameterValue)]) -> NativeDeviceState {
        let mut s = DEFINITION.default_state().unwrap();
        for &(id, v) in changes {
            s = DEFINITION.patch_parameter(&s, id, v).unwrap();
        }
        s
    }
    pub(super) fn runtime(
        s: &NativeDeviceState,
        rate: u32,
        ids: Option<&[ParameterId]>,
    ) -> Box<dyn OwnedProcessorRuntime> {
        let t = TuningTable::default();
        let c = DevicePreparationContext {
            sample_rate: rate,
            tuning: &t,
            resources: &R,
        };
        let f = if let Some(ids) = ids {
            DEFINITION
                .prepare_automated_runtime_instance(
                    ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(1), 1).unwrap(),
                    s,
                    &c,
                    ids,
                )
                .unwrap()
                .processor
                .unwrap()
        } else {
            DEFINITION.prepare_runtime_factory(s, &c).unwrap().unwrap()
        };
        let PreparedProcessorFactory::Owned(f) = f else {
            panic!("owned limiter")
        };
        f.create_runtime().unwrap()
    }
    struct Values {
        start: usize,
        held: Option<[f32; 3]>,
        missing: bool,
        bad: bool,
    }
    impl NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[GAIN, CEILING, RELEASE]
        }
        fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
            if self.missing {
                return None;
            }
            let i = self.start + offset;
            if self.bad && i == 17 && id == RELEASE {
                return Some(ParameterValue::Float(f32::NAN));
            }
            let a = self.held.unwrap_or([
                -12.0 + (i % 41) as f32 * 0.6,
                -24.0 + (i % 29) as f32 * 0.8,
                0.1 + (i % 31) as f32 * 10.0,
            ]);
            Some(ParameterValue::Float(a[id.value() as usize - 1]))
        }
    }
    fn values(held: Option<[f32; 3]>) -> Values {
        Values {
            start: 0,
            held,
            missing: false,
            bad: false,
        }
    }
    pub(super) fn plain(rt: &mut dyn OwnedProcessorRuntime, input: &[[f32; 2]]) -> Vec<[f32; 2]> {
        let mut out = input.to_vec();
        rt.begin_block();
        rt.process_block(&mut out, &R);
        rt.commit_block();
        out
    }
    fn auto(
        rt: &mut dyn OwnedProcessorRuntime,
        input: &[[f32; 2]],
        v: &mut Values,
        width: usize,
    ) -> Vec<[f32; 2]> {
        let mut out = input.to_vec();
        for (i, b) in out.chunks_mut(width).enumerate() {
            v.start = i * width;
            rt.begin_block();
            rt.process_block_with_automation(b, None, v, &R).unwrap();
            rt.commit_block();
        }
        out
    }
    #[test]
    fn preparation_admits_only_the_sorted_requested_union() {
        let base = state(&[]);
        let tuning = TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: 8000,
            tuning: &tuning,
            resources: &R,
        };
        for ids in [
            &[][..],
            &[GAIN, GAIN],
            &[CEILING, GAIN],
            &[ParameterId::from_static(13)],
            &[LOOKAHEAD, ParameterId::from_static(13)],
        ] {
            assert!(prepare(&base, &context, ids).is_err());
        }
        for id in [
            GAIN,
            CEILING,
            RELEASE,
            AUTO_RELEASE,
            LINK,
            LOOKAHEAD,
            ROUTING,
            MODE,
            MAXIMIZE,
            THRESHOLD,
            OUTPUT,
            LINK_MID_SIDE,
        ] {
            let rt = runtime(&base, 8000, Some(&[id]));
            for candidate in [
                GAIN,
                CEILING,
                RELEASE,
                AUTO_RELEASE,
                LINK,
                LOOKAHEAD,
                ROUTING,
                MODE,
                MAXIMIZE,
                THRESHOLD,
                OUTPUT,
                LINK_MID_SIDE,
            ] {
                assert_eq!(rt.supports_parameter_automation(candidate), candidate == id);
            }
        }
        for raw in 1..=13 {
            let id = ParameterId::new(raw).unwrap();
            assert_eq!(
                DEFINITION.supports_parameter_automation(&base, id).unwrap(),
                raw <= 12
            );
        }
        let look = DEFINITION
            .parameter_semantics_for_state(&base, LOOKAHEAD)
            .unwrap()
            .unwrap();
        assert_eq!(
            look.change_impact,
            hex_lattice_contract::ParameterChangeImpact::Structural
        );
        assert_eq!(look.smoothing, ParameterSmoothing::DeviceManaged);
    }
    #[test]
    fn held_scalar_targets_match_canonical_edits_in_all_modes_and_routes() {
        for rate in [8000, 96000] {
            for mode in 0..3 {
                for routing in 0..2 {
                    for (id, target) in [(GAIN, 6.0), (CEILING, -18.0), (RELEASE, 10.0)] {
                        let base = state(&[
                            (MODE, ParameterValue::Choice(mode)),
                            (ROUTING, ParameterValue::Choice(routing)),
                        ]);
                        let mut old = runtime(&base, rate, None);
                        plain(old.as_mut(), &[[2.0, -1.0]; 512]);
                        let mut targets = [0.0, -0.3, 100.0];
                        targets[id.value() as usize - 1] = target;
                        let changed = DEFINITION
                            .patch_parameter(&base, id, ParameterValue::Float(target))
                            .unwrap();
                        let mut actual = runtime(&base, rate, Some(&[GAIN, CEILING, RELEASE]));
                        actual.inherit_from(old.as_ref());
                        let mut expected = runtime(&changed, rate, None);
                        expected.inherit_from(old.as_ref());
                        let input = [[0.1, -0.05]; 1024];
                        let out = auto(actual.as_mut(), &input, &mut values(Some(targets)), 13);
                        assert_eq!(
                            out,
                            plain(expected.as_mut(), &input),
                            "rate{rate} mode{mode} route{routing} id{id:?}"
                        );
                        assert!(
                            out.iter()
                                .zip(plain(old.as_mut(), &input))
                                .any(|(a, b)| a.iter().zip(b).any(|(a, b)| (a - b).abs() > 1e-7)),
                            "inactive{id:?} rate{rate} mode{mode} route{routing}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn dense_gain_matches_independent_native_ramp_delay_and_uneven_blocks() {
        let rate = 8000;
        let base = state(&[]);
        let mut a = runtime(&base, rate, Some(&[GAIN, CEILING, RELEASE]));
        let mut b = runtime(&base, rate, Some(&[GAIN, CEILING, RELEASE]));
        let input = [[0.001, -0.0005]; 1024];
        let out = auto(a.as_mut(), &input, &mut values(None), 1024);
        assert_eq!(out, auto(b.as_mut(), &input, &mut values(None), 13));
        let delay = (1.5_f64 * f64::from(rate) / 1000.0).ceil() as usize;
        let mut ring = vec![[0.0; 2]; delay];
        let mut gain = 1.0;
        let v = values(None);
        for (i, frame) in out.iter().enumerate() {
            let Some(ParameterValue::Float(db)) = v.value_at(GAIN, i) else {
                panic!("gain")
            };
            gain += (10.0_f64.powf(f64::from(db) / 20.0) - gain) / (0.005 * f64::from(rate));
            let expected = ring[i % delay].map(|x| x as f32);
            ring[i % delay] = input[i].map(|x| f64::from(x) * gain);
            assert_eq!(*frame, expected, "frame{i}");
        }
    }
    #[test]
    fn refusal_fallback_shadowed_base_bypass_and_reset_preserve_cache_and_meter() {
        let rate = 8000;
        let base = state(&[]);
        let ids = [GAIN, CEILING, RELEASE];
        let input = [[1.5, -0.5]; 257];
        let mut a = runtime(&base, rate, Some(&ids));
        let mut b = runtime(&base, rate, Some(&ids));
        assert_eq!(
            auto(a.as_mut(), &input, &mut values(None), 13),
            auto(b.as_mut(), &input, &mut values(None), 13)
        );
        assert_eq!(a.take_dynamics_meter(), b.take_dynamics_meter());
        let mut bad = values(None);
        bad.bad = true;
        a.begin_block();
        let mut rejected = input;
        assert!(
            a.process_block_with_automation(&mut rejected, None, &bad, &R)
                .is_err()
        );
        a.rollback_block();
        assert_eq!(a.take_dynamics_meter(), b.take_dynamics_meter());
        assert_eq!(
            auto(a.as_mut(), &input, &mut values(None), 13),
            auto(b.as_mut(), &input, &mut values(None), 13)
        );
        assert_eq!(a.take_dynamics_meter(), b.take_dynamics_meter());
        let changed = state(&[
            (GAIN, ParameterValue::Float(-6.0)),
            (CEILING, ParameterValue::Float(-12.0)),
            (RELEASE, ParameterValue::Float(1000.0)),
        ]);
        let mut next = runtime(&changed, rate, Some(&ids));
        next.inherit_from(a.as_ref());
        let mut stable = runtime(&base, rate, Some(&ids));
        stable.inherit_from(a.as_ref());
        assert_eq!(
            auto(
                next.as_mut(),
                &input,
                &mut values(Some([6.0, -3.0, 10.0])),
                13
            ),
            auto(
                stable.as_mut(),
                &input,
                &mut values(Some([6.0, -3.0, 10.0])),
                13
            )
        );
        let mut expected = runtime(&changed, rate, None);
        expected.inherit_from(next.as_ref());
        let mut missing = values(None);
        missing.missing = true;
        assert_eq!(
            auto(next.as_mut(), &input, &mut missing, 13),
            plain(expected.as_mut(), &input)
        );
        let mut reference = runtime(&changed, rate, Some(&ids));
        reference.inherit_from(next.as_ref());
        let mut dry = input;
        next.begin_block();
        next.process_bypassed_block_with_automation(
            &mut dry,
            None,
            &values(Some([3.0, -9.0, 0.1])),
            &R,
        )
        .unwrap();
        next.commit_block();
        assert_eq!(dry, input);
        auto(
            reference.as_mut(),
            &input,
            &mut values(Some([3.0, -9.0, 0.1])),
            257,
        );
        assert_eq!(
            auto(next.as_mut(), &input, &mut values(None), 13),
            auto(reference.as_mut(), &input, &mut values(None), 13)
        );
        next.reset();
        let mut fresh = runtime(&changed, rate, Some(&ids));
        assert_eq!(
            auto(next.as_mut(), &input, &mut values(None), 13),
            auto(fresh.as_mut(), &input, &mut values(None), 13)
        );
    }
    #[test]
    fn detached_queued_output_keeps_last_authored_targets() {
        let base = state(&[]);
        let ids = [GAIN, CEILING, RELEASE];
        let mut tail = runtime(&base, 8000, Some(&ids));
        auto(
            tail.as_mut(),
            &[[1.0, -0.5]; 64],
            &mut values(Some([-6.0, -18.0, 10.0])),
            64,
        );
        let mut live = runtime(&base, 8000, Some(&ids));
        live.inherit_from(tail.as_ref());
        tail.begin_output_tail();
        assert_eq!(
            plain(tail.as_mut(), &[[0.0; 2]; 257]),
            auto(
                live.as_mut(),
                &[[0.0; 2]; 257],
                &mut values(Some([-6.0, -18.0, 10.0])),
                257
            )
        );
    }
}

#[cfg(test)]
mod control_tests;
