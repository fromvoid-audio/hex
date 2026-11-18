//! Control-side preparation of an Arrangement VST3 instrument source.
use super::*;
use crate::Invalid;
use hex_project::{BeatTime, Editor, Project};

const VST3: DeviceTypeId = DeviceTypeId::from_static("hex.vst3");

struct InstrumentDefinition {
    identity: ProcessorInstanceIdentity,
    second: Option<ProcessorInstanceIdentity>,
    instrument_profile: bool,
    epoch: u64,
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
        if (identity != self.identity && Some(identity) != self.second)
            || context.sample_rate != 48_000
        {
            return Err(DeviceContractError::InvalidId);
        }
        let facts = PreparedForeignProcessor::new(8, 3, 16)?.with_binding(
            hex_lattice_contract::ForeignProcessorBinding {
                root: 1,
                instance: identity.device_id(),
                graph: 1,
                epoch: self.epoch,
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

fn fixture() -> (Editor, ProcessorInstanceIdentity) {
    fixture_with_duration(BeatTime::from_ratio(1, 1).unwrap())
}

fn fixture_with_duration(duration: BeatTime) -> (Editor, ProcessorInstanceIdentity) {
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
            duration,
            pitch: 60,
            channel: 2,
            velocity: 101,
        })
        .unwrap();
    (editor, identity)
}

fn prepare(
    editor: &Editor,
    identity: ProcessorInstanceIdentity,
    instrument_profile: bool,
) -> Result<Arc<PreparedRevision>, Invalid> {
    prepare_epoch(editor, identity, instrument_profile, 1)
}

fn prepare_epoch(
    editor: &Editor,
    identity: ProcessorInstanceIdentity,
    instrument_profile: bool,
    epoch: u64,
) -> Result<Arc<PreparedRevision>, Invalid> {
    prepare_instruments(editor, identity, None, instrument_profile, epoch)
}

fn prepare_instruments(
    editor: &Editor,
    identity: ProcessorInstanceIdentity,
    second: Option<ProcessorInstanceIdentity>,
    instrument_profile: bool,
    epoch: u64,
) -> Result<Arc<PreparedRevision>, Invalid> {
    prepare_instruments_with_media(
        editor,
        identity,
        second,
        instrument_profile,
        epoch,
        &std::collections::BTreeMap::new(),
    )
}

fn prepare_instruments_with_media(
    editor: &Editor,
    identity: ProcessorInstanceIdentity,
    second: Option<ProcessorInstanceIdentity>,
    instrument_profile: bool,
    epoch: u64,
    media: &std::collections::BTreeMap<hex_project::AssetId, crate::Audio>,
) -> Result<Arc<PreparedRevision>, Invalid> {
    let definition = InstrumentDefinition {
        identity,
        second,
        instrument_profile,
        epoch,
    };
    let definitions: [&dyn NativeDeviceDefinition; 2] =
        [&definition, &hex_devices::delay::DelayDefinition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    PreparedRevision::prepare_with_effect_registry(
        editor.project(),
        media,
        Arc::new(SineTable::new()),
        None,
        &registry,
    )
    .map(Arc::new)
}

#[test]
fn live_new_track_and_second_instrument_keep_the_first_gate_and_clock() {
    let (mut editor, first) = fixture();
    let mut playing = SessionPlayback::new(prepare(&editor, first, true).unwrap()).unwrap();
    let mut endpoint = NoteEndpoint::default();
    playing.apply(SessionPlaybackCommand::Play).unwrap();
    endpoint.render(&mut playing, &mut [[0.0; 2]; 32]).unwrap();
    let first_note = endpoint.notes[0].1;
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "second".into(),
        })
        .unwrap()
    else {
        panic!("second track")
    };
    let mut empty = SessionPlayback::new(prepare(&editor, first, true).unwrap()).unwrap();
    empty
        .inherit_state(&mut playing)
        .expect("append independent empty track during play");
    assert_eq!(empty.performance_frame(), 32);
    endpoint.render(&mut empty, &mut [[0.0; 2]; 32]).unwrap();
    assert!(endpoint.notes.is_empty());
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(1).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 128).unwrap(),
            pitch: 67,
            channel: 2,
            velocity: 100,
        })
        .unwrap();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: NativeDeviceState::new(VST3, 1, vec![19]).unwrap(),
        })
        .unwrap();
    let device = editor.project().track(track).unwrap().instrument().unwrap();
    let second = ProcessorInstanceIdentity::new(
        ProcessorChainOwner::Track(track.value()),
        device.id().value(),
    )
    .unwrap();
    let mut candidate =
        SessionPlayback::new(prepare_instruments(&editor, first, Some(second), true, 1).unwrap())
            .unwrap();
    {
        let target = candidate.compensation.as_ref().unwrap();
        let donor = empty.compensation.as_mut().unwrap();
        let crate::signal_graph::TrackOutput::Main { edge } = donor.graph.tracks[1].output else {
            panic!("direct main route");
        };
        assert!(target.can_copy_from(donor, Some(crate::signal_graph::FreshHistory::Track(track))));
        donor.delays.begin_transaction().unwrap();
        donor.edge(edge, [1.0, 0.0]).unwrap();
        donor.delays.commit_transaction().unwrap();
        assert!(
            !target.can_copy_from(donor, Some(crate::signal_graph::FreshHistory::Track(track)))
        );
        donor.delays.begin_transaction().unwrap();
        let mut pending = [[0.0; 2]; 19];
        donor.edge_block(edge, &mut pending).unwrap();
        donor.delays.commit_transaction().unwrap();
        assert!(
            pending.contains(&[1.0, 0.0]),
            "refusal must preserve delayed PCM"
        );
    }
    candidate
        .inherit_state(&mut empty)
        .expect("install fresh second generator during play");
    assert_eq!(candidate.performance_frame(), 64);

    #[derive(Default)]
    struct Sources(Vec<(u64, hex_lattice::ForeignNote)>);
    impl ForeignProcessors for Sources {
        fn process_segment(
            &mut self,
            _: ForeignProcessorNode<'_>,
            _: usize,
            _: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            Err(DeviceContractError::ForeignProcessingFailed)
        }
        fn process_segment_with_notes(
            &mut self,
            node: ForeignProcessorNode<'_>,
            _: usize,
            _: &mut [Frame],
            _: &[hex_lattice_contract::ForeignParameterPoint],
            notes: &[hex_lattice::ForeignNote],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            self.0
                .extend(notes.iter().map(|note| (node.device_id, *note)));
            Ok(())
        }
        fn reject_callback(&mut self) {}
    }
    let mut sources = Sources::default();
    candidate
        .render_with_foreign_processors(&mut [[0.0; 2]; 256], None, None, None, &mut sources)
        .unwrap();
    assert_eq!(candidate.performance_frame(), 320);
    assert_eq!(sources.0.len(), 2);
    assert!(
        sources
            .0
            .iter()
            .all(|(device, note)| *device == second.device_id() && note.pitch == 67)
    );
    assert!(sources.0[0].1.on && !sources.0[1].1.on);
    let mut rest = vec![[0.0; 2]; 24_001 - 320];
    sources.0.clear();
    candidate
        .render_with_foreign_processors(&mut rest, None, None, None, &mut sources)
        .unwrap();
    assert_eq!(
        sources.0,
        [(
            first.device_id(),
            hex_lattice::ForeignNote {
                on: false,
                velocity: 0,
                ..first_note
            }
        )]
    );
}

#[test]
fn silent_group_child_edit_keeps_a_hosted_instrument_gate_and_compensated_pcm() {
    for partition in [1, 257] {
        let (mut editor, first) = fixture_with_duration(BeatTime::from_ratio(1, 50).unwrap());
        let track = editor.project().tracks()[0].id();
        let Created::Track(group) = editor
            .execute(Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group");
        };
        let original = prepare(&editor, first, true).unwrap();
        assert_eq!(original.arrangement().main_latency_frames(), 19);
        let mut playing = SessionPlayback::new(Arc::clone(&original)).unwrap();
        let mut reference = SessionPlayback::new(original).unwrap();
        let mut endpoint = NoteEndpoint {
            instrument_sample: 0.25,
            ..Default::default()
        };
        let mut reference_endpoint = NoteEndpoint {
            instrument_sample: 0.25,
            ..Default::default()
        };
        playing.apply(SessionPlaybackCommand::Play).unwrap();
        reference.apply(SessionPlaybackCommand::Play).unwrap();
        let mut warm = [[0.0; 2]; 32];
        endpoint.render(&mut playing, &mut warm).unwrap();
        reference_endpoint
            .render(&mut reference, &mut [[0.0; 2]; 32])
            .unwrap();
        let [(_, onset)] = endpoint.notes.as_slice() else {
            panic!("one original note-on");
        };
        assert!(onset.on && warm[31] == [0.25; 2]);
        let onset = *onset;
        let Created::Track(empty) = editor
            .execute(Command::InsertTrack {
                name: "empty".into(),
                before_track: Some(track),
            })
            .unwrap()
        else {
            panic!("track");
        };
        assert_eq!(
            editor.project().track(empty).unwrap().parent_group(),
            Some(group)
        );
        let mut released = 0;
        for removed in [false, true] {
            if removed {
                editor.execute(Command::RemoveTrack(empty)).unwrap();
            }
            let mut next = SessionPlayback::new(prepare(&editor, first, true).unwrap()).unwrap();
            next.try_inherit_state_from(&mut playing)
                .expect("unchanged hosted gate and clock must admit a silent child edit");
            let mut actual = [[0.0; 2]; 257];
            let mut expected = actual;
            for (actual, expected) in actual
                .chunks_mut(partition)
                .zip(expected.chunks_mut(partition))
            {
                endpoint.render(&mut next, actual).unwrap();
                reference_endpoint.render(&mut reference, expected).unwrap();
                assert_eq!(actual, expected, "partition {partition}, removed {removed}");
                assert_eq!(endpoint.notes, reference_endpoint.notes);
                for &(_, note) in &endpoint.notes {
                    assert!(removed && !note.on && note.note_id == onset.note_id);
                    released += 1;
                }
            }
            assert_eq!(next.performance_frame(), reference.performance_frame());
            playing = next;
        }
        assert_eq!(released, 1);
    }
}

