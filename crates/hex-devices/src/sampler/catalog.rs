//! Registered sampler preparation. Empty is analytic silence; loaded is mutable
//! borrowed-PCM processing whose exact resource custody remains with the host.
use hex_lattice::{NativeInstrumentDefinition, PreparedAnalyticNote, PreparedNativeInstrument};
use hex_lattice_contract::{
    DeviceContractError as Error, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterAccess, ParameterAutomation, ParameterChangeImpact, ParameterDescriptor, ParameterId,
    ParameterKind, ParameterModulation, ParameterNormalization, ParameterPresentation,
    ParameterSemantics, ParameterSmoothing, ParameterStepping, ParameterValue,
};

/// Existing authored linear gain, before envelope and occurrence output gain.
pub const GAIN: ParameterId = ParameterId::from_static(1);
pub(super) const GAIN_DESCRIPTORS: [ParameterDescriptor; 1] = [ParameterDescriptor {
    id: GAIN,
    label: "gain",
    kind: ParameterKind::Float {
        min: 0.0,
        max: 4.0,
        unit: None,
    },
    default: ParameterValue::Float(1.0),
    presentation: ParameterPresentation::Linear,
}];

pub(super) fn gain_semantics(parameter: ParameterId) -> Option<ParameterSemantics> {
    (parameter == GAIN).then_some(ParameterSemantics {
        access: ParameterAccess::ReadWrite,
        normalization: ParameterNormalization::Linear,
        stepping: ParameterStepping::Continuous,
        automation: ParameterAutomation::Unsupported,
        modulation: ParameterModulation::SampleRate,
        smoothing: ParameterSmoothing::None,
        change_impact: ParameterChangeImpact::ValueOnly,
    })
}

