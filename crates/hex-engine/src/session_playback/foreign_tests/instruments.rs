//! Control-side preparation of an Arrangement VST3 instrument source.
use super::*;
use crate::Invalid;
use hex_project::{BeatTime, Editor, Project};

const VST3: DeviceTypeId = DeviceTypeId::from_static("hex.vst3");

struct InstrumentDefinition {
    identity: ProcessorInstanceIdentity,
    instrument_profile: bool,
}

impl NativeDeviceMetadata for InstrumentDefinition {
    fn type_id(&self) -> DeviceTypeId {
        VST3
    }

    fn state_version(&self) -> u32 {
        1
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(VST3, 1, vec![19])
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state != &self.default_state()? {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(state.clone())
    }

    fn read_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }

    fn patch_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
        _: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
}

impl NativeDeviceDefinition for InstrumentDefinition {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        self.migrate_state(state)?;
        Ok(StereoMatrix::identity())
    }

    fn prepare_instance_runtime_factory(
        &self,
        identity: ProcessorInstanceIdentity,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        self.migrate_state(state)?;
        if identity != self.identity || context.sample_rate != 48_000 {
            return Err(DeviceContractError::InvalidId);
        }
        let facts = PreparedForeignProcessor::new(8, 3, 16)?.with_binding(
            hex_lattice_contract::ForeignProcessorBinding {
                root: 1,
                instance: identity.device_id(),
                graph: 1,
                epoch: 1,
            },
        )?;
        let facts = if self.instrument_profile {
            facts.with_instrument_profile()
        } else {
            facts
        };
        Ok(Some(PreparedProcessorFactory::Foreign(facts)))
    }
}

#[test]
fn vst3_instrument_prepares_exact_generator_and_authored_notes() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "synth".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: NativeDeviceState::new(VST3, 1, vec![19]).unwrap(),
        })
        .unwrap();
    let device = editor.project().track(track).unwrap().instrument().unwrap();
    let owner = ProcessorChainOwner::Track(track.value());
    let identity = ProcessorInstanceIdentity::new(owner, device.id().value()).unwrap();
    let beat = BeatTime::from_ratio(1, 1).unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beat,
        })
        .unwrap()
    else {
        panic!("clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat,
            pitch: 60,
            channel: 2,
            velocity: 101,
        })
        .unwrap();
    let prepare = |instrument_profile| {
        let definition = InstrumentDefinition {
            identity,
            instrument_profile,
        };
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        PreparedRevision::prepare_with_effect_registry(
            editor.project(),
            &std::collections::BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
            &registry,
        )
    };
    assert!(matches!(
        prepare(false),
        Err(Invalid(
            "vst3 instrument worker binding or processing role differs"
        ))
    ));
    let revision = Arc::new(prepare(true).unwrap());
    let track = &revision.arrangement().tracks()[0];
    let node = track.foreign_instrument.as_ref().unwrap();
    assert_eq!(node.owner, owner);
    assert_eq!(node.device_id, identity.device_id());
    assert_eq!(node.algorithmic_latency_frames(), Some(19));
    assert_eq!(track.instrument_latency, Some(19));
    let mut notes = Vec::new();
    track
        .for_each_mutable_midi_span(0, 1, |span| {
            notes.push((
                span.start,
                span.gate_end,
                span.channel,
                span.pitch,
                span.velocity,
            ));
        })
        .unwrap();
    assert_eq!(notes, [(0, 24_000, 2, 60, 101)]);
    assert!(matches!(
        SessionPlayback::new(revision),
        Err(SessionPlaybackError::MutableInstrumentDiagnostic(
            "vst3 instrument render owner is unavailable"
        ))
    ));
}