#[test]
fn vst3_instrument_prepares_exact_generator_and_authored_notes() {
    let (editor, identity) = fixture();
    assert!(matches!(
        prepare(&editor, identity, false),
        Err(Invalid(
            "vst3 instrument worker binding or processing role differs"
        ))
    ));
    let revision = prepare(&editor, identity, true).unwrap();
    let track = &revision.arrangement().tracks()[0];
    let node = track.foreign_instrument.as_ref().unwrap();
    assert_eq!(node.owner, identity.owner());
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
    assert_eq!(revision.arrangement().main_latency_frames(), 19);
    let playback = crate::playback::Playback::from_prepared(revision);
    let nodes: Vec<_> = playback.foreign_nodes_on_control().collect();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].device_id, identity.device_id());
    let mut bindings = Vec::new();
    playback.visit_foreign_runtime_bindings(&mut |binding| bindings.push(binding.unwrap()));
    assert_eq!(bindings[0].instance, identity.device_id());
}

#[derive(Default)]
struct NoteEndpoint {
    notes: Vec<(usize, hex_lattice::ForeignNote)>,
    next_offset: usize,
    fail_at: Option<usize>,
    rejected: bool,
    instrument_sample: f32,
    effect: Option<Borrowed>,
}

impl ForeignProcessors for NoteEndpoint {
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [Frame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        self.effect
            .as_mut()
            .expect("only the hosted effect uses this endpoint")
            .process_segment(node, offset, frames, clock)
    }

    fn process_segment_with_notes(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [Frame],
        _: &[hex_lattice_contract::ForeignParameterPoint],
        notes: &[hex_lattice::ForeignNote],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        assert_eq!(offset, self.next_offset);
        assert_eq!(frames, [[0.0; 2]]);
        assert_eq!(node.prepared.binding().unwrap().instance, node.device_id);
        assert!(clock.unwrap().transport_is_running().is_some());
        self.next_offset += frames.len();
        self.notes.extend(notes.iter().map(|note| (offset, *note)));
        if self.fail_at == Some(offset) {
            return Err(DeviceContractError::ForeignProcessingFailed);
        }
        frames.fill([self.instrument_sample; 2]);
        Ok(())
    }

    fn reject_callback(&mut self) {
        self.rejected = true;
    }
}

impl NoteEndpoint {
    fn render(
        &mut self,
        playback: &mut SessionPlayback,
        frames: &mut [Frame],
    ) -> Result<(), SessionPlaybackError> {
        self.next_offset = 0;
        self.notes.clear();
        if let Some(effect) = &mut self.effect {
            effect.next_offset = 0;
        }
        playback.render_with_foreign_processors(frames, None, None, None, self)
    }
}

#[test]
fn paused_vst3_instrument_preserves_downstream_arrangement_pcm() {
    use crate::computer_keyboard::ComputerKeyboardInput;
    use crate::playback::{Playback, PlaybackCommand};

    let (mut editor, identity) = fixture();
    let track = editor.project().tracks()[0].id();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "audio".into(),
            path: "pause-marker.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 1024,
            playback_frames: 1024,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("audio marker")
    };
    let mut samples = vec![[0.0; 2]; 1024];
    samples[253] = [0.25, -0.5];
    let media =
        std::collections::BTreeMap::from([(asset, crate::Audio::new(48_000, samples).unwrap())]);
    let revision =
        prepare_instruments_with_media(&editor, identity, None, true, 1, &media).unwrap();
    assert_eq!(revision.arrangement().main_latency_frames(), 19);

    for partition in [1, 257] {
        let mut paused = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        let mut reference =
            Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        let keys = Arc::new(ComputerKeyboardInput::default());
        paused
            .prepare_computer_keyboard(editor.project(), track, Arc::clone(&keys))
            .unwrap();
        paused.prepare_resident_paused_source_monitor(None).unwrap();
        let mut endpoint = NoteEndpoint {
            instrument_sample: 0.125,
            ..Default::default()
        };
        let mut reference_endpoint = NoteEndpoint {
            instrument_sample: 0.125,
            ..Default::default()
        };
        let render = |player: &mut Playback, endpoint: &mut NoteEndpoint, pcm: &mut [Frame]| {
            endpoint.next_offset = 0;
            endpoint.notes.clear();
            player
                .render_with_foreign_processors(pcm, None, None, None, endpoint)
                .unwrap();
        };
        for (player, endpoint) in [
            (&mut paused, &mut endpoint),
            (&mut reference, &mut reference_endpoint),
        ] {
            player.apply(PlaybackCommand::Play);
            render(player, endpoint, &mut [[0.0; 2]; 257]);
            player.apply(PlaybackCommand::Pause);
        }
        let [(0, original)] = endpoint.notes[..] else {
            panic!("authored onset")
        };
        assert!(original.on && original.pitch == 60);
        keys.set_key(0, Some((67, 100, 1))).unwrap();
        let mut pcm = [[0.0; 2]; 257];
        let mut events = Vec::new();
        for (index, block) in pcm.chunks_mut(partition).enumerate() {
            render(&mut paused, &mut endpoint, block);
            events.extend(
                endpoint
                    .notes
                    .iter()
                    .map(|(offset, note)| (index * partition + offset, *note)),
            );
        }
        let [(0, off), (7, on)] = events[..] else {
            panic!("one authored release and the next native-quantum live onset: {events:?}");
        };
        assert!(!off.on && off.note_id == original.note_id);
        assert!(on.on && on.pitch == 67 && on.note_id != original.note_id);
        assert_eq!(paused.position(), 257);
        assert!(pcm.iter().any(|frame| *frame != [0.0; 2]));
        let mut resumed = [[0.0; 2]; 19];
        let mut expected = resumed;
        paused.apply(PlaybackCommand::Play);
        reference.apply(PlaybackCommand::Play);
        render(&mut paused, &mut endpoint, &mut resumed);
        render(&mut reference, &mut reference_endpoint, &mut expected);
        assert!(
            endpoint.notes.is_empty(),
            "the retained source must not chase the old onset"
        );
        assert_eq!(expected[15][0] - expected[15][1], 0.75);
        assert_eq!(resumed, expected, "partition {partition}");
        assert!(pcm.iter().all(|frame| frame[0] == frame[1]));
    }
}

#[test]
fn finite_bounce_requires_an_explicit_foreign_instrument_renderer() {
    let (editor, identity) = fixture();
    let revision = prepare(&editor, identity, true).unwrap();
    assert!(
        SessionPlayback::new_finite_arrangement_bounce(Arc::clone(&revision), None, 8).is_err()
    );
    assert!(revision.arrangement_bounce_requires_preroll());
    let mut playback =
        SessionPlayback::new_finite_arrangement_bounce_with_foreign(revision, None, 8).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut endpoint = NoteEndpoint::default();
    endpoint.render(&mut playback, &mut [[0.0; 2]; 8]).unwrap();
    let [(offset, note)] = endpoint.notes.as_slice() else {
        panic!("expected one authored MIDI note");
    };
    assert_eq!((*offset, note.frame, note.pitch, note.on), (0, 0, 60, true));
}

