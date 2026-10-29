use super::*;

fn mutable_definition(
    id: hex_lattice_contract::DeviceTypeId,
    release_frames: u64,
) -> MutableFeedbackDefinition {
    MutableFeedbackDefinition {
        metadata: ContinuityInstrumentDefinition {
            id,
            levels: [0.2, 0.4],
            release_frames,
        },
    }
}

fn prepared_with_registry(
    project: &Project,
    registry: &hex_lattice::NativeInstrumentRegistry<'_>,
) -> Result<Arc<PreparedRevision>, crate::Invalid> {
    prepared_with_registry_and_previous(
        project,
        registry,
        Arc::new(hex_devices::SineTable::new()),
        None,
    )
}

fn prepared_with_registry_and_previous(
    project: &Project,
    registry: &hex_lattice::NativeInstrumentRegistry<'_>,
    sine_table: Arc<hex_devices::SineTable>,
    previous: Option<&PreparedSessionInventory>,
) -> Result<Arc<PreparedRevision>, crate::Invalid> {
    Ok(Arc::new(
        PreparedRevision::prepare_with_instrument_registry(
            project,
            &BTreeMap::new(),
            sine_table,
            previous,
            registry,
        )?,
    ))
}

fn mutable_session_loop_project(id: hex_lattice_contract::DeviceTypeId) -> Project {
    let project = with_test_instrument(&project(1.0, 0, 1 << 32), id, 0);
    let mut snapshot = project.snapshot();
    let first_note_id = snapshot.next_id;
    let second_note_id = first_note_id + 1;
    snapshot.tracks[0].content = TrackContentSnapshot::Instrument(Vec::new());
    let Some(SessionCellContentSnapshot::Midi(clip)) = snapshot
        .session
        .cells
        .first_mut()
        .map(|cell| &mut cell.content)
    else {
        panic!("fixture contains a Session MIDI source");
    };
    clip.source.source_length_raw = 4 << 32;
    clip.source.source_start_raw = 0;
    clip.source.loop_start_raw = 2 << 32;
    clip.source.loop_end_raw = 3 << 32;
    clip.source.looping = true;
    clip.source.notes = vec![
        MidiNoteSnapshot {
            active: true,
            id: first_note_id,
            start_raw: 1 << 32,
            duration_raw: 2 << 32,
            pitch: 60,
            channel: 0,
            velocity: 100,
        },
        MidiNoteSnapshot {
            active: true,
            id: second_note_id,
            start_raw: 2 << 32,
            duration_raw: 1 << 24,
            pitch: 64,
            channel: 0,
            velocity: 100,
        },
    ];
    snapshot.next_id = second_note_id + 1;
    Project::from_snapshot(snapshot).unwrap()
}

fn single_repeating_session_loop_project(id: hex_lattice_contract::DeviceTypeId) -> Project {
    let project = mutable_session_loop_project(id);
    let mut snapshot = project.snapshot();
    let Some(SessionCellContentSnapshot::Midi(clip)) = snapshot
        .session
        .cells
        .first_mut()
        .map(|cell| &mut cell.content)
    else {
        panic!("fixture contains a Session MIDI source");
    };
    clip.source.notes = vec![clip.source.notes[1].clone()];
    Project::from_snapshot(snapshot).unwrap()
}

fn render_initial_block(revision: Arc<PreparedRevision>) -> Vec<crate::Frame> {
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = vec![[0.0; 2]; 256];
    playback.render(&mut output).unwrap();
    output
}

#[test]
fn registered_event_cap_rejects_candidate_before_history_and_preserves_old_pcm() {
    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(1)));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(64)));
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = mutable_definition(id, 8);
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let project = with_test_instrument(&project(1.0, 0, 1 << 32), id, 0);
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::AddTrack {
            name: "redo branch".into(),
        })
        .unwrap();
    assert!(editor.undo());
    assert!(editor.can_redo());

    let before_project = editor.project().clone();
    let clip = before_project.tracks()[0].midi_clips()[0].id();
    let previous = prepared_with_registry(editor.project(), &registry).unwrap();
    let previous_pcm = render_initial_block(Arc::clone(&previous));
    assert!(previous_pcm.iter().any(|frame| *frame != [0.0; 2]));

    let result = editor.execute_prepared(
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).unwrap(),
            pitch: 64,
            channel: 0,
            velocity: 100,
        },
        |candidate| {
            prepared_with_registry(candidate, &registry)
                .map_err(|error| hex_project::Invalid(error.0))
        },
    );
    assert_eq!(
        result.unwrap_err(),
        hex_project::Invalid(
            "mutable midi event density exceeds the instrument capacity per query",
        )
    );
    assert_eq!(editor.project(), &before_project);
    assert!(!editor.can_undo());
    assert!(editor.can_redo());
    assert_eq!(render_initial_block(Arc::clone(&previous)), previous_pcm);

    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(None));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(None));
}

