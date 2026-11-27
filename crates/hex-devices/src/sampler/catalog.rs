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
/// Stereo linear balance (-1.0 left ..= 1.0 right).
pub const PAN: ParameterId = ParameterId::from_static(2);
/// Semitone pitch transpose (-48.0 ..= 48.0 st).
pub const TRANSPOSE: ParameterId = ParameterId::from_static(3);
/// Fine pitch detune (-100.0 ..= 100.0 ct).
pub const DETUNE: ParameterId = ParameterId::from_static(4);
/// Normalized sample playback start position (0.0 ..= 1.0).
pub const SAMPLE_START: ParameterId = ParameterId::from_static(5);
/// Normalized sample playback end position (0.0 ..= 1.0).
pub const SAMPLE_END: ParameterId = ParameterId::from_static(6);
/// Normalized Classic loop start within the playback region (0.0 ..= 1.0).
pub const LOOP_START: ParameterId = ParameterId::from_static(7);
/// Normalized Classic loop length within the playback region (0.0 ..= 1.0).
pub const LOOP_LENGTH: ParameterId = ParameterId::from_static(8);
/// Envelope attack duration in seconds (0.0 ..= 60.0 s).
pub const ATTACK: ParameterId = ParameterId::from_static(9);
/// Envelope decay duration in seconds (0.0 ..= 60.0 s).
pub const DECAY: ParameterId = ParameterId::from_static(10);
/// Envelope sustain level (0.0 ..= 1.0).
pub const SUSTAIN: ParameterId = ParameterId::from_static(11);
/// Envelope release duration in seconds (0.0 ..= 60.0 s).
pub const RELEASE: ParameterId = ParameterId::from_static(12);

pub(super) const PARAMETER_COUNT: usize = 12;