#[test]
fn linear_foreign_export_requires_a_renderer_and_delivers_authored_notes() {
    use crate::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
    use std::sync::atomic::AtomicBool;

    let (mut editor, identity) = fixture();
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 1024,
            enabled: true,
        })
        .unwrap();
    let revision = prepare(&editor, identity, true).unwrap();
    assert!(SessionPlayback::new_linear_arrangement_export(Arc::clone(&revision)).is_err());
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        editor.project(),
        1_000_000,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(
        SessionPlayback::new_linear_arrangement_export_with_foreign(
            Arc::clone(&revision),
            Some(&resources),
        )
        .is_err()
    );
    let mut without_renderer =
        SessionPlayback::new_linear_arrangement_export_with_foreign(Arc::clone(&revision), None)
            .unwrap();
    without_renderer
        .apply(SessionPlaybackCommand::Play)
        .unwrap();
    assert!(without_renderer.render(&mut [[0.0; 2]; 8]).is_err());

    let mut playback =
        SessionPlayback::new_linear_arrangement_export_with_foreign(revision, None).unwrap();
    assert!(playback.is_linear_arrangement_export());
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut endpoint = NoteEndpoint::default();
    endpoint.render(&mut playback, &mut [[0.0; 2]; 8]).unwrap();
    let [(offset, note)] = endpoint.notes.as_slice() else {
        panic!("expected one authored MIDI note");
    };
    assert_eq!((*offset, note.frame, note.pitch, note.on), (0, 0, 60, true));
    playback.apply(SessionPlaybackCommand::Seek(1023)).unwrap();
    endpoint.render(&mut playback, &mut [[0.0; 2]; 8]).unwrap();
    assert_eq!(playback.arrangement_frame(), 1031);
    let long_frame = MAX_FINITE_BOUNCE_FRAMES + 1;
    playback
        .apply(SessionPlaybackCommand::Seek(long_frame))
        .unwrap();
    endpoint.render(&mut playback, &mut [[0.0; 2]; 8]).unwrap();
    assert_eq!(playback.arrangement_frame(), long_frame + 8);
}

#[test]
fn vst_note_audition_sends_a_finite_exact_velocity_chord_once() {
    use crate::{
        computer_keyboard::{ComputerKeyboardInput, KeyboardActivationClock},
        playback::Playback,
    };
    let (editor, identity) = fixture();
    let project = editor.project();
    let track = project.tracks()[0].id();
    let revision = prepare(&editor, identity, true).unwrap();
    let clock = Arc::new(KeyboardActivationClock::default());
    let input = Arc::new(
        ComputerKeyboardInput::with_output_activation(clock)
            .unwrap()
            .with_note_audition(&[(60, 37), (64, 93)], 8)
            .unwrap(),
    );
    let mut playback = Playback::from_prepared(Arc::clone(&revision));
    playback
        .prepare_computer_keyboard(project, track, Arc::clone(&input))
        .unwrap();
    let mut endpoint = NoteEndpoint::default();
    let mut output = [[0.0; 2]; 8];
    endpoint.next_offset = 0;
    playback
        .render_with_foreign_processors(&mut output, None, None, None, &mut endpoint)
        .unwrap();
    assert!(endpoint.notes.is_empty());
    input.trigger_note_audition().unwrap();
    endpoint.next_offset = 0;
    playback
        .render_with_foreign_processors(&mut output, None, None, None, &mut endpoint)
        .unwrap();
    let onsets = endpoint.notes.clone();
    assert_eq!(onsets.len(), 2);
    assert_eq!(
        (onsets[0].1.pitch, onsets[0].1.velocity, onsets[0].1.on),
        (60, 37, true)
    );
    assert_eq!(
        (onsets[1].1.pitch, onsets[1].1.velocity, onsets[1].1.on),
        (64, 93, true)
    );
    assert!(input.note_audition_finished());
    endpoint.next_offset = 0;
    endpoint.notes.clear();
    playback
        .render_with_foreign_processors(&mut output, None, None, None, &mut endpoint)
        .unwrap();
    assert_eq!(endpoint.notes.len(), 2);
    for (release, onset) in endpoint.notes.iter().zip(&onsets) {
        assert!(!release.1.on);
        assert_eq!(release.1.note_id, onset.1.note_id);
    }
    let mut replacement = Playback::from_prepared(revision);
    replacement
        .prepare_computer_keyboard(project, track, Arc::clone(&input))
        .unwrap();
    endpoint.next_offset = 0;
    endpoint.notes.clear();
    replacement
        .render_with_foreign_processors(&mut output, None, None, None, &mut endpoint)
        .unwrap();
    assert!(endpoint.notes.is_empty(), "a completed bank cannot replay");
}

#[test]
fn vst_audition_rolls_back_failed_audio_and_keeps_gate_across_handoff() {
    use crate::{
        computer_keyboard::{ComputerKeyboardInput, KeyboardActivationClock},
        playback::Playback,
    };
    let (editor, identity) = fixture();
    let project = editor.project();
    let track = project.tracks()[0].id();
    let revision = prepare(&editor, identity, true).unwrap();
    let clock = Arc::new(KeyboardActivationClock::default());
    let input = Arc::new(
        ComputerKeyboardInput::with_output_activation(clock)
            .unwrap()
            .with_note_audition(&[(60, 37)], 8)
            .unwrap(),
    );
    let mut playback = Playback::from_prepared(Arc::clone(&revision));
    playback
        .prepare_computer_keyboard(project, track, Arc::clone(&input))
        .unwrap();
    input.trigger_note_audition().unwrap();
    let mut endpoint = NoteEndpoint {
        fail_at: Some(1),
        ..Default::default()
    };
    let mut output = [[1.0; 2]; 4];
    assert!(
        playback
            .render_with_foreign_processors(&mut output, None, None, None, &mut endpoint)
            .is_err()
    );
    assert!(endpoint.rejected);
    assert_eq!(output, [[0.0; 2]; 4]);
    assert!(!input.note_audition_finished());
    endpoint.fail_at = None;
    endpoint.next_offset = 0;
    endpoint.notes.clear();
    playback
        .render_with_foreign_processors(&mut output, None, None, None, &mut endpoint)
        .unwrap();
    let [(0, onset)] = endpoint.notes.as_slice() else {
        panic!("retry must admit the onset once")
    };
    let onset = *onset;
    assert!(onset.on && onset.pitch == 60 && onset.velocity == 37);
    assert!(!input.note_audition_finished());
    let mut replacement = Playback::from_prepared(revision);
    replacement
        .prepare_computer_keyboard_for_controller_activation(project, track, Arc::clone(&input))
        .unwrap();
    replacement
        .try_inherit_runtime_in_control_context(&mut playback, Some(1), Some(1))
        .unwrap();
    endpoint.next_offset = 0;
    endpoint.notes.clear();
    endpoint.fail_at = Some(3);
    assert!(
        replacement
            .render_with_foreign_processors(&mut output, None, None, None, &mut endpoint)
            .is_err()
    );
    assert!(
        !input.note_audition_finished(),
        "a rejected render must not consume gate time"
    );
    endpoint.fail_at = None;
    endpoint.next_offset = 0;
    endpoint.notes.clear();
    replacement
        .render_with_foreign_processors(&mut output, None, None, None, &mut endpoint)
        .unwrap();
    assert!(
        endpoint.notes.is_empty(),
        "handoff must retain the held note"
    );
    assert!(input.note_audition_finished());
    endpoint.next_offset = 0;
    endpoint.notes.clear();
    replacement
        .render_with_foreign_processors(&mut output, None, None, None, &mut endpoint)
        .unwrap();
    let [(0, release)] = endpoint.notes.as_slice() else {
        panic!("bounded gate must release")
    };
    assert!(!release.on && release.note_id == onset.note_id);
}

#[test]
fn vst_audition_cancel_releases_the_original_note() {
    use crate::{computer_keyboard::ComputerKeyboardInput, playback::Playback};
    let (editor, identity) = fixture();
    let project = editor.project();
    let track = project.tracks()[0].id();
    let input = Arc::new(
        ComputerKeyboardInput::default()
            .with_note_audition(&[(61, 41)], 100)
            .unwrap(),
    );
    let mut playback = Playback::from_prepared(prepare(&editor, identity, true).unwrap());
    playback
        .prepare_computer_keyboard(project, track, Arc::clone(&input))
        .unwrap();
    input.trigger_note_audition().unwrap();
    let mut endpoint = NoteEndpoint::default();
    let mut output = [[0.0; 2]; 8];
    playback
        .render_with_foreign_processors(&mut output, None, None, None, &mut endpoint)
        .unwrap();
    let [(0, onset)] = endpoint.notes.as_slice() else {
        panic!("audition onset")
    };
    let onset = *onset;
    input.cancel();
    endpoint.next_offset = 0;
    endpoint.notes.clear();
    playback
        .render_with_foreign_processors(&mut output, None, None, None, &mut endpoint)
        .unwrap();
    let [(0, release)] = endpoint.notes.as_slice() else {
        panic!("cancellation release")
    };
    assert!(!release.on && release.note_id == onset.note_id);
}