#[test]
fn registered_voice_cap_rejects_overlapping_finite_arrangement_notes() {
    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(256)));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(1)));
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = mutable_definition(id, 8);
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let project = with_test_instrument(&project(1.0, 0, 1 << 32), id, 0);
    let mut snapshot = project.snapshot();
    let clip = match &mut snapshot.tracks[0].content {
        TrackContentSnapshot::Instrument(clips)
        | TrackContentSnapshot::Hybrid {
            midi_clips: clips, ..
        } => clips.first_mut().unwrap(),
        TrackContentSnapshot::Audio(_) => panic!("fixture contains Arrangement MIDI"),
    };
    clip.notes.push(MidiNoteSnapshot {
        active: true,
        id: snapshot.next_id,
        start_raw: 0,
        duration_raw: 1 << 32,
        pitch: 64,
        channel: 0,
        velocity: 100,
    });
    snapshot.next_id += 1;
    let project = Project::from_snapshot(snapshot).unwrap();
    assert_eq!(
        prepared_with_registry(&project, &registry).unwrap_err(),
        crate::Invalid("midi voice density exceeds the instrument capacity per track")
    );
    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(None));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(None));
}

#[test]
fn registered_factory_with_zero_event_capacity_is_refused() {
    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(0)));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(64)));
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let definition = mutable_definition(id, 8);
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let project = with_test_instrument(&project(1.0, 0, 1 << 32), id, 0);

    assert_eq!(
        prepared_with_registry(&project, &registry).unwrap_err(),
        crate::Invalid("mutable instrument capacity declaration is unsupported")
    );
    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(None));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(None));
}

#[test]
fn session_loop_admission_checks_device_caps_at_the_first_seam() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let project = mutable_session_loop_project(id);
    let definition = mutable_definition(id, 8);
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();

    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(2)));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(64)));
    let event_result = prepared_with_registry(&project, &registry);
    assert_eq!(
        event_result.unwrap_err(),
        crate::Invalid("mutable midi source exceeds instrument event capacity")
    );

    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(256)));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(1)));
    let voice_result = prepared_with_registry(&project, &registry);
    assert_eq!(
        voice_result.unwrap_err(),
        crate::Invalid("mutable midi source exceeds instrument voice capacity")
    );

    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(None));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(None));
}

#[test]
fn cached_session_template_revalidates_a_lower_registered_event_capacity() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let project = single_repeating_session_loop_project(id);
    let definition = mutable_definition(id, 8);
    let definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&definition];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();
    let sine_table = Arc::new(hex_devices::SineTable::new());
    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(256)));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(64)));
    let previous =
        prepared_with_registry_and_previous(&project, &registry, Arc::clone(&sine_table), None)
            .unwrap();
    let clip = previous
        .session()
        .midi_clips()
        .first()
        .expect("fixture has one Session MIDI clip");
    assert!(clip.instrument_template().is_event_source());

    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(1)));
    let result = prepared_with_registry_and_previous(
        &project,
        &registry,
        sine_table,
        Some(previous.session()),
    );
    assert_eq!(
        result.unwrap_err(),
        crate::Invalid("mutable midi source exceeds instrument event capacity")
    );
    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(None));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(None));
}

#[test]
fn cached_session_template_recomputes_voice_proof_when_release_limit_changes() {
    let id = hex_lattice_contract::DeviceTypeId::from_static("test.instrument.mutable.feedback");
    let project = single_repeating_session_loop_project(id);
    let old_definition = mutable_definition(id, 8);
    let old_definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&old_definition];
    let old_registry = hex_lattice::NativeInstrumentRegistry::new(&old_definitions).unwrap();
    let sine_table = Arc::new(hex_devices::SineTable::new());
    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(256)));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(Some(1)));
    let previous =
        prepared_with_registry_and_previous(&project, &old_registry, Arc::clone(&sine_table), None)
            .unwrap();
    let old_clip = previous
        .session()
        .midi_clips()
        .first()
        .expect("fixture has one Session MIDI clip");
    assert_eq!(old_clip.work.active_voices, 1);

    // Keep type, state, source and device caps identical, but expand the
    // factory's declared release tail beyond the one-beat source period.
    let new_definition = mutable_definition(id, 30_000);
    let new_definitions: [&dyn hex_lattice::NativeInstrumentDefinition; 1] = [&new_definition];
    let new_registry = hex_lattice::NativeInstrumentRegistry::new(&new_definitions).unwrap();
    let result = prepared_with_registry_and_previous(
        &project,
        &new_registry,
        sine_table,
        Some(previous.session()),
    );
    assert_eq!(
        result.unwrap_err(),
        crate::Invalid("mutable midi source exceeds instrument voice capacity")
    );
    MUTABLE_EVENT_CAPACITY_OVERRIDE.with(|limit| limit.set(None));
    MUTABLE_VOICE_CAPACITY_OVERRIDE.with(|limit| limit.set(None));
}