pub(super) const DESCRIPTORS: [ParameterDescriptor; PARAMETER_COUNT] = [
    ParameterDescriptor {
        id: GAIN,
        label: "gain",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 4.0,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: PAN,
        label: "pan",
        kind: ParameterKind::Float {
            min: -1.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: TRANSPOSE,
        label: "transpose",
        kind: ParameterKind::Float {
            min: -48.0,
            max: 48.0,
            unit: Some("st"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: DETUNE,
        label: "detune",
        kind: ParameterKind::Float {
            min: -100.0,
            max: 100.0,
            unit: Some("ct"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: SAMPLE_START,
        label: "sample start",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: SAMPLE_END,
        label: "sample end",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LOOP_START,
        label: "loop start",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: LOOP_LENGTH,
        label: "loop length",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: ATTACK,
        label: "attack",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 60.0,
            unit: Some("s"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: DECAY,
        label: "decay",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 60.0,
            unit: Some("s"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: SUSTAIN,
        label: "sustain",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: RELEASE,
        label: "release",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 60.0,
            unit: Some("s"),
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
];

pub(super) const fn parameter_index(parameter: ParameterId) -> Option<usize> {
    match parameter.value() {
        1..=12 => Some((parameter.value() - 1) as usize),
        _ => None,
    }
}

pub(super) fn parameter_semantics(parameter: ParameterId) -> Option<ParameterSemantics> {
    parameter_index(parameter).map(|_| ParameterSemantics {
        access: ParameterAccess::ReadWrite,
        normalization: ParameterNormalization::Linear,
        stepping: ParameterStepping::Continuous,
        automation: ParameterAutomation::DeviceArrangementEnvelope,
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
        &DESCRIPTORS
    }
    fn ports(&self) -> &'static [hex_lattice_contract::DevicePortDescriptor] {
        hex_lattice_contract::STEREO_INSTRUMENT_PORTS
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
        Ok(parameter_semantics(parameter))
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
        let authored = super::state::SamplerState::from_native(state)?;
        let extent = sampler_extent(&authored) as f64;
        let region_len = authored
            .region
            .end
            .saturating_sub(authored.region.start)
            .max(1) as f64;
        let value = match parameter {
            GAIN => authored.gain as f32,
            PAN => authored.pan as f32,
            TRANSPOSE => authored.transpose_semitones as f32,
            DETUNE => authored.detune_cents as f32,
            SAMPLE_START => (authored.region.start as f64 / extent).clamp(0.0, 1.0) as f32,
            SAMPLE_END => (authored.region.end as f64 / extent).clamp(0.0, 1.0) as f32,
            LOOP_START => match authored.mode {
                super::state::AuthoredMode::Classic { loop_region, .. } => {
                    (loop_region.start.saturating_sub(authored.region.start) as f64 / region_len)
                        .clamp(0.0, 1.0) as f32
                }
                _ => 0.0,
            },
            LOOP_LENGTH => match authored.mode {
                super::state::AuthoredMode::Classic { loop_region, .. } => {
                    (loop_region.end.saturating_sub(loop_region.start) as f64 / region_len)
                        .clamp(0.0, 1.0) as f32
                }
                _ => 1.0,
            },
            ATTACK => authored.envelope.attack_seconds as f32,
            DECAY => authored.envelope.decay_seconds as f32,
            SUSTAIN => authored.envelope.sustain as f32,
            RELEASE => authored.envelope.release_seconds as f32,
            _ => return Err(Error::UnknownParameter),
        };
        Ok(ParameterValue::Float(value))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, Error> {
        let index = parameter_index(parameter).ok_or(Error::UnknownParameter)?;
        DESCRIPTORS[index].validate(value)?;
        let ParameterValue::Float(raw) = value else {
            return Err(Error::InvalidParameterValue);
        };
        let mut authored = super::state::SamplerState::from_native(state)?;
        let extent = sampler_extent(&authored);
        match parameter {
            GAIN => authored.gain = f64::from(raw),
            PAN => authored.pan = f64::from(raw),
            TRANSPOSE => authored.transpose_semitones = f64::from(raw),
            DETUNE => authored.detune_cents = f64::from(raw),
            SAMPLE_START => {
                let start = ((f64::from(raw) * extent as f64).round() as usize)
                    .min(authored.region.end.saturating_sub(1));
                authored.region.start = start;
                clamp_loop_to_region(&mut authored);
            }
            SAMPLE_END => {
                let end = ((f64::from(raw) * extent as f64).round() as usize)
                    .clamp(authored.region.start.saturating_add(1), extent.max(1));
                authored.region.end = end;
                clamp_loop_to_region(&mut authored);
            }
            LOOP_START => {
                if let super::state::AuthoredMode::Classic {
                    ref mut loop_region,
                    ..
                } = authored.mode
                {
                    let region_len = authored.region.end.saturating_sub(authored.region.start);
                    let loop_len = loop_region.end.saturating_sub(loop_region.start).max(1);
                    let start = authored.region.start
                        + ((f64::from(raw) * region_len as f64).round() as usize)
                            .min(region_len.saturating_sub(1));
                    let end = start.saturating_add(loop_len).min(authored.region.end);
                    loop_region.start = start.min(end.saturating_sub(1));
                    loop_region.end = end.max(loop_region.start.saturating_add(1));
                }
            }
            LOOP_LENGTH => {
                if let super::state::AuthoredMode::Classic {
                    ref mut loop_region,
                    ..
                } = authored.mode
                {
                    let region_len = authored.region.end.saturating_sub(authored.region.start);
                    let desired_len =
                        ((f64::from(raw) * region_len as f64).round() as usize).max(1);
                    let start = loop_region
                        .start
                        .clamp(authored.region.start, authored.region.end.saturating_sub(1));
                    let end = start.saturating_add(desired_len).min(authored.region.end);
                    loop_region.start = start;
                    loop_region.end = end.max(start.saturating_add(1));
                }
            }
            ATTACK => authored.envelope.attack_seconds = f64::from(raw),
            DECAY => authored.envelope.decay_seconds = f64::from(raw),
            SUSTAIN => authored.envelope.sustain = f64::from(raw),
            RELEASE => authored.envelope.release_seconds = f64::from(raw),
            _ => return Err(Error::UnknownParameter),
        }
        authored.replace_in(state)
    }
}

fn sampler_extent(state: &super::state::SamplerState) -> usize {
    let loop_end = match state.mode {
        super::state::AuthoredMode::Classic { loop_region, .. } => loop_region.end,
        _ => 0,
    };
    state.region.end.max(loop_end).max(1)
}

fn clamp_loop_to_region(state: &mut super::state::SamplerState) {
    if let super::state::AuthoredMode::Classic {
        loop_enabled: true,
        ref mut loop_region,
    } = state.mode
    {
        loop_region.start = loop_region
            .start
            .clamp(state.region.start, state.region.end.saturating_sub(1));
        loop_region.end = loop_region
            .end
            .clamp(loop_region.start.saturating_add(1), state.region.end);
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

    #[test]
    fn sampler_core_performance_parameters_roundtrip_and_modulate_runtime() {
        use hex_lattice::{
            BorrowedDeviceResources, DevicePreparationContext, DeviceResourceIdentity,
            DeviceResourcePcm, DeviceResourceView, NativeInstrumentEvent,
            NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair, NativeParameterAutomation,
        };
        use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};

        struct OneParam {
            id: [ParameterId; 1],
            values: Vec<f32>,
        }
        impl NativeParameterAutomation for OneParam {
            fn parameter_ids(&self) -> &[ParameterId] {
                &self.id
            }
            fn value_at(&self, parameter: ParameterId, offset: usize) -> Option<ParameterValue> {
                (parameter == self.id[0])
                    .then(|| self.values.get(offset).copied().map(ParameterValue::Float))
                    .flatten()
            }
        }

        let slot = DeviceAssetSlot::new(1).unwrap();
        let asset = DeviceAssetKey::new(7).unwrap();
        let mut authored = super::super::state::SamplerState::for_source_frames(64).unwrap();
        authored.mode = super::super::state::AuthoredMode::Classic {
            loop_enabled: true,
            loop_region: super::super::Region { start: 8, end: 56 },
        };
        let state = authored
            .to_native()
            .unwrap()
            .with_asset_references(vec![DeviceAssetReference { slot, asset }])
            .unwrap();

        // All 12 core parameters expose DeviceArrangementEnvelope + SampleRate modulation.
        assert_eq!(DEFINITION.descriptors().len(), PARAMETER_COUNT);
        for d in DEFINITION.descriptors() {
            let sem = DEFINITION
                .parameter_semantics_for_state(&state, d.id)
                .unwrap()
                .expect("semantics");
            assert_eq!(
                sem.automation,
                ParameterAutomation::DeviceArrangementEnvelope
            );
            assert_eq!(sem.modulation, ParameterModulation::SampleRate);
        }

        // Patch/read roundtrip preserves asset binding across pitch, region, loop, and ADSR.
        let mut patched = state.clone();
        for (id, value) in [
            (GAIN, 1.5_f32),
            (PAN, -0.5_f32),
            (TRANSPOSE, 12.0_f32),
            (DETUNE, 25.0_f32),
            (SAMPLE_START, 0.25_f32),
            (SAMPLE_END, 0.75_f32),
            (LOOP_START, 0.25_f32),
            (LOOP_LENGTH, 0.5_f32),
            (ATTACK, 0.01_f32),
            (DECAY, 0.02_f32),
            (SUSTAIN, 0.5_f32),
            (RELEASE, 0.05_f32),
        ] {
            patched = DEFINITION
                .patch_parameter(&patched, id, ParameterValue::Float(value))
                .unwrap();
            assert_eq!(patched.asset_references(), state.asset_references());
            let read = DEFINITION.read_parameter(&patched, id).unwrap();
            let ParameterValue::Float(actual) = read else {
                panic!("expected float");
            };
            assert!(actual.is_finite());
        }

        // Runtime sample-rate automation modulates pan, pitch, and sustain cleanly.
        let pcm: Vec<f32> = (0..64)
            .map(|i| ((i as f32) * 0.25).sin() * 0.5 + 0.25)
            .collect();
        let view = DeviceResourceView {
            slot,
            identity: DeviceResourceIdentity {
                asset,
                content: [5; 32],
                derivation_version: 1,
                derivation: [6; 32],
            },
            sample_rate: 48_000,
            pcm: DeviceResourcePcm::Mono(&pcm),
        };
        let views = [view];
        let resources = BorrowedDeviceResources(&views);
        let tuning = hex_lattice::TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &tuning,
            resources: &resources,
        };
        let prepared = DEFINITION.prepare_instrument(&state, &context).unwrap();
        let factory = prepared.prepare_mutable_runtime().unwrap().unwrap();
        assert_eq!(
            factory.parameter_automation_descriptors().len(),
            PARAMETER_COUNT
        );

        let limits = NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: super::super::runtime::MAX_EVENTS,
            max_active_occurrences: 32,
            max_tail_frames: 48_000 * 600,
        };
        let render_with = |id: ParameterId, value: f32| {
            let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();
            let mut out = [[0.0_f32; 2]; 32];
            let events = [NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 127,
            }];
            let auto = OneParam {
                id: [id],
                values: vec![value; 32],
            };
            pair.begin_block();
            pair.process_block_with_clock_and_automation(
                &events,
                &mut out,
                None,
                &[],
                &auto,
                &resources,
            )
            .unwrap();
            pair.commit_block();
            out
        };

        let left_panned = render_with(PAN, -1.0);
        assert!(left_panned.iter().any(|f| f[0].abs() > 0.01));
        assert!(left_panned.iter().all(|f| f[1] == 0.0));

        let pitched_up = render_with(TRANSPOSE, 12.0);
        let unpitched = render_with(TRANSPOSE, 0.0);
        assert_ne!(pitched_up, unpitched);

        let low_sustain = render_with(SUSTAIN, 0.25);
        let full_sustain = render_with(SUSTAIN, 1.0);
        assert_ne!(low_sustain, full_sustain);
    }
}