#[test]
fn computer_keyboard_uses_the_same_worker_while_stopped_playing_and_rearmed_after_stop() {
    use crate::{
        computer_keyboard::{ComputerKeyboardInput, KeyboardActivationClock},
        playback::{Playback, PlaybackCommand},
    };
    let (editor, identity) = fixture();
    let project = editor.project();
    let track = project.tracks()[0].id();
    let revision = prepare(&editor, identity, true).unwrap();
    let clock = Arc::new(KeyboardActivationClock::default());
    let keys = Arc::new(ComputerKeyboardInput::with_output_activation(Arc::clone(&clock)).unwrap());
    let mut playback = Playback::from_prepared(Arc::clone(&revision));
    playback
        .prepare_computer_keyboard(project, track, Arc::clone(&keys))
        .unwrap();
    let mut endpoint = NoteEndpoint::default();
    let mut render = |playback: &mut Playback, count: usize| {
        endpoint.next_offset = 0;
        endpoint.notes.clear();
        playback
            .render_with_foreign_processors(
                &mut [[0.; 2]; 8][..count],
                None,
                None,
                None,
                &mut endpoint,
            )
            .unwrap();
        endpoint.notes.clone()
    };

    keys.set_key(0, Some((67, 100, 1))).unwrap();
    let [(0, first)] = render(&mut playback, 2)[..] else {
        panic!("stopped note-on")
    };
    assert!(first.on && first.pitch == 67 && first.channel == 0);
    playback.apply(PlaybackCommand::Play);
    let [(0, authored)] = render(&mut playback, 6)[..] else {
        panic!("authored note-on")
    };
    assert!(authored.on && authored.pitch == 60 && authored.note_id != first.note_id);

    let mut next = Playback::from_prepared(revision);
    next.prepare_computer_keyboard_for_controller_activation(project, track, Arc::clone(&keys))
        .unwrap();
    next.try_inherit_runtime_in_control_context(&mut playback, Some(1), Some(1))
        .unwrap();
    assert!(render(&mut next, 8).is_empty());
    keys.set_key(0, None).unwrap();
    let [(0, release)] = render(&mut next, 8)[..] else {
        panic!("key release")
    };
    assert!(!release.on && release.note_id == first.note_id);

    keys.set_key(0, Some((68, 100, 2))).unwrap();
    let [(0, held)] = render(&mut next, 8)[..] else {
        panic!("held key")
    };
    clock.revoke_reserved_prefix();
    keys.ensure_output_activation_on_control().unwrap();
    keys.set_key(1, Some((69, 100, 3))).unwrap();
    next.apply_priority_stop(&clock);
    let [(0, authored_off), (0, held_off), (0, fresh)] = render(&mut next, 8)[..] else {
        panic!("stop and fresh epoch")
    };
    assert!(!authored_off.on && authored_off.note_id == authored.note_id);
    assert!(!held_off.on && held_off.note_id == held.note_id);
    assert!(fresh.on && fresh.pitch == 69 && fresh.note_id != held.note_id);
    keys.cancel();
    let [(0, release)] = render(&mut next, 8)[..] else {
        panic!("cancel release")
    };
    assert!(!release.on && release.note_id == fresh.note_id);
}

#[test]
fn live_audition_releases_only_its_own_vst_occurrence() {
    use crate::{
        computer_keyboard::{ComputerKeyboardInput, LiveNoteEvent},
        playback::Playback,
    };
    let (editor, identity) = fixture();
    let project = editor.project();
    let track = project.tracks()[0].id();
    let revision = prepare(&editor, identity, true).unwrap();
    let input = Arc::new(ComputerKeyboardInput::default().with_live_notes().unwrap());
    let mut playback = Playback::from_prepared(revision);
    playback
        .prepare_computer_keyboard(project, track, Arc::clone(&input))
        .unwrap();
    let queue = input.live_notes().unwrap();
    let activation = input.begin_live_notes_activation_on_control().unwrap();
    queue
        .publish(
            activation,
            LiveNoteEvent::NoteOn {
                channel: 0,
                pitch: 60,
                velocity: 93,
            },
        )
        .unwrap();
    input
        .trigger_live_note_audition_on_control(&[(60, 37)], 4)
        .unwrap();
    let mut endpoint = NoteEndpoint::default();
    let mut render = || {
        endpoint.next_offset = 0;
        endpoint.notes.clear();
        playback
            .render_with_foreign_processors(&mut [[0.; 2]; 8], None, None, None, &mut endpoint)
            .unwrap();
        endpoint.notes.clone()
    };

    let [(_, preview), (_, hardware)] = render()[..] else {
        panic!("preview and live onsets");
    };
    assert!(preview.on && preview.velocity == 37);
    assert!(hardware.on && hardware.velocity == 93 && hardware.note_id != preview.note_id);
    let [(_, released)] = render()[..] else {
        panic!("only the preview expires");
    };
    assert!(!released.on && released.note_id == preview.note_id);

    input
        .trigger_live_note_audition_on_control(&[(60, 37)], 32)
        .unwrap();
    let [(_, replacement)] = render()[..] else {
        panic!("fresh preview");
    };
    assert!(replacement.on && replacement.note_id != preview.note_id);
    input.cancel_live_note_audition_on_control();
    queue
        .publish(
            activation,
            LiveNoteEvent::NoteOff {
                channel: 0,
                pitch: 60,
                velocity: 41,
            },
        )
        .unwrap();
    let [(_, cancelled), (_, off)] = render()[..] else {
        panic!("each input releases its own occurrence");
    };
    assert!(!cancelled.on && cancelled.note_id == replacement.note_id);
    assert!(!off.on && off.note_id == hardware.note_id && off.velocity == 41);
    assert!(queue.same_current_activation(activation));
}

#[test]
fn hardware_midi_keeps_ordered_taps_exact_release_and_activation_scope() {
    use crate::{
        computer_keyboard::{ComputerKeyboardInput, KeyboardActivationClock, LiveNoteEvent},
        playback::{Playback, PlaybackCommand},
    };
    let (editor, identity) = fixture();
    let project = editor.project();
    let track = project.tracks()[0].id();
    let revision = prepare(&editor, identity, true).unwrap();
    let clock = Arc::new(KeyboardActivationClock::default());
    let keys = Arc::new(
        ComputerKeyboardInput::with_output_activation(Arc::clone(&clock))
            .unwrap()
            .with_live_notes()
            .unwrap(),
    );
    let mut playback = Playback::from_prepared(revision);
    playback
        .prepare_computer_keyboard(project, track, Arc::clone(&keys))
        .unwrap();
    let queue = keys.live_notes().unwrap();
    let mut activation = keys.begin_live_notes_activation_on_control().unwrap();
    let on = LiveNoteEvent::NoteOn {
        channel: 2,
        pitch: 60,
        velocity: 93,
    };
    let off = LiveNoteEvent::NoteOff {
        channel: 2,
        pitch: 60,
        velocity: 41,
    };
    let mut endpoint = NoteEndpoint::default();
    let mut render = |playback: &mut Playback, fail_at| {
        endpoint.next_offset = 0;
        endpoint.notes.clear();
        endpoint.fail_at = fail_at;
        let result = playback.render_with_foreign_processors(
            &mut [[0.; 2]; 8],
            None,
            None,
            None,
            &mut endpoint,
        );
        (result, endpoint.notes.clone())
    };

    keys.set_key(0, Some((60, 100, 1))).unwrap();
    queue.publish(activation, on).unwrap();
    queue.publish(activation, off).unwrap();
    let (result, notes) = render(&mut playback, None);
    result.unwrap();
    let [(0, key), (0, onset), (1, release)] = notes.as_slice() else {
        panic!("expected the computer key and an ordered hardware tap");
    };
    assert!(key.on && key.channel == 0 && key.note_id != onset.note_id);
    assert!(onset.on && onset.channel == 2 && onset.pitch == 60 && onset.velocity == 93);
    assert!(!release.on && release.note_id == onset.note_id && release.velocity == 41);
    assert!(queue.snapshot().unwrap().is_empty());

    queue.publish(activation, on).unwrap();
    let (result, notes) = render(&mut playback, None);
    result.unwrap();
    let [(0, held)] = notes.as_slice() else {
        panic!("expected held hardware note")
    };
    queue.cancel_activation(activation).unwrap();
    activation = keys.begin_live_notes_activation_on_control().unwrap();
    queue.publish(activation, on).unwrap();
    let (result, notes) = render(&mut playback, None);
    result.unwrap();
    let [(0, old), (0, fresh)] = notes.as_slice() else {
        panic!("expected scoped reconnect")
    };
    assert!(!old.on && old.note_id == held.note_id);
    assert!(fresh.on && fresh.note_id != held.note_id);

    playback.apply(PlaybackCommand::Play);
    let (result, notes) = render(&mut playback, None);
    result.unwrap();
    let [(0, authored)] = notes.as_slice() else {
        panic!("expected one authored note alongside the held inputs");
    };
    assert!(authored.on && authored.note_id != fresh.note_id && authored.note_id != key.note_id);
    clock.revoke_reserved_prefix();
    playback.apply_priority_stop(&clock);
    let (result, notes) = render(&mut playback, None);
    result.unwrap();
    assert_eq!(notes.len(), 3);
    assert!(
        notes
            .iter()
            .any(|(_, note)| !note.on && note.note_id == key.note_id)
    );
    assert!(
        notes
            .iter()
            .any(|(_, note)| !note.on && note.note_id == fresh.note_id)
    );
    assert!(
        notes
            .iter()
            .any(|(_, note)| !note.on && note.note_id == authored.note_id)
    );
    keys.ensure_output_activation_on_control().unwrap();
    queue.publish(activation, on).unwrap(); // The old input epoch stays fenced.
    let (result, notes) = render(&mut playback, None);
    result.unwrap();
    assert!(notes.is_empty());

    let fresh_activation = keys.begin_live_notes_activation_on_control().unwrap();
    queue.publish(fresh_activation, on).unwrap();
    assert!(render(&mut playback, Some(1)).0.is_err());
    assert_eq!(queue.snapshot().unwrap().events(), &[on]);
}

