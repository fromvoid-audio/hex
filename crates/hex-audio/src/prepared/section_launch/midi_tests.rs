//! Real section publication and copied note transport with a headless worker.
use super::*;
use hex_engine::{LaunchQuantization, PreparedRevision, SessionPlayback, SineTable};
use hex_lattice::{
    DevicePreparationContext, NativeDeviceDefinition, NativeDeviceRegistry,
    PreparedForeignProcessor, PreparedProcessorFactory, ProcessorChainOwner,
    ProcessorInstanceIdentity, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, ForeignProcessorBinding, NativeDeviceMetadata,
    NativeDeviceState, ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_plugin_host::transport::channel as plugin_channel;
use hex_project::{BeatTime, Command, Created, Editor, LoopRegion, Project};

const VST3: DeviceTypeId = DeviceTypeId::from_static("hex.vst3");
const QUANTUM: usize = 8;

struct Instrument(PreparedForeignProcessor);

impl NativeDeviceMetadata for Instrument {
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
        NativeDeviceState::new(VST3, 1, vec![])
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
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

impl NativeDeviceDefinition for Instrument {
    fn prepare_matrix(&self, _: &NativeDeviceState) -> Result<StereoMatrix, DeviceContractError> {
        Ok(StereoMatrix::identity())
    }
    fn prepare_instance_runtime_factory(
        &self,
        _: ProcessorInstanceIdentity,
        _: &NativeDeviceState,
        _: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(PreparedProcessorFactory::Foreign(self.0)))
    }
}

fn queue(control: &mut Control, playback: Playback, revision: u64, policy: PublicationPolicy) {
    // The test owns a fake worker and installs its callback directly. Native
    // constructor/lease admission is covered by the original-worker fixtures.
    control
        .publications
        .push(QueuedPublication::Legacy(Publication {
            pcm_frames: 0,
            forwarded_pcm: false,
            revision,
            control_context_generation: 1,
            explicit_control_context: true,
            source_context: None,
            policy,
            fresh_runtime_cancellation: None,
            section_launch_frame: None,
            streams: None,
            native_references: None,
            playback: Box::new(playback),
        }))
        .unwrap();
}

fn render(callback: &mut Callback) {
    let mut pcm = [[0.0; 2]; QUANTUM];
    let mut run = || {
        callback.begin_block();
        callback.begin_native_effect_callback(QUANTUM).unwrap();
        callback.render(&mut pcm).unwrap();
        assert!(matches!(
            callback.finish_native_effect_callback(true),
            OutputTransportDelivery::Accepted
        ));
    };
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(&mut run);
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    run();
}

#[test]
fn section_jump_releases_original_note_and_stages_destination_at_physical_offset() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "synth".into(),
        })
        .unwrap()
    else {
        panic!("midi track");
    };
    let state = NativeDeviceState::new(VST3, 1, vec![]).unwrap();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: state.clone(),
        })
        .unwrap();
    let target_beat = BeatTime::from_quarters(4).unwrap();
    for (start, duration) in [
        (BeatTime::ZERO, BeatTime::from_quarters(2).unwrap()),
        (target_beat, BeatTime::from_ratio(1, 6_000).unwrap()),
    ] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start,
                length: duration,
            })
            .unwrap()
        else {
            panic!("midi clip");
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration,
                pitch: 60,
                channel: 2,
                velocity: 100,
            })
            .unwrap();
    }
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id()
        .value();
    let binding = ForeignProcessorBinding {
        root: 1,
        instance: device,
        graph: 1,
        epoch: 1,
    };
    let facts = PreparedForeignProcessor::new(QUANTUM as u16, 0, 2 * QUANTUM as u32)
        .unwrap()
        .with_binding(binding)
        .unwrap()
        .with_instrument_profile();
    let definition = Instrument(facts);
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let source = Arc::new(
        PreparedRevision::prepare_with_effect_registry(
            editor.project(),
            &Default::default(),
            Arc::new(SineTable::new()),
            None,
            &registry,
        )
        .unwrap(),
    );
    let identity =
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(track.value()), device).unwrap();
    let region = LoopRegion::new(96_000, 120_000, true).unwrap();
    for (quantization, offset, observe) in [
        (LaunchQuantization::Immediate, 0, false),
        (LaunchQuantization::Quarter, 2, false),
        (LaunchQuantization::Quarter, 2, true),
    ] {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (_plugin_control, transport, mut worker) =
            plugin_channel(binding, QUANTUM as u16).unwrap();
        callback.native_sources.install_for_test(
            crate::plugin_effects::NativeEffectCallback::for_test(
                identity,
                binding,
                state.clone(),
                transport,
            )
            .with_instrument_profile_for_test(),
        );
        let keys = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
        keys.set_key(0, Some((67, 100, 1))).unwrap();
        let mut old = Playback::try_from_prepared_with_session(Arc::clone(&source)).unwrap();
        old.apply(PlaybackCommand::PlayFrom(23_990));
        old.prepare_computer_keyboard(editor.project(), track, Arc::clone(&keys))
            .unwrap();
        queue(&mut control, old, 1, PublicationPolicy::InstallFreshRuntime);
        render(&mut callback);
        let first = worker.receive().unwrap().unwrap();
        let [on, live] = first.events.as_slice() else {
            panic!("one outgoing clip gate and one physical key")
        };
        assert_eq!(on.bytes, [0x92, 60, 100]);
        assert_eq!(live.bytes, [0x90, 67, 100]);
        let old_id = on.note_id.unwrap();
        let live_id = live.note_id.unwrap();
        worker.complete(first).unwrap();
        let mut next = Playback::from_interactive_arrangement_preroll(
            SessionPlayback::prepare_section_arrangement_preroll(
                Arc::clone(&source),
                None,
                region,
                0,
                &AtomicBool::new(false),
            )
            .unwrap(),
        )
        .unwrap();
        next.prepare_computer_keyboard(editor.project(), track, Arc::clone(&keys))
            .unwrap();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        queue(
            &mut control,
            next,
            2,
            PublicationPolicy::InstallSectionWhenReady {
                fence,
                quantization,
            },
        );
        let _take = observe.then(|| control.sample_recording.start(1, &[], true).unwrap());
        render(&mut callback);
        let next = worker.receive().unwrap().unwrap();
        let [release, onset, end] = next.events.as_slice() else {
            panic!(
                "outgoing release followed by destination gate, got {:?}",
                next.events.as_slice()
            )
        };
        assert_eq!(
            (release.frame, release.bytes, release.note_id),
            (offset, [0x82, 60, 0], Some(old_id))
        );
        assert_eq!((onset.frame, onset.bytes), (offset, [0x92, 60, 100]));
        let new_id = onset.note_id.unwrap();
        assert_ne!(old_id, new_id);
        assert_eq!(
            (end.frame, end.bytes, end.note_id),
            (offset + 4, [0x82, 60, 0], Some(new_id))
        );
        assert_eq!(control.status.applied_revision.load(Acquire), 2);
        assert_eq!(control.reclaim(), 1);
        worker.complete(next).unwrap();
        keys.set_key(0, None).unwrap();
        render(&mut callback);
        let third = worker.receive().unwrap().unwrap();
        let [key_release] = third.events.as_slice() else {
            panic!("physical key releases once on the retained instrument")
        };
        assert_eq!(
            (key_release.bytes, key_release.note_id),
            ([0x80, 67, 0], Some(live_id))
        );
    }
}
