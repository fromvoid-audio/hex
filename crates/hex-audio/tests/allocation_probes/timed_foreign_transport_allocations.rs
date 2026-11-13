//! Borrowed foreign instrument replay on the calling thread, without a native device.
use hex_engine::{
    Frame, PlaybackCommand, PreparedRevision, SineTable, TimedTransportCommand, playback::Playback,
};
use hex_lattice::{
    DevicePreparationContext, ForeignProcessorNode, ForeignProcessors, NativeDeviceDefinition,
    NativeDeviceRegistry, NativeInstrumentClock, PreparedForeignProcessor,
    PreparedProcessorFactory, ProcessorChainOwner, ProcessorInstanceIdentity, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, ForeignParameterPoint, ForeignProcessorBinding,
    NativeDeviceMetadata, NativeDeviceState, ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

const TYPE: DeviceTypeId = DeviceTypeId::from_static("hex.vst3");
struct Definition(ProcessorInstanceIdentity);
impl NativeDeviceMetadata for Definition {
    fn type_id(&self) -> DeviceTypeId {
        TYPE
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(TYPE, 1, vec![19])
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
impl NativeDeviceDefinition for Definition {
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
        if identity != self.0 || context.sample_rate != 48_000 {
            return Err(DeviceContractError::InvalidId);
        }
        let facts = PreparedForeignProcessor::new(8, 3, 16)?
            .with_binding(ForeignProcessorBinding {
                root: 1,
                instance: identity.device_id(),
                graph: 1,
                epoch: 1,
            })?
            .with_instrument_profile();
        Ok(Some(PreparedProcessorFactory::Foreign(facts)))
    }
}
#[derive(Default)]
struct Endpoint {
    next: usize,
    events: usize,
    rejected: bool,
}
impl ForeignProcessors for Endpoint {
    fn process_segment(
        &mut self,
        _: ForeignProcessorNode<'_>,
        _: usize,
        _: &mut [Frame],
        _: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }
    fn process_segment_with_notes(
        &mut self,
        _: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [Frame],
        _: &[ForeignParameterPoint],
        notes: &[hex_lattice::ForeignNote],
        _: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        assert_eq!(offset, self.next);
        self.next += frames.len();
        self.events += notes.len();
        frames.fill([0.25; 2]);
        Ok(())
    }
    fn reject_callback(&mut self) {
        self.rejected = true;
    }
}

#[test]
fn timed_foreign_replay_does_not_allocate_or_free() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "instrument".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: NativeDeviceState::new(TYPE, 1, vec![19]).unwrap(),
        })
        .unwrap();
    let device = editor.project().track(track).unwrap().instrument().unwrap();
    let identity = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Track(track.value()),
        device.id().value(),
    )
    .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 2,
            velocity: 101,
        })
        .unwrap();
    let definition = Definition(identity);
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_effect_registry(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
            &registry,
        )
        .unwrap(),
    );
    let mut playback = Playback::from_prepared(revision);
    let commands = [
        TimedTransportCommand {
            offset: 0,
            command: PlaybackCommand::PlayFrom(0),
        },
        TimedTransportCommand {
            offset: 5,
            command: PlaybackCommand::PlayFrom(0),
        },
        TimedTransportCommand {
            offset: 10,
            command: PlaybackCommand::Pause,
        },
    ];
    let mut endpoint = Endpoint::default();
    let mut output = [[0.0; 2]; 16];
    let mut result = Ok(());
    let counts = allocation_counter::measure(|| {
        result = playback.render_with_timed_transport_and_foreign_processors(
            &mut output,
            &commands,
            &mut endpoint,
        );
    });
    assert!(result.is_ok());
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(!endpoint.rejected);
    assert_eq!(endpoint.events, 4);
}