fn session_held_note_fixture() -> (Arc<PreparedRevision>, hex_project::ClipId) {
    let (mut editor, identity) = fixture();
    let track = editor.project().tracks()[0].id();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "session".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "notes".into(),
            source_length: BeatTime::from_ratio(1, 32).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 64).unwrap(),
            pitch: 64,
            channel: 3,
            velocity: 98,
        })
        .unwrap();
    let revision = prepare(&editor, identity, true).unwrap();
    (revision, clip)
}

#[test]
fn session_play_from_releases_and_restarts_foreign_note_across_partitions() {
    let (revision, clip) = session_held_note_fixture();

    for block in [1, 7, 64, 128, 256, 512] {
        let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut endpoint = NoteEndpoint::default();
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        endpoint.render(&mut playback, &mut [[0.0; 2]; 16]).unwrap();
        let [(0, original)] = endpoint.notes.as_slice() else {
            panic!("expected the original Session onset: {:?}", endpoint.notes)
        };
        let original = *original;
        assert!(original.on && original.pitch == 64 && original.channel == 3);

        playback.play_from(12_000).unwrap();
        let mut events = Vec::new();
        let mut output = [[0.0; 2]; 400];
        let mut elapsed = 0;
        for chunk in output.chunks_mut(block) {
            endpoint.render(&mut playback, chunk).unwrap();
            events.extend(
                endpoint
                    .notes
                    .iter()
                    .map(|(offset, note)| (elapsed + offset, *note)),
            );
            elapsed += chunk.len();
        }
        let [(0, released), (0, restarted), (375, ended)] = events.as_slice() else {
            panic!("block {block}: unexpected events {events:?}")
        };
        assert!(
            !released.on && released.note_id == original.note_id,
            "block {block}"
        );
        assert!(
            restarted.on && restarted.note_id != original.note_id,
            "block {block}"
        );
        assert_eq!(
            (restarted.pitch, restarted.channel, restarted.velocity),
            (64, 3, 98)
        );
        assert!(
            !ended.on && ended.note_id == restarted.note_id,
            "block {block}"
        );
    }
}

#[test]
fn timed_play_from_releases_and_restarts_a_held_session_note() {
    use crate::TimedTransportCommand;
    use crate::playback::{Playback, PlaybackAction, PlaybackCommand};

    let (revision, clip) = session_held_note_fixture();

    for block in [1, 7, 64, 128, 256, 512] {
        let mut playback = Playback::from_prepared(Arc::clone(&revision));
        playback
            .try_apply_action(PlaybackAction::LaunchAndPlay(
                SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                },
            ))
            .unwrap();
        let mut endpoint = NoteEndpoint::default();
        let mut output = [[0.0; 2]; 512];
        let mut events = Vec::new();
        for (index, part) in output.chunks_mut(block).enumerate() {
            let start = index * block;
            let end = start + part.len();
            let commands = if start <= 16 && 16 < end {
                vec![TimedTransportCommand {
                    offset: 16 - start,
                    command: PlaybackCommand::PlayFrom(12_000),
                }]
            } else {
                Vec::new()
            };
            endpoint.next_offset = 0;
            endpoint.notes.clear();
            playback
                .render_with_timed_transport_and_foreign_processors(part, &commands, &mut endpoint)
                .unwrap();
            assert!(!endpoint.rejected, "block {block}");
            events.extend(
                endpoint
                    .notes
                    .iter()
                    .map(|(offset, note)| (start + offset, *note)),
            );
        }
        let [(0, original), (16, released), (16, restarted), (391, ended)] = events.as_slice()
        else {
            panic!("block {block}: unexpected events {events:?}")
        };
        assert!(original.on && original.pitch == 64 && original.channel == 3);
        assert!(!released.on && released.note_id == original.note_id);
        assert!(restarted.on && restarted.note_id != original.note_id);
        assert_eq!(
            (restarted.pitch, restarted.channel, restarted.velocity),
            (64, 3, 98)
        );
        assert!(!ended.on && ended.note_id == restarted.note_id);
    }
}

#[test]
fn console_clip_replaces_arrangement_gates_and_stop_releases_its_note() {
    use crate::LaunchQuantization::Immediate;
    let (mut editor, identity) = fixture();
    let track = editor.project().tracks()[0].id();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "console".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "console notes".into(),
            source_length: BeatTime::from_quarters(1).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).unwrap(),
            pitch: 64,
            channel: 3,
            velocity: 98,
        })
        .unwrap();
    let mut playback = SessionPlayback::new(prepare(&editor, identity, true).unwrap()).unwrap();
    let mut endpoint = NoteEndpoint::default();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    endpoint.render(&mut playback, &mut [[0.0; 2]; 2]).unwrap();
    let original = endpoint.notes[0].1;
    assert!(original.on && original.pitch == 60);
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: Immediate,
        })
        .unwrap();
    endpoint.render(&mut playback, &mut [[0.0; 2]; 2]).unwrap();
    let [(0, release), (0, console)] = endpoint.notes.as_slice() else {
        panic!(
            "expected arrangement release and console onset, got {:?}",
            endpoint.notes
        );
    };
    assert!(!release.on && release.note_id == original.note_id);
    assert!(
        console.on
            && console.pitch == 64
            && console.channel == 3
            && console.note_id != original.note_id
    );
    let console_id = console.note_id;
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track,
            quantization: Immediate,
        })
        .unwrap();
    endpoint.render(&mut playback, &mut [[0.0; 2]; 2]).unwrap();
    let [(0, release)] = endpoint.notes.as_slice() else {
        panic!("expected console release")
    };
    assert!(!release.on && release.note_id == console_id);
    playback
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: Immediate,
        })
        .unwrap();
    endpoint.render(&mut playback, &mut [[0.0; 2]; 2]).unwrap();
    assert!(
        endpoint.notes.is_empty(),
        "old arrangement note must not restart"
    );

    editor
        .execute(Command::SetMidiClipGain {
            clip,
            gain: hex_project::ClipGain::new(-6_000).unwrap(),
        })
        .unwrap();
    let mut playback = SessionPlayback::new(prepare(&editor, identity, true).unwrap()).unwrap();
    assert!(matches!(
        playback.apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: Immediate
        }),
        Err(SessionPlaybackError::MutableInstrumentDiagnostic(
            "console clip volume is unavailable for vst3 instruments"
        ))
    ));
}

#[test]
fn seek_and_stop_release_original_notes_and_a_late_fault_refuses_the_block() {
    let (editor, identity) = fixture();
    let revision = prepare(&editor, identity, true).unwrap();
    let mut playback = SessionPlayback::new(revision).unwrap();
    let mut endpoint = NoteEndpoint::default();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    endpoint.render(&mut playback, &mut [[0.0; 2]; 2]).unwrap();
    let [(0, initial)] = endpoint.notes.as_slice() else {
        panic!("one onset");
    };
    let initial = *initial;
    assert!(initial.on && initial.channel == 2 && initial.pitch == 60);
    playback.apply(SessionPlaybackCommand::Seek(9)).unwrap();
    endpoint.render(&mut playback, &mut [[0.0; 2]; 2]).unwrap();
    let [(0, release)] = endpoint.notes.as_slice() else {
        panic!("seek releases without chasing");
    };
    assert!(!release.on);
    assert_eq!(
        (release.note_id, release.channel, release.pitch),
        (initial.note_id, 2, 60)
    );
    playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    endpoint.render(&mut playback, &mut [[0.0; 2]; 1]).unwrap();
    let restarted = endpoint.notes[0].1;
    assert!(restarted.on && restarted.note_id != initial.note_id);
    playback.apply(SessionPlaybackCommand::Stop).unwrap();
    endpoint.render(&mut playback, &mut [[0.0; 2]; 2]).unwrap();
    assert_eq!(
        endpoint.notes,
        [(
            0,
            hex_lattice::ForeignNote {
                on: false,
                velocity: 0,
                ..restarted
            }
        )]
    );
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    endpoint.fail_at = Some(1);
    let mut output = [[1.0; 2]; 2];
    assert!(endpoint.render(&mut playback, &mut output).is_err());
    assert!(endpoint.rejected);
    assert_eq!(output, [[0.0; 2]; 2]);
    assert_eq!(playback.performance_frame(), 0);
}

