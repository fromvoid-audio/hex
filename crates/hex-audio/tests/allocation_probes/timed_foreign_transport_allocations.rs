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

fn instrument_fixture() -> (Editor, ProcessorInstanceIdentity) {
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
    (editor, identity)
}

#[test]
fn timed_foreign_replay_does_not_allocate_or_free() {
    let (editor, identity) = instrument_fixture();
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

#[test]
fn timed_mixed_stream_and_foreign_replay_does_not_allocate_or_free() {
    use hex_engine::{
        BorrowedMediaPage, MediaPages, STREAM_PAGE_FRAMES, StreamSourceKey, StreamedSourceMetadata,
        compile::PreparedAudioSource,
        native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets},
    };
    use std::sync::atomic::AtomicBool;
    struct Pages {
        key: StreamSourceKey,
        data: Vec<Frame>,
        requests: usize,
    }
    impl MediaPages for Pages {
        fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
            if key != self.key {
                return None;
            }
            let start = usize::try_from(index)
                .ok()?
                .checked_mul(STREAM_PAGE_FRAMES)?;
            let end = start.checked_add(STREAM_PAGE_FRAMES)?.min(self.data.len());
            (start < end).then_some(BorrowedMediaPage {
                page_index: index,
                frames: &self.data[start..end],
            })
        }
        fn request_page(&mut self, _: StreamSourceKey, _: u64) {
            self.requests += 1;
        }
    }
    let (mut editor, identity) = instrument_fixture();
    let Created::Imported { asset, track, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "stream".into(),
            path: "stream.wav".into(),
            sample_rate: 48_000,
            frames: 8192,
        })
        .unwrap()
    else {
        panic!("stream")
    };
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap(),
        })
        .unwrap();
    let key = StreamSourceKey::new(23, asset);
    let source =
        StreamedSourceMetadata::new(key, 48_000, 48_000, 8192, 8192, 0, [0x48; 32]).unwrap();
    let media = BTreeMap::from([(asset, PreparedAudioSource::Streamed(Arc::new(source)))]);
    let cancelled = AtomicBool::new(false);
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        editor.project(),
        1_000_000,
        &cancelled,
    )
    .unwrap();
    let definition = Definition(identity);
    let definitions: [&dyn NativeDeviceDefinition; 2] =
        [&definition, &hex_devices::utility::UTILITY_DEFINITION];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources_device_resources_and_effect_registry(
            editor.project(),
            &media,
            23,
            Arc::new(SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            100_000,
            &cancelled,
            &registry,
            None,
        )
        .unwrap(),
    );
    let mut playback =
        Playback::try_from_prepared_with_device_resources(revision, &resources).unwrap();
    let mut pages = Pages {
        key,
        data: vec![[0.1; 2]; 8192],
        requests: 0,
    };
    let commands = [
        TimedTransportCommand {
            offset: 0,
            command: PlaybackCommand::PlayFrom(0),
        },
        TimedTransportCommand {
            offset: 32,
            command: PlaybackCommand::PlayFrom(4093),
        },
        TimedTransportCommand {
            offset: 64,
            command: PlaybackCommand::Pause,
        },
    ];
    let mut endpoint = Endpoint::default();
    let mut output = [[0.0; 2]; 80];
    let mut node_calls = 0;
    let mut project_calls = 0;
    let mut timing_calls = 0;
    let mut node = |_, _, _, _: &[Frame]| node_calls += 1;
    let mut project = |_, _, _: &[Frame]| project_calls += 1;
    let mut timing = |_, _, _, _: &hex_engine::PreparedTempoMap| timing_calls += 1;
    let tracks = [track.value()];
    let mut tap = hex_engine::ProjectOutputTap::new(&tracks, &mut project)
        .unwrap()
        .with_timing(&mut timing);
    let mut result = Ok(());
    let counts = allocation_counter::measure(|| {
        result = playback.render_with_timed_transport_foreign_and_signal_observers(
            &mut output,
            &commands,
            Some(&mut pages),
            Some(&mut node),
            Some(&mut tap),
            &mut endpoint,
        );
    });
    assert!(result.is_ok());
    assert!(node_calls > 0 && project_calls > 0 && timing_calls > 0);
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(endpoint.events, 4);
    assert!(!endpoint.rejected);
    assert_eq!(pages.requests, 0);
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
}