pub static DEFINITION: SamplerDefinition = SamplerDefinition;
pub struct SamplerDefinition;
impl NativeDeviceMetadata for SamplerDefinition {
    fn type_id(&self) -> DeviceTypeId {
        super::state::TYPE_ID
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("sampler")
    }
    fn state_version(&self) -> u32 {
        super::state::STATE_VERSION
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &GAIN_DESCRIPTORS
    }
    fn descriptors_for_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<Vec<ParameterDescriptor>, Error> {
        if super::state::is_empty_state(state) {
            return Ok(Vec::new());
        }
        super::state::SamplerState::from_native(state)?;
        Ok(self.descriptors().to_vec())
    }
    fn parameter_semantics_for_state(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<Option<ParameterSemantics>, Error> {
        super::state::SamplerState::from_native(state)?;
        Ok(gain_semantics(parameter))
    }
    fn default_state(&self) -> Result<NativeDeviceState, Error> {
        super::state::empty_state()
    }
    fn migrate_state(&self, state: &NativeDeviceState) -> Result<NativeDeviceState, Error> {
        // Saved v1/v2 states (including the saved v2 empty marker) re-encode at
        // the current version in memory, keeping host gains and the source
        // binding. Projects keep their saved form; see `state::saved_form`.
        if super::state::is_empty_state(state) {
            if state.state_version() == super::state::STATE_VERSION {
                return Ok(state.clone());
            }
            return super::state::current_empty_state()?.with_host_metadata_from(state);
        }
        let authored = super::state::SamplerState::from_native(state)?;
        if state.state_version() == super::state::STATE_VERSION {
            return Ok(state.clone());
        }
        authored.to_current_native()?.with_host_metadata_from(state)
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, Error> {
        if parameter != GAIN {
            return Err(Error::UnknownParameter);
        }
        let authored = super::state::SamplerState::from_native(state)?;
        Ok(ParameterValue::Float(authored.gain as f32))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, Error> {
        if parameter != GAIN {
            return Err(Error::UnknownParameter);
        }
        GAIN_DESCRIPTORS[0].validate(value)?;
        let ParameterValue::Float(gain) = value else {
            return Err(Error::InvalidParameterValue);
        };
        let mut authored = super::state::SamplerState::from_native(state)?;
        authored.gain = f64::from(gain);
        authored.replace_in(state)
    }
}
impl NativeInstrumentDefinition for SamplerDefinition {
    fn resource_factory_inline_bound(&self) -> Option<usize> {
        Some(std::mem::size_of::<super::PreparedSamplerRuntime>())
    }
    fn supports_asset_resources(&self) -> bool {
        true
    }
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, Error> {
        prepare(state, context, super::runtime::MAX_EVENTS)
    }
    fn prepare_instrument_with_runtime_limits(
        &self,
        state: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
        limits: hex_lattice::NativeInstrumentRuntimeLimits,
    ) -> Result<Box<dyn PreparedNativeInstrument>, Error> {
        prepare(state, context, limits.max_events_per_block)
    }
}
fn prepare(
    state: &NativeDeviceState,
    context: &hex_lattice::DevicePreparationContext<'_>,
    max_events: usize,
) -> Result<Box<dyn PreparedNativeInstrument>, Error> {
    if !(1..=super::runtime::MAX_EVENTS).contains(&max_events)
        || !(8_000..=192_000).contains(&context.sample_rate)
    {
        return Err(Error::InvalidParameterValue);
    }
    if super::state::is_empty_state(state) {
        if !context.resources.is_empty() {
            return Err(Error::AssetResourcesUnavailable);
        }
        Ok(Box::new(EmptySampler))
    } else {
        Ok(Box::new(PreparedSamplerInstrument(
            super::state::prepare_runtime_on_control(state, context, max_events)?
                .with_sample_rate_gain(),
        )))
    }
}
/// Scalar instrument adapter shared by registry preparation and the obsolete private host path.
/// It owns no PCM or source lease and grants no independent registry/publication admission.
pub struct PreparedSamplerInstrument(super::PreparedSamplerRuntime);
impl PreparedSamplerInstrument {
    pub fn from_runtime_on_control(runtime: super::PreparedSamplerRuntime) -> Self {
        Self(runtime)
    }
}
impl PreparedNativeInstrument for PreparedSamplerInstrument {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, Error> {
        Err(Error::UnsupportedProcessingClass)
    }
    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>>, Error> {
        Ok(Some(Box::new(self.0.clone())))
    }
}
struct EmptySampler;
impl PreparedNativeInstrument for EmptySampler {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn prepare_note(&self, pitch: u8, velocity: u8) -> Result<PreparedAnalyticNote, Error> {
        if pitch > 127 || velocity > 127 {
            return Err(Error::InvalidParameterValue);
        }
        Ok(PreparedAnalyticNote::new(
            [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
            0,
            silence,
        ))
    }
}
fn silence(
    _: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
    _: u64,
    _: u64,
    _: u64,
    _: &dyn hex_lattice::DeviceResources,
) -> hex_lattice::StereoFrame {
    [0.0; 2]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_default_is_explicitly_empty_silent_and_never_substitutes_a_missing_source() {
        let registry = crate::native_instrument_registry().unwrap();
        let state = registry
            .default_state(super::super::state::TYPE_ID)
            .unwrap();
        assert!(super::super::state::is_empty_state(&state));
        assert!(state.asset_references().is_empty());
        assert!(super::super::state::SamplerState::from_native(&state).is_err());
        let tuning = hex_lattice::TuningTable::default();
        let context = hex_lattice::DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &tuning,
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        };
        let instrument = registry.prepare_instrument(&state, &context).unwrap();
        assert_eq!(instrument.algorithmic_latency_frames(), Some(0));
        let note = instrument.prepare_note(60, 127).unwrap();
        assert_eq!(note.release_frames(), 0);
        for frame in [0, 1, 48000, u64::MAX] {
            assert_eq!(note.sample(frame, 16, context.resources), [0.0; 2]);
        }
        let unbound = super::super::state::SamplerState::for_source_frames(4)
            .unwrap()
            .to_native()
            .unwrap();
        assert!(!super::super::state::is_empty_state(&unbound));
        assert!(registry.prepare_instrument(&unbound, &context).is_err());
        for (version, payload) in [
            (super::super::state::STATE_VERSION, b"empty!".as_slice()),
            (1, b"empty".as_slice()),
        ] {
            let malformed =
                NativeDeviceState::new(super::super::state::TYPE_ID, version, payload.to_vec())
                    .unwrap();
            assert!(!super::super::state::is_empty_state(&malformed));
            assert!(registry.prepare_instrument(&malformed, &context).is_err());
            assert!(registry.migrate_state(&malformed).is_err());
        }
        let bound_marker = state
            .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
                slot: hex_lattice_contract::DeviceAssetSlot::new(1).unwrap(),
                asset: hex_lattice_contract::DeviceAssetKey::new(1).unwrap(),
            }])
            .unwrap();
        assert!(!super::super::state::is_empty_state(&bound_marker));
        assert!(
            registry
                .prepare_instrument(&bound_marker, &context)
                .is_err()
        );
    }
    #[test]
    fn registered_loaded_sampler_uses_exact_original_rate_pcm_and_refuses_unleased_bindings() {
        use hex_lattice::{
            BorrowedDeviceResources, DevicePreparationContext, DeviceResourceIdentity,
            DeviceResourcePcm, DeviceResourceView, NativeInstrumentEvent,
            NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
        };
        use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
        let registry = crate::native_instrument_registry().unwrap();
        let slot = DeviceAssetSlot::new(1).unwrap();
        let asset = DeviceAssetKey::new(9).unwrap();
        let state = super::super::state::SamplerState::for_source_frames(4)
            .unwrap()
            .to_native()
            .unwrap()
            .with_asset_references(vec![DeviceAssetReference { slot, asset }])
            .unwrap();
        let pcm = [0.25, 0.75, -0.25, 0.0];
        let view = DeviceResourceView {
            slot,
            identity: DeviceResourceIdentity {
                asset,
                content: [2; 32],
                derivation_version: 1,
                derivation: [3; 32],
            },
            sample_rate: 8000,
            pcm: DeviceResourcePcm::Mono(&pcm),
        };
        let views = [view];
        let resources = BorrowedDeviceResources(&views);
        let tuning = hex_lattice::TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: 16000,
            tuning: &tuning,
            resources: &resources,
        };
        let host_limits = NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 32,
            max_tail_frames: 16000 * 600,
        };
        let prepared = registry
            .prepare_instrument_with_runtime_limits(&state, &context, host_limits)
            .unwrap();
        assert!(prepared.prepare_note(60, 127).is_err());
        assert!(
            registry
                .prepare_instrument_with_runtime_limits(
                    &state,
                    &context,
                    NativeInstrumentRuntimeLimits {
                        max_events_per_block: 0,
                        ..host_limits
                    }
                )
                .is_err()
        );
        let small = registry
            .prepare_instrument_with_runtime_limits(
                &state,
                &context,
                NativeInstrumentRuntimeLimits {
                    max_events_per_block: 1,
                    ..host_limits
                },
            )
            .unwrap();
        assert_eq!(
            small
                .prepare_mutable_runtime()
                .unwrap()
                .unwrap()
                .max_events_per_block(),
            1
        );
        let standalone = registry.prepare_instrument(&state, &context).unwrap();
        assert_eq!(
            standalone
                .prepare_mutable_runtime()
                .unwrap()
                .unwrap()
                .max_events_per_block(),
            super::super::runtime::MAX_EVENTS
        );
        let factory = prepared.prepare_mutable_runtime().unwrap().unwrap();
        let mut pair = NativeInstrumentRuntimePair::new(
            factory.as_ref(),
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: host_limits.max_events_per_block,
                max_active_occurrences: 32,
                max_tail_frames: 16000 * 600,
            },
        )
        .unwrap();
        let mut output = [[0.0; 2]; 8];
        pair.begin_block();
        pair.process_block(
            &[NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 127,
            }],
            &mut output,
            &resources,
        )
        .unwrap();
        pair.commit_block();
        assert_eq!(
            output,
            [0.25, 0.5, 0.75, 0.25, -0.25, -0.125, 0.0, 0.0].map(|v| [v; 2])
        );
        let empty = DevicePreparationContext {
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            ..context
        };
        assert!(registry.prepare_instrument(&state, &empty).is_err());
        let bad_views = [DeviceResourceView {
            identity: DeviceResourceIdentity {
                asset: DeviceAssetKey::new(10).unwrap(),
                ..view.identity
            },
            ..view
        }];
        let bad = BorrowedDeviceResources(&bad_views);
        assert!(
            registry
                .prepare_instrument(
                    &state,
                    &DevicePreparationContext {
                        resources: &bad,
                        ..context
                    }
                )
                .is_err()
        );
        // An ordinary definition refuses bindings before trying to migrate the
        // deliberately unsupported authored version or executing its provider.
        let ordinary = NativeDeviceState::new(crate::SINE_INSTRUMENT_TYPE_ID, 999, vec![])
            .unwrap()
            .with_asset_references(state.asset_references().to_vec())
            .unwrap();
        assert!(matches!(
            registry.prepare_instrument(&ordinary, &context),
            Err(Error::AssetResourcesUnavailable)
        ));
    }
}