#[test]
fn compatible_gain_edit_keeps_the_accepted_instrument_note_until_release() {
    let (mut editor, identity) = fixture_with_duration(BeatTime::from_ratio(1, 128).unwrap());
    let original = prepare(&editor, identity, true).unwrap();
    let prepared_track = &original.arrangement().tracks()[0];
    let track = prepared_track.canonical_track_id().unwrap();
    let mut deadline = None;
    prepared_track
        .for_each_mutable_midi_span(0, 1, |span| deadline = Some(span.gate_end))
        .unwrap();
    let mut playing = SessionPlayback::new(original).unwrap();
    let mut endpoint = NoteEndpoint::default();
    playing.apply(SessionPlaybackCommand::Play).unwrap();
    endpoint.render(&mut playing, &mut [[0.0; 2]; 2]).unwrap();
    let [(0, onset)] = endpoint.notes.as_slice() else {
        panic!("one onset");
    };
    let onset = *onset;

    editor.execute(Command::Gain { track, gain: 0.5 }).unwrap();
    let mut edited = SessionPlayback::new(prepare(&editor, identity, true).unwrap()).unwrap();
    edited.inherit_state(&mut playing).unwrap();
    assert_eq!(edited.performance_frame(), 2);
    endpoint.render(&mut edited, &mut [[0.0; 2]; 2]).unwrap();
    assert!(endpoint.notes.is_empty());

    let frames_until_release = usize::try_from(deadline.unwrap() - 4).unwrap();
    endpoint
        .render(&mut edited, &mut vec![[0.0; 2]; frames_until_release + 1])
        .unwrap();
    assert_eq!(
        endpoint.notes,
        [(
            frames_until_release,
            hex_lattice::ForeignNote {
                on: false,
                velocity: 0,
                ..onset
            }
        )]
    );
}

#[test]
fn adding_delay_during_a_loop_keeps_the_foreign_instrument_gate_and_clock() {
    let (mut editor, identity) = fixture_with_duration(BeatTime::from_ratio(1, 128).unwrap());
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::AddTrack {
            name: "independent track".into(),
        })
        .unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 1024,
            enabled: true,
        })
        .unwrap();
    let original = prepare(&editor, identity, true).unwrap();
    let mut deadline = None;
    original.arrangement().tracks()[0]
        .for_each_mutable_midi_span(0, 1, |span| deadline = Some(span.gate_end))
        .unwrap();
    let mut playing = SessionPlayback::new(original).unwrap();
    let mut endpoint = NoteEndpoint::default();
    playing.apply(SessionPlaybackCommand::Play).unwrap();
    endpoint
        .render(&mut playing, &mut [[0.0; 2]; 1024])
        .unwrap();
    endpoint.render(&mut playing, &mut [[0.0; 2]; 32]).unwrap();
    let [(0, onset)] = endpoint.notes.as_slice() else {
        panic!("one onset after the wrap");
    };
    assert!(onset.on);
    let onset = *onset;

    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_devices::delay::DelayDefinition.default_state().unwrap(),
        })
        .unwrap();
    let mut edited = SessionPlayback::new(prepare(&editor, identity, true).unwrap()).unwrap();
    edited.try_inherit_state_live_from(&mut playing).unwrap();
    assert_eq!(edited.performance_frame(), 1056);
    assert_eq!(edited.arrangement_frame(), 32);
    assert!(edited.playing());
    endpoint.render(&mut edited, &mut [[0.0; 2]; 8]).unwrap();
    assert!(endpoint.notes.is_empty());

    editor.execute(Command::Gain { track, gain: 0.5 }).unwrap();
    let mut adjusted = SessionPlayback::new(prepare(&editor, identity, true).unwrap()).unwrap();
    adjusted.try_inherit_state_live_from(&mut edited).unwrap();
    let release = usize::try_from(deadline.unwrap() - 40).unwrap();
    endpoint
        .render(&mut adjusted, &mut vec![[0.0; 2]; release + 1])
        .unwrap();
    assert_eq!(
        endpoint.notes,
        [(
            release,
            hex_lattice::ForeignNote {
                on: false,
                velocity: 0,
                ..onset
            }
        )]
    );
}

#[test]
fn moving_a_wrapped_loop_keeps_foreign_gates_through_live_effect_edits() {
    for on_main in [false, true] {
        let short_note = BeatTime::from_ratio(1, 128).unwrap();
        let (mut editor, instrument) = fixture_with_duration(short_note);
        let track = editor.project().tracks()[0].id();
        let clip = editor.project().tracks()[0].midi_clips()[0].id();
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::from_ratio(1, 32).unwrap(),
                duration: short_note,
                pitch: 67,
                channel: 2,
                velocity: 101,
            })
            .unwrap();
        editor
            .execute(Command::SetLoopRegion {
                start: 512,
                end: 1024,
                enabled: true,
            })
            .unwrap();
        let revision = prepare(&editor, instrument, true).unwrap();
        let instrument_latency = revision.arrangement().main_latency_frames() as usize;
        let (mut onset_frame, mut release_frame) = (0, 0);
        revision.arrangement().tracks()[0]
            .for_each_mutable_midi_span(512, 1024, |span| {
                onset_frame = span.start;
                release_frame = span.gate_end;
            })
            .unwrap();
        assert!((512..1024).contains(&onset_frame));
        let mut playing = SessionPlayback::new(revision).unwrap();
        let mut endpoint = NoteEndpoint {
            instrument_sample: 0.5,
            ..Default::default()
        };
        playing.apply(SessionPlaybackCommand::Play).unwrap();
        endpoint
            .render(&mut playing, &mut [[0.0; 2]; 1024])
            .unwrap();
        let mut dry = vec![[0.0; 2]; (onset_frame - 512 + 8) as usize];
        endpoint.render(&mut playing, &mut dry).unwrap();
        let [(_, onset)] = endpoint.notes.as_slice() else {
            panic!("one onset on the wrapped pass");
        };
        let onset = *onset;
        assert_eq!((onset.on, onset.pitch), (true, 67));
        let cut = playing.arrangement_frame();
        let performance = playing.performance_frame();
        assert_eq!(cut, onset_frame + 8);
        assert_eq!(performance - cut, 512);

        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 512,
                enabled: true,
            })
            .unwrap();
        let mut moved = SessionPlayback::new(prepare(&editor, instrument, true).unwrap()).unwrap();
        moved.try_inherit_state_live_from(&mut playing).unwrap();
        assert_eq!(
            (moved.arrangement_frame(), moved.performance_frame()),
            (cut, performance)
        );
        endpoint.render(&mut moved, &mut [[0.0; 2]; 8]).unwrap();
        assert_eq!(moved.arrangement_frame(), cut + 8);
        assert!(endpoint.notes.is_empty());

        let state = state(19).unwrap();
        let (owner, command) = if on_main {
            (
                ProcessorChainOwner::Main,
                Command::InsertMainDevice {
                    before: None,
                    state,
                },
            )
        } else {
            (
                ProcessorChainOwner::Track(track.value()),
                Command::InsertDevice {
                    track,
                    before: None,
                    state,
                },
            )
        };
        let Created::Device(device) = editor.execute(command).unwrap() else {
            panic!("one hosted effect");
        };
        let effect = BorrowedDefinition {
            binding_epoch: Some(1),
            unbounded_tail: false,
            instances: vec![(
                ProcessorInstanceIdentity::new(owner, device.value()).unwrap(),
                3,
            )],
        };
        let synth = InstrumentDefinition {
            identity: instrument,
            second: None,
            instrument_profile: true,
            epoch: 1,
        };
        let definitions: [&dyn NativeDeviceDefinition; 2] = [&synth, &effect];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let prepare_effect_edit = |editor: &Editor| {
            let candidate = PreparedRevision::prepare_with_effect_registry(
                editor.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(SineTable::new()),
                None,
                &registry,
            )
            .unwrap();
            SessionPlayback::new(Arc::new(candidate)).unwrap()
        };
        let mut inserted = prepare_effect_edit(&editor);
        inserted.try_inherit_state_live_from(&mut moved).unwrap();
        assert_eq!(
            (inserted.arrangement_frame(), inserted.performance_frame()),
            (cut + 8, performance + 8)
        );
        let mut effect = Borrowed::new(owner, device.value());
        // Track insertion starts a fresh compensation epoch. Main can use
        // the instrument origins already retained in the old delay bank.
        if !on_main {
            effect.initial_unstamped_frames = instrument_latency;
        }
        endpoint.effect = Some(effect);
        let mut wet = [[0.0; 2]; 64];
        endpoint.render(&mut inserted, &mut wet).unwrap();
        let dry_sample = *dry.last().unwrap();
        assert_eq!(wet[63], dry_sample.map(|sample| sample * 0.25));
        assert_ne!(wet[63], [0.0; 2]);
        assert!(endpoint.notes.is_empty());

        if !on_main {
            editor.execute(Command::RemoveDevice(device)).unwrap();
            let mut without_active_effect = prepare_effect_edit(&editor);
            assert!(matches!(
                without_active_effect.try_inherit_state_live_from(&mut inserted),
                Err(super::super::foreign_arrangement::INCOMPATIBLE_HANDOFF)
            ));
            assert!(editor.undo());
        }
        editor
            .execute(Command::ToggleDeviceBypass { device })
            .unwrap();
        let mut bypassed = prepare_effect_edit(&editor);
        bypassed.try_inherit_state_live_from(&mut inserted).unwrap();
        let mut aligned_dry = [[0.0; 2]; 64];
        endpoint.render(&mut bypassed, &mut aligned_dry).unwrap();
        assert_eq!(aligned_dry[63], dry_sample);
        assert!(endpoint.notes.is_empty());
        assert_eq!(endpoint.effect.as_ref().unwrap().processed, 128);

        let mut playing = if on_main {
            bypassed
        } else {
            editor.execute(Command::RemoveDevice(device)).unwrap();
            let mut removed = prepare_effect_edit(&editor);
            removed.try_inherit_state_live_from(&mut bypassed).unwrap();
            assert_eq!(removed.performance_frame(), bypassed.performance_frame());
            assert_eq!(removed.arrangement_frame(), bypassed.arrangement_frame());
            endpoint.effect = None;
            removed
        };
        let release = (release_frame - playing.arrangement_frame()) as usize;
        let mut after = vec![[0.0; 2]; release + 1];
        endpoint.render(&mut playing, &mut after).unwrap();
        assert_eq!(after.last().unwrap(), &dry_sample);
        assert_eq!(
            endpoint.notes,
            [(
                release,
                hex_lattice::ForeignNote {
                    on: false,
                    velocity: 0,
                    ..onset
                }
            )]
        );
    }
}

#[test]
fn incompatible_edits_wait_for_the_original_note_off() {
    let (mut editor, identity) = fixture();
    let original = prepare(&editor, identity, true).unwrap();
    let prepared_track = &original.arrangement().tracks()[0];
    let track = prepared_track.canonical_track_id().unwrap();
    let mut note = None;
    prepared_track
        .for_each_mutable_midi_span(0, 1, |span| note = Some(span.note))
        .unwrap();
    let mut replacement =
        SessionPlayback::new(prepare_epoch(&editor, identity, true, 2).unwrap()).unwrap();
    let mut playing = SessionPlayback::new(original).unwrap();
    let mut endpoint = NoteEndpoint::default();
    playing.apply(SessionPlaybackCommand::Play).unwrap();
    endpoint.render(&mut playing, &mut [[0.0; 2]; 2]).unwrap();
    let onset = endpoint.notes[0].1;
    assert!(matches!(
        replacement.inherit_state(&mut playing),
        Err(super::super::foreign_arrangement::INCOMPATIBLE_HANDOFF)
    ));

    editor
        .execute(Command::RemoveMidiNote(note.unwrap()))
        .unwrap();
    let mut removed_note = SessionPlayback::new(prepare(&editor, identity, true).unwrap()).unwrap();
    assert!(removed_note.inherit_state(&mut playing).is_err());
    assert!(editor.undo());

    editor.execute(Command::RemoveInstrument { track }).unwrap();
    let mut removed = SessionPlayback::new(prepare(&editor, identity, true).unwrap()).unwrap();
    assert!(removed.inherit_state(&mut playing).is_err());
    assert_eq!(playing.performance_frame(), 2);
    playing.apply(SessionPlaybackCommand::Stop).unwrap();
    assert!(removed.inherit_state(&mut playing).is_err());
    endpoint.render(&mut playing, &mut [[0.0; 2]; 2]).unwrap();
    assert_eq!(endpoint.notes.len(), 1);
    assert!(!endpoint.notes[0].1.on);
    assert_eq!(endpoint.notes[0].1.note_id, onset.note_id);
    removed.inherit_state(&mut playing).unwrap();
}

#[test]
fn loop_and_transport_release_notes_with_the_identity_of_the_current_pass() {
    for partition in [1, 7, 64, 128, 256, 512] {
        for stop in [false, true] {
            let (mut editor, identity) = fixture();
            editor
                .execute(Command::SetLoopRegion {
                    start: 0,
                    end: 11,
                    enabled: true,
                })
                .unwrap();
            let revision = prepare(&editor, identity, true).unwrap();
            let mut playback = SessionPlayback::new(revision).unwrap();
            let mut endpoint = NoteEndpoint::default();
            playback.apply(SessionPlaybackCommand::Play).unwrap();
            let mut frames = [[0.0; 2]; 1025];
            let mut events = Vec::new();
            let mut origin = 0;
            for chunk in frames.chunks_mut(partition) {
                endpoint.render(&mut playback, chunk).unwrap();
                events.extend(
                    endpoint
                        .notes
                        .iter()
                        .map(|(offset, note)| (origin + offset, *note)),
                );
                origin += chunk.len();
            }
            let (frame, first) = events.first().copied().expect("initial onset");
            assert_eq!(frame, 0);
            assert!(first.on && first.pitch == 60 && first.channel == 2 && first.velocity == 101);
            let wraps = (frames.len() - 1) / 11;
            assert_eq!(
                events.len(),
                1 + 2 * wraps,
                "partition={partition} stop={stop}"
            );
            let mut current = first;
            let mut ids = std::collections::HashSet::from([first.note_id]);
            for wrap in 1..=wraps {
                let (off_frame, off) = events[2 * wrap - 1];
                let (on_frame, on) = events[2 * wrap];
                assert_eq!((off_frame, on_frame), (11 * wrap, 11 * wrap));
                assert!(
                    !off.on
                        && off.note_id == current.note_id
                        && off.pitch == 60
                        && off.channel == 2
                );
                assert!(on.on && on.pitch == 60 && on.channel == 2 && on.velocity == 101);
                assert!(ids.insert(on.note_id), "reused note ID at wrap {wrap}");
                current = on;
            }
            playback
                .apply(if stop {
                    SessionPlaybackCommand::Stop
                } else {
                    SessionPlaybackCommand::Pause
                })
                .unwrap();
            endpoint.render(&mut playback, &mut [[0.0; 2]; 1]).unwrap();
            assert_eq!(
                endpoint.notes,
                [(
                    0,
                    hex_lattice::ForeignNote {
                        on: false,
                        velocity: 0,
                        ..current
                    }
                )],
                "partition={partition} stop={stop}"
            );
        }
    }
}

#[test]
fn timed_transport_preserves_foreign_note_offsets_and_rejects_the_staged_callback() {
    use crate::playback::{Playback, PlaybackCommand};
    use crate::{TimedTransportCommand as Timed, TimedTransportError};
    let (editor, identity) = fixture();
    let revision = prepare(&editor, identity, true).unwrap();
    let commands = [
        Timed {
            offset: 0,
            command: PlaybackCommand::PlayFrom(0),
        },
        Timed {
            offset: 5,
            command: PlaybackCommand::Pause,
        },
        Timed {
            offset: 5,
            command: PlaybackCommand::PlayFrom(0),
        },
        Timed {
            offset: 10,
            command: PlaybackCommand::Pause,
        },
    ];
    let mut playback = Playback::from_prepared(Arc::clone(&revision));
    let mut endpoint = NoteEndpoint {
        instrument_sample: 0.25,
        ..Default::default()
    };
    let mut output = [[0.0; 2]; 16];
    playback
        .render_with_timed_transport_and_foreign_processors(&mut output, &commands, &mut endpoint)
        .unwrap();
    assert!(!endpoint.rejected);
    let events = &endpoint.notes;
    assert_eq!(events.len(), 4, "{events:?}");
    assert_eq!((events[0].0, events[0].1.on), (0, true));
    assert_eq!((events[1].0, events[1].1.on), (5, false));
    assert_eq!((events[2].0, events[2].1.on), (5, true));
    assert_eq!((events[3].0, events[3].1.on), (10, false));
    assert_eq!(events[0].1.note_id, events[1].1.note_id);
    assert_eq!(events[2].1.note_id, events[3].1.note_id);
    assert_ne!(events[0].1.note_id, events[2].1.note_id);

    let mut invalid = Playback::from_prepared(Arc::clone(&revision));
    let mut endpoint = NoteEndpoint::default();
    let mut untouched = [[9.0; 2]; 16];
    assert_eq!(
        invalid.render_with_timed_transport_and_foreign_processors(
            &mut untouched,
            &[Timed {
                offset: 17,
                command: PlaybackCommand::Play
            }],
            &mut endpoint
        ),
        Err(TimedTransportError::InvalidOffset { index: 0 })
    );
    assert_eq!(untouched, [[9.0; 2]; 16]);
    assert!(!endpoint.rejected && endpoint.notes.is_empty());
    assert!(!invalid.playing());

    let mut failing = Playback::from_prepared(Arc::clone(&revision));
    let mut endpoint = NoteEndpoint {
        fail_at: Some(6),
        ..Default::default()
    };
    let mut output = [[9.0; 2]; 16];
    assert!(matches!(
        failing.render_with_timed_transport_and_foreign_processors(
            &mut output,
            &commands,
            &mut endpoint
        ),
        Err(TimedTransportError::Render { offset: 5, .. })
    ));
    assert!(endpoint.rejected);
    assert_eq!(output, [[0.0; 2]; 16]);

    let mut failing = Playback::from_prepared(revision);
    let mut endpoint = NoteEndpoint::default();
    let mut output = [[9.0; 2]; 16];
    let commands = [
        commands[0],
        Timed {
            offset: 5,
            command: PlaybackCommand::PlayFrom(u64::MAX),
        },
    ];
    assert!(matches!(
        failing.render_with_timed_transport_and_foreign_processors(
            &mut output,
            &commands,
            &mut endpoint
        ),
        Err(TimedTransportError::Action { index: 1, .. })
    ));
    assert!(endpoint.rejected);
    assert_eq!(output, [[0.0; 2]; 16]);
}

#[test]
fn timed_mixed_pages_and_foreign_instrument_keep_offsets_and_refusal() {
    use crate::compile::PreparedAudioSource;
    use crate::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
    use crate::playback::{Playback, PlaybackCommand};
    use crate::{
        BorrowedMediaPage, MediaPages, STREAM_PAGE_FRAMES, StreamSourceKey, StreamedSourceMetadata,
        TimedTransportCommand as Timed, TimedTransportError,
    };
    use std::sync::atomic::AtomicBool;

    struct Pages {
        key: StreamSourceKey,
        data: Vec<Frame>,
        missing: bool,
        requests: Vec<(StreamSourceKey, u64)>,
    }
    impl MediaPages for Pages {
        fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
            if self.missing || key != self.key {
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
        fn request_page(&mut self, key: StreamSourceKey, index: u64) {
            self.requests.push((key, index));
        }
    }
    let (mut editor, identity) = fixture();
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
            state: crate::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
            )
            .unwrap(),
        })
        .unwrap();
    let key = StreamSourceKey::new(23, asset);
    let source =
        StreamedSourceMetadata::new(key, 48_000, 48_000, 8192, 8192, 0, [0x48; 32]).unwrap();
    let media = std::collections::BTreeMap::from([(
        asset,
        PreparedAudioSource::Streamed(Arc::new(source)),
    )]);
    let cancelled = AtomicBool::new(false);
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        editor.project(),
        1_000_000,
        &cancelled,
    )
    .unwrap();
    let definition = InstrumentDefinition {
        identity,
        second: None,
        instrument_profile: true,
        epoch: 1,
    };
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
            &crate::audio_transform::AudioTransformPreparation::default(),
            &resources,
            100_000,
            &cancelled,
            &registry,
            None,
        )
        .unwrap(),
    );
    assert_eq!(revision.arrangement().main_latency_frames(), 19);
    let playback = || {
        Playback::try_from_prepared_with_device_resources(Arc::clone(&revision), &resources)
            .unwrap()
    };
    let pcm: Vec<_> = (0..8192)
        .map(|frame| {
            let value = frame as f32 / 16384.0;
            [value, -value]
        })
        .collect();
    let mut pages = Pages {
        key,
        data: pcm.clone(),
        missing: false,
        requests: Vec::new(),
    };
    let commands = [
        Timed {
            offset: 0,
            command: PlaybackCommand::PlayFrom(0),
        },
        Timed {
            offset: 32,
            command: PlaybackCommand::PlayFrom(4093),
        },
        Timed {
            offset: 64,
            command: PlaybackCommand::Pause,
        },
    ];
    let mut runtime = playback();
    let mut endpoint = NoteEndpoint::default();
    let mut output = [[0.0; 2]; 80];
    let mut master = [[0.0; 2]; 80];
    let mut track_output = [[0.0; 2]; 80];
    let mut clock = [None; 80];
    let mut node_calls = Vec::new();
    {
        let mut observe = |signal, offset, frames: &[Frame]| {
            let target = match signal {
                crate::ProjectSignal::Master => &mut master,
                crate::ProjectSignal::Track(id) if id == track.value() => &mut track_output,
                other => panic!("unexpected signal {other:?}"),
            };
            target[offset..offset + frames.len()].copy_from_slice(frames);
        };
        let mut timing = |offset, frame, len, _: &crate::PreparedTempoMap| {
            for i in 0..len {
                clock[offset + i] = Some(frame + i as u64);
            }
        };
        let tracks = [track.value()];
        let mut tap = crate::ProjectOutputTap::new(&tracks, &mut observe)
            .unwrap()
            .with_timing(&mut timing);
        runtime
            .render_with_timed_transport_foreign_and_signal_observers(
                &mut output,
                &commands,
                Some(&mut pages),
                Some(&mut |identity, phase, offset, frames: &[Frame]| {
                    node_calls.push((identity, phase, offset, frames.len()))
                }),
                Some(&mut tap),
                &mut endpoint,
            )
            .unwrap();
    }
    assert_eq!(master, output);
    assert!(track_output.iter().flatten().any(|value| *value != 0.0));
    assert!(!node_calls.is_empty());
    assert!(
        node_calls
            .iter()
            .all(|(_, _, offset, len)| offset + len <= 80)
    );
    for offset in [0, 32] {
        assert!(node_calls.iter().any(|(_, _, at, _)| *at == offset));
    }
    assert_eq!(clock[0], Some(0));
    assert_eq!(clock[32], Some(4093));
    assert_eq!(clock[64], None);
    let events = &endpoint.notes;
    assert_eq!(events.len(), 4, "{events:?}");
    assert_eq!((events[0].0, events[0].1.on), (0, true));
    assert_eq!((events[1].0, events[1].1.on), (32, false));
    assert_eq!((events[2].0, events[2].1.on), (32, true));
    assert_eq!((events[3].0, events[3].1.on), (64, false));
    assert_eq!(events[0].1.note_id, events[1].1.note_id);
    assert_eq!(events[2].1.note_id, events[3].1.note_id);
    assert_ne!(events[0].1.note_id, events[2].1.note_id);
    assert!(pages.requests.is_empty());
    assert_eq!(&output[19..32], &pcm[..13]);
    assert_eq!(&output[51..64], &pcm[4093..4106]);

    let mut missing = Pages {
        key,
        data: pcm.clone(),
        missing: true,
        requests: Vec::new(),
    };
    let mut runtime = playback();
    let mut endpoint = NoteEndpoint::default();
    let mut output = [[9.0; 2]; 80];
    let mut missing_node_calls = 0;
    let mut missing_project_calls = 0;
    let mut project = |_, _, _: &[Frame]| missing_project_calls += 1;
    let mut tap = crate::ProjectOutputTap::new(&[], &mut project).unwrap();
    runtime
        .render_with_timed_transport_foreign_and_signal_observers(
            &mut output,
            &commands,
            Some(&mut missing),
            Some(&mut |_, _, _, _: &[Frame]| missing_node_calls += 1),
            Some(&mut tap),
            &mut endpoint,
        )
        .unwrap();
    assert!(missing_node_calls > 0 && missing_project_calls > 0);
    assert_eq!(output, [[0.0; 2]; 80]);
    assert_eq!(&endpoint.notes, events);
    assert!(missing.requests.contains(&(key, 0)));
    assert!(missing.requests.contains(&(key, 1)));
    assert!(missing.requests.iter().all(|(source, _)| *source == key));

    let mut invalid = playback();
    let mut endpoint = NoteEndpoint::default();
    let mut untouched = [[9.0; 2]; 80];
    let mut untouched_pages = Pages {
        key,
        data: pcm.clone(),
        missing: true,
        requests: Vec::new(),
    };
    let mut invalid_calls = 0;
    assert_eq!(
        invalid.render_with_timed_transport_foreign_and_signal_observers(
            &mut untouched,
            &[Timed {
                offset: 81,
                command: PlaybackCommand::Play
            }],
            Some(&mut untouched_pages),
            Some(&mut |_, _, _, _: &[Frame]| invalid_calls += 1),
            None,
            &mut endpoint,
        ),
        Err(TimedTransportError::InvalidOffset { index: 0 })
    );
    assert_eq!(untouched, [[9.0; 2]; 80]);
    assert!(untouched_pages.requests.is_empty() && endpoint.notes.is_empty() && !endpoint.rejected);
    assert!(!invalid.playing());
    assert_eq!(invalid_calls, 0);

    let mut runtime = playback();
    let mut endpoint = NoteEndpoint {
        fail_at: Some(33),
        ..Default::default()
    };
    let mut output = [[9.0; 2]; 80];
    let mut prefix_calls = 0;
    let mut project_calls = 0;
    let mut project = |_, _, _: &[Frame]| project_calls += 1;
    let mut tap = crate::ProjectOutputTap::new(&[], &mut project).unwrap();
    assert!(matches!(
        runtime.render_with_timed_transport_foreign_and_signal_observers(
            &mut output,
            &commands,
            Some(&mut pages),
            Some(&mut |_, _, offset, frames: &[Frame]| {
                assert!(offset + frames.len() <= 80);
                prefix_calls += 1;
            }),
            Some(&mut tap),
            &mut endpoint
        ),
        Err(TimedTransportError::Render { offset: 32, .. })
    ));
    assert!(endpoint.rejected);
    assert!(prefix_calls > 0 && project_calls > 0);
    assert_eq!(output, [[0.0; 2]; 80]);
}
