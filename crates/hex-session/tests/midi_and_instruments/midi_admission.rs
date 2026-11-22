//! Playable MIDI admission and failed candidate preparation remain transactional.
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
use hex_project::{BeatTime, Command, Created, Editor, MidiNoteInsert, MidiNoteUpdate, Project};
use hex_session::service::{Event, Request, Service};
use std::{
    fs, thread,
    time::{Duration, Instant},
};

fn event(service: &Service) -> Event {
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("missing worker event: {error}"),
        }
    }
}

fn edit(service: &Service, command: Command) -> Event {
    service.submit(Request::Edit(command)).unwrap();
    event(service)
}

// Keep 129 real onsets inside the admission window without authoring illegal
// same-pitch overlap or pitches above the 8 kHz fixture's Nyquist limit.
fn dense_note(index: usize, base: BeatTime) -> MidiNoteInsert {
    let duration = BeatTime::from_ratio(1, 1024).unwrap();
    MidiNoteInsert {
        active: true,
        start: BeatTime::from_raw(base.raw() + (index / 32) as u64 * duration.raw()),
        duration,
        pitch: 48 + (index % 32) as u8,
        channel: 0,
        velocity: 100,
    }
}

fn populated(service: &Service) -> (hex_project::ClipId, Event) {
    use hex_project::{BeatTime, Created};
    let track_event = edit(
        service,
        Command::InsertTrackWithDevice {
            name: "instrument".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        },
    );
    let Some(Created::Track(track)) = track_event.created else {
        panic!("track expected")
    };
    let clip_event = edit(
        service,
        Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(20).unwrap(),
            length: BeatTime::ONE,
        },
    );
    let Some(Created::Clip(clip)) = clip_event.created else {
        panic!("clip expected")
    };
    let note_event = edit(
        service,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        },
    );
    assert!(note_event.error.is_none());
    assert!(note_event.playback.is_some());
    (clip, note_event)
}

#[test]
fn unrenderable_edit_preserves_revision_history_ids_and_working_graph() {
    use hex_project::BeatTime;
    let service = Service::new(8_000, 16, 32).unwrap();
    assert!(event(&service).playback.is_some());
    let (clip, working) = populated(&service);
    assert!(working.state.timeline_frames > 84_000);
    service.submit(Request::Undo).unwrap();
    let undone = event(&service);
    assert!(undone.state.can_redo);
    let rejected = edit(
        &service,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 127,
            channel: 0,
            velocity: 100,
        },
    );
    assert!(rejected.error.is_some());
    assert_eq!(rejected.state.project, undone.state.project);
    assert_eq!(rejected.state.revision, undone.state.revision);
    assert_eq!(rejected.state.unsaved, undone.state.unsaved);
    assert!(rejected.state.can_redo);
    assert!(rejected.playback.is_none());
    service.submit(Request::Redo).unwrap();
    let mut redone = event(&service);
    assert!(redone.error.is_none());
    assert_eq!(redone.state.project, working.state.project);
    let playback = redone.playback.as_mut().unwrap();
    playback.apply(hex_engine::playback::PlaybackCommand::Seek(80_100));
    playback.apply(hex_engine::playback::PlaybackCommand::Play);
    let mut frames = [[0.0; 2]; 256];
    playback.render(&mut frames).unwrap();
    assert!(frames.iter().any(|frame| frame[0].abs() > 0.001));
}

#[test]
fn unsupported_return_effect_rejects_prepared_edit_without_history_commit() {
    let service = Service::new(8_000, 16, 32).unwrap();
    let initial = event(&service);
    assert!(initial.playback.is_some());
    let added = edit(
        &service,
        Command::AddReturnTrack {
            name: "return".into(),
        },
    );
    let Some(Created::ReturnTrack(return_track)) = added.created else {
        panic!("return track expected")
    };
    assert!(added.playback.is_some());
    let unsupported = NativeDeviceState::new(
        DeviceTypeId::from_static("test.unregistered.return.effect"),
        1,
        vec![1],
    )
    .unwrap();
    let rejected = edit(
        &service,
        Command::InsertReturnDevice {
            return_track,
            before: None,
            state: unsupported,
        },
    );
    assert!(rejected.error.is_some());
    assert_eq!(rejected.state.project, added.state.project);
    assert_eq!(rejected.state.revision, added.state.revision);
    assert_eq!(rejected.state.unsaved, added.state.unsaved);
    assert!(rejected.state.can_undo);
    assert!(rejected.playback.is_none());
    service.submit(Request::Undo).unwrap();
    let undone = event(&service);
    assert!(undone.state.project.return_tracks().is_empty());
}

#[test]
fn open_playable_instrument_and_reject_invalid_candidate_atomically() {
    use hex_project::{BeatTime, Created};
    let path =
        std::env::temp_dir().join(format!("hex-session-midi-open-{}.hex", std::process::id()));
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "instrument".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track expected")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip expected")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    hex_storage::save_new(&path, editor.project()).unwrap();
    let service = Service::new(8_000, 16, 32).unwrap();
    event(&service);
    service.submit(Request::Open(path.clone())).unwrap();
    let opened = event(&service);
    assert!(opened.open_complete && opened.project_replaced);
    assert!(opened.error.is_none());
    assert!(opened.playback.is_some());
    fs::remove_file(&path).unwrap();
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 127,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    hex_storage::save_new(&path, editor.project()).unwrap();
    service.submit(Request::Open(path.clone())).unwrap();
    let refused = event(&service);
    fs::remove_file(path).unwrap();
    assert!(refused.open_complete && !refused.project_replaced);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, opened.state.project);
    assert_eq!(refused.state.revision, opened.state.revision);
    assert_eq!(refused.state.document_path, opened.state.document_path);
    service
        .submit(Request::RestorePlayback { generation: 99 })
        .unwrap();
    let restored = event(&service);
    assert!(restored.error.is_none());
    assert!(restored.playback.is_some());
}

#[test]
fn tempo_edit_that_collapses_a_gate_preserves_the_playable_revision() {
    use hex_project::{BeatTime, Created, Tempo};
    let service = Service::new(8_000, 16, 32).unwrap();
    event(&service);
    assert!(
        edit(
            &service,
            Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(20_000_000).unwrap(),
            }
        )
        .error
        .is_none()
    );
    let track_event = edit(
        &service,
        Command::AddMidiTrack {
            name: "short gate".into(),
        },
    );
    let Some(Created::Track(track)) = track_event.created else {
        panic!("track expected")
    };
    let clip_event = edit(
        &service,
        Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        },
    );
    let Some(Created::Clip(clip)) = clip_event.created else {
        panic!("clip expected")
    };
    let working = edit(
        &service,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 8192).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 100,
        },
    );
    assert!(working.error.is_none());
    let rejected = edit(
        &service,
        Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(999_000_000).unwrap(),
        },
    );
    assert!(
        rejected
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("zero frames")
    );
    assert_eq!(rejected.state.project, working.state.project);
    assert_eq!(rejected.state.revision, working.state.revision);
    assert!(rejected.playback.is_none());
    service.submit(Request::Undo).unwrap();
    let undone = event(&service);
    assert!(
        undone
            .state
            .project
            .midi_clip(clip)
            .unwrap()
            .notes()
            .is_empty()
    );
    service.submit(Request::Redo).unwrap();
    let redone = event(&service);
    assert_eq!(redone.state.project, working.state.project);
    assert!(redone.playback.is_some());
}

#[test]
fn batch_onset_budget_rejection_keeps_dirty_revision_redo_and_project() {
    use hex_engine::midi_compile::MAX_ONSETS_PER_WINDOW;

    let path = std::env::temp_dir().join(format!(
        "hex-session-midi-batch-budget-{}.hex",
        std::process::id()
    ));
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "instrument".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track expected")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(130).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip expected")
    };
    let mut updates = Vec::new();
    for index in 0..=MAX_ONSETS_PER_WINDOW {
        let start = BeatTime::from_ratio(index as u64, 4).unwrap();
        let Created::Note(note) = editor
            .execute(Command::AddMidiNote {
                clip,
                start,
                duration: BeatTime::from_ratio(1, 1024).unwrap(),
                pitch: 48,
                channel: 0,
                velocity: 100,
            })
            .unwrap()
        else {
            panic!("note expected")
        };
        let dense = dense_note(index, BeatTime::ZERO);
        updates.push(MidiNoteUpdate {
            note,
            start: dense.start,
            duration: dense.duration,
            pitch: dense.pitch,
            channel: 0,
            velocity: 100,
        });
    }
    hex_storage::save_new(&path, editor.project()).unwrap();

    let service = Service::new(8_000, 8, 32).unwrap();
    event(&service);
    service.submit(Request::Open(path.clone())).unwrap();
    let opened = event(&service);
    fs::remove_file(path).unwrap();
    assert!(opened.open_complete && opened.project_replaced);
    assert!(opened.error.is_none());
    assert!(opened.playback.is_some());

    assert!(
        edit(
            &service,
            Command::AddMidiTrack {
                name: "temporary one".into(),
            },
        )
        .error
        .is_none()
    );
    assert!(
        edit(
            &service,
            Command::AddMidiTrack {
                name: "temporary two".into(),
            },
        )
        .error
        .is_none()
    );
    service.submit(Request::Undo).unwrap();
    let dirty_with_redo = event(&service);
    assert!(dirty_with_redo.state.unsaved);
    assert!(dirty_with_redo.state.can_redo);
    let rejected = edit(
        &service,
        Command::UpdateMidiNotes {
            clip,
            notes: updates,
        },
    );
    assert!(
        rejected
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("onset density")
    );
    assert!(rejected.playback.is_none());
    assert_eq!(rejected.state.project, dirty_with_redo.state.project);
    assert_eq!(rejected.state.revision, dirty_with_redo.state.revision);
    assert_eq!(rejected.state.unsaved, dirty_with_redo.state.unsaved);
    assert_eq!(rejected.state.can_redo, dirty_with_redo.state.can_redo);

    service.submit(Request::Redo).unwrap();
    let redone = event(&service);
    assert!(redone.error.is_none());
    assert_eq!(
        redone.state.project.tracks().len(),
        dirty_with_redo.state.project.tracks().len() + 1
    );
}

#[test]
fn batch_note_admission_commits_once_and_rejects_density_without_publication() {
    let service = Service::new(8_000, 16, 32).unwrap();
    assert!(event(&service).playback.is_some());
    let track_event = edit(
        &service,
        Command::InsertTrackWithDevice {
            name: "batch instrument".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        },
    );
    let Some(Created::Track(track)) = track_event.created else {
        panic!("track expected")
    };
    let clip_event = edit(
        &service,
        Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(16).unwrap(),
        },
    );
    let Some(Created::Clip(clip)) = clip_event.created else {
        panic!("clip expected")
    };
    let valid_notes = (0..3)
        .map(|index| MidiNoteInsert {
            active: true,
            start: BeatTime::from_quarters(index).unwrap(),
            duration: BeatTime::from_ratio(1, 8).unwrap(),
            pitch: 60 + index as u8,
            channel: index as u8,
            velocity: 70 + index as u8,
        })
        .collect();
    let valid = edit(
        &service,
        Command::AddMidiNotes {
            clip,
            notes: valid_notes,
        },
    );
    assert!(valid.error.is_none());
    assert!(valid.playback.is_some());
    let Some(Created::Notes {
        clip: created_clip,
        first,
        last,
    }) = valid.created
    else {
        panic!("created note range expected")
    };
    assert_eq!(created_clip, clip);
    assert_eq!(last.value() - first.value(), 2);
    assert_eq!(
        valid.state.project.midi_clip(clip).unwrap().notes().len(),
        3
    );
    let batch_project = valid.state.project.clone();
    service.submit(Request::Undo).unwrap();
    let undone_batch = event(&service);
    assert!(undone_batch.error.is_none());
    assert!(undone_batch.playback.is_some());
    assert!(
        undone_batch
            .state
            .project
            .midi_clip(clip)
            .unwrap()
            .notes()
            .is_empty()
    );
    service.submit(Request::Redo).unwrap();
    let redone_batch = event(&service);
    assert!(redone_batch.error.is_none());
    assert!(redone_batch.playback.is_some());
    assert_eq!(redone_batch.state.project, batch_project);

    for name in ["temporary one", "temporary two"] {
        let added = edit(&service, Command::AddMidiTrack { name: name.into() });
        assert!(added.error.is_none());
    }
    service.submit(Request::Undo).unwrap();
    let dirty_with_redo = event(&service);
    assert!(dirty_with_redo.state.unsaved);
    assert!(dirty_with_redo.state.can_redo);
    let baseline_ids = dirty_with_redo
        .state
        .project
        .midi_clip(clip)
        .unwrap()
        .notes()
        .iter()
        .map(|note| note.id().value())
        .collect::<Vec<_>>();
    let rejected = edit(
        &service,
        Command::AddMidiNotes {
            clip,
            notes: (0..=hex_engine::midi_compile::MAX_ONSETS_PER_WINDOW)
                .map(|index| dense_note(index, BeatTime::ZERO))
                .collect(),
        },
    );
    assert!(
        rejected
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("onset density")
    );
    assert!(rejected.playback.is_none());
    assert_eq!(rejected.state.project, dirty_with_redo.state.project);
    assert_eq!(rejected.state.revision, dirty_with_redo.state.revision);
    assert_eq!(rejected.state.unsaved, dirty_with_redo.state.unsaved);
    assert_eq!(rejected.state.can_redo, dirty_with_redo.state.can_redo);
    assert_eq!(
        rejected
            .state
            .project
            .midi_clip(clip)
            .unwrap()
            .notes()
            .iter()
            .map(|note| note.id().value())
            .collect::<Vec<_>>(),
        baseline_ids
    );

    service.submit(Request::Redo).unwrap();
    let redone = event(&service);
    assert!(redone.error.is_none());
    assert!(redone.playback.is_some());
    assert_eq!(
        redone.state.project.tracks().len(),
        dirty_with_redo.state.project.tracks().len() + 1
    );
    assert_eq!(
        redone.state.project.midi_clip(clip).unwrap().notes().len(),
        3
    );
}

#[test]
fn inactive_notes_do_not_consume_playback_density_and_failed_reactivation_is_atomic() {
    let service = Service::new(8_000, 16, 32).unwrap();
    assert!(event(&service).playback.is_some());
    let track = match edit(
        &service,
        Command::InsertTrackWithDevice {
            name: "notes".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        },
    )
    .created
    {
        Some(Created::Track(id)) => id,
        other => panic!("track: {other:?}"),
    };
    let clip = match edit(
        &service,
        Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(16).unwrap(),
        },
    )
    .created
    {
        Some(Created::Clip(id)) => id,
        other => panic!("clip: {other:?}"),
    };
    let inserted = edit(
        &service,
        Command::AddMidiNotes {
            clip,
            notes: (0..=hex_engine::midi_compile::MAX_ONSETS_PER_WINDOW)
                .map(|index| {
                    let mut note = dense_note(index, BeatTime::ZERO);
                    note.active = false;
                    note
                })
                .collect(),
        },
    );
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let ids: Vec<_> = inserted
        .state
        .project
        .midi_clip(clip)
        .unwrap()
        .notes()
        .iter()
        .map(|note| note.id())
        .collect();
    assert_eq!(
        ids.len(),
        hex_engine::midi_compile::MAX_ONSETS_PER_WINDOW + 1
    );
    let rejected = edit(
        &service,
        Command::SetMidiNotesActive {
            clip,
            notes: ids.clone(),
            active: true,
        },
    );
    assert!(
        rejected
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("onset density")
    );
    assert_eq!(rejected.state.project, inserted.state.project);
    assert_eq!(rejected.state.revision, inserted.state.revision);
    assert!(rejected.playback.is_none());
    let accepted = edit(
        &service,
        Command::SetMidiNotesActive {
            clip,
            notes: vec![ids[0]],
            active: true,
        },
    );
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert_eq!(
        accepted
            .state
            .project
            .midi_clip(clip)
            .unwrap()
            .notes()
            .iter()
            .filter(|note| note.active())
            .count(),
        1
    );
}

#[test]
fn mixed_note_edit_admission_preserves_original_revision_on_compile_rejection() {
    let service = Service::new(8_000, 16, 32).unwrap();
    assert!(event(&service).playback.is_some());
    let track_event = edit(
        &service,
        Command::InsertTrackWithDevice {
            name: "mixed instrument".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        },
    );
    let Some(Created::Track(track)) = track_event.created else {
        panic!("track expected")
    };
    let clip_event = edit(
        &service,
        Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(16).unwrap(),
        },
    );
    let Some(Created::Clip(clip)) = clip_event.created else {
        panic!("clip expected")
    };
    let first_event = edit(
        &service,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 8).unwrap(),
            pitch: 48,
            channel: 2,
            velocity: 64,
        },
    );
    let Some(Created::Note(first)) = first_event.created else {
        panic!("note expected")
    };
    let retained_event = edit(
        &service,
        Command::AddMidiNote {
            clip,
            start: BeatTime::from_quarters(4).unwrap(),
            duration: BeatTime::from_ratio(1, 8).unwrap(),
            pitch: 72,
            channel: 4,
            velocity: 96,
        },
    );
    assert!(retained_event.error.is_none());
    let valid = edit(
        &service,
        Command::EditMidiNotes {
            clip,
            remove: vec![first],
            insert: vec![
                MidiNoteInsert {
                    active: true,
                    start: BeatTime::from_ratio(1, 3).unwrap(),
                    duration: BeatTime::from_ratio(1, 16).unwrap(),
                    pitch: 49,
                    channel: 2,
                    velocity: 65,
                },
                MidiNoteInsert {
                    active: true,
                    start: BeatTime::from_ratio(4, 3).unwrap(),
                    duration: BeatTime::from_ratio(1, 16).unwrap(),
                    pitch: 53,
                    channel: 3,
                    velocity: 88,
                },
            ],
        },
    );
    assert!(valid.error.is_none());
    assert!(valid.playback.is_some());
    assert!(matches!(valid.created, Some(Created::Notes { clip: id, .. }) if id == clip));
    assert_eq!(
        valid.state.project.midi_clip(clip).unwrap().notes().len(),
        3
    );
    let mixed_project = valid.state.project.clone();
    service.submit(Request::Undo).unwrap();
    let undone = event(&service);
    assert!(undone.error.is_none());
    assert!(undone.playback.is_some());
    assert_eq!(
        undone.state.project.midi_clip(clip).unwrap().notes().len(),
        2
    );
    assert!(undone.state.project.midi_note(first).is_some());
    service.submit(Request::Redo).unwrap();
    let redone = event(&service);
    assert!(redone.error.is_none());
    assert_eq!(redone.state.project, mixed_project);

    for name in ["temporary one", "temporary two"] {
        assert!(
            edit(&service, Command::AddMidiTrack { name: name.into() })
                .error
                .is_none()
        );
    }
    service.submit(Request::Undo).unwrap();
    let dirty_with_redo = event(&service);
    assert!(dirty_with_redo.state.unsaved);
    assert!(dirty_with_redo.state.can_redo);
    let baseline_ids = dirty_with_redo
        .state
        .project
        .midi_clip(clip)
        .unwrap()
        .notes()
        .iter()
        .map(|note| note.id().value())
        .collect::<Vec<_>>();
    let remove = dirty_with_redo
        .state
        .project
        .midi_clip(clip)
        .unwrap()
        .notes()[0]
        .id();
    let rejected = edit(
        &service,
        Command::EditMidiNotes {
            clip,
            remove: vec![remove],
            insert: (0..=hex_engine::midi_compile::MAX_ONSETS_PER_WINDOW)
                .map(|index| dense_note(index, BeatTime::from_quarters(8).unwrap()))
                .collect(),
        },
    );
    assert!(
        rejected
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("onset density")
    );
    assert!(rejected.playback.is_none());
    assert_eq!(rejected.state.project, dirty_with_redo.state.project);
    assert_eq!(rejected.state.revision, dirty_with_redo.state.revision);
    assert_eq!(rejected.state.unsaved, dirty_with_redo.state.unsaved);
    assert_eq!(rejected.state.can_redo, dirty_with_redo.state.can_redo);
    assert_eq!(
        rejected
            .state
            .project
            .midi_clip(clip)
            .unwrap()
            .notes()
            .iter()
            .map(|note| note.id().value())
            .collect::<Vec<_>>(),
        baseline_ids
    );
    service.submit(Request::Redo).unwrap();
    let redo = event(&service);
    assert!(redo.error.is_none());
    assert!(redo.playback.is_some());
    assert_eq!(
        redo.state.project.tracks().len(),
        dirty_with_redo.state.project.tracks().len() + 1
    );
}

#[test]
fn stable_note_extent_update_prepares_once_preserves_window_and_roundtrips() {
    let service = Service::new(8_000, 16, 32).unwrap();
    event(&service);
    let (clip, mut before) = populated(&service);
    let note = before.state.project.midi_clip(clip).unwrap().notes()[0].id();
    let next_id = before.state.project.snapshot().next_id;
    let mut changed = edit(
        &service,
        Command::UpdateMidiNotesWithSourceExtent {
            clip,
            notes: vec![MidiNoteUpdate {
                note,
                start: BeatTime::from_quarters(5).unwrap(),
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            }],
        },
    );
    assert!(changed.error.is_none(), "{:?}", changed.error);
    assert!(changed.playback.is_some());
    assert_eq!(changed.state.revision, before.state.revision + 1);
    assert_eq!(changed.state.project.snapshot().next_id, next_id);
    let updated = changed.state.project.midi_clip(clip).unwrap();
    assert_eq!(
        (updated.start(), updated.length()),
        (BeatTime::from_quarters(20).unwrap(), BeatTime::ONE)
    );
    assert_eq!(updated.source_length(), BeatTime::from_quarters(6).unwrap());
    assert_eq!(updated.notes()[0].id(), note);
    // Source growth retains the note for future editing; material beyond the
    // finite Arrangement window must stay inaudible.
    let mut original_pcm = vec![[0.0; 2]; 26 * 4_000];
    let mut moved_pcm = original_pcm.clone();
    for (playback, output) in [
        (before.playback.as_mut().unwrap(), &mut original_pcm),
        (changed.playback.as_mut().unwrap(), &mut moved_pcm),
    ] {
        playback.apply(hex_engine::PlaybackCommand::Play);
        playback.render(output).unwrap();
    }
    assert!(
        original_pcm
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1e-6)
    );
    assert!(moved_pcm.iter().flatten().all(|sample| *sample == 0.0));
    let path =
        std::env::temp_dir().join(format!("hex-stable-note-extent-{}.hex", std::process::id()));
    hex_storage::save_new(&path, &changed.state.project).unwrap();
    let reopened = hex_storage::load(&path).unwrap();
    fs::remove_file(path).unwrap();
    assert_eq!(reopened, *changed.state.project);
    service.submit(Request::Undo).unwrap();
    let undone = event(&service);
    assert!(undone.error.is_none() && undone.playback.is_some());
    assert_eq!(undone.state.project, before.state.project);
    service.submit(Request::Redo).unwrap();
    let redone = event(&service);
    assert!(redone.error.is_none() && redone.playback.is_some());
    assert_eq!(redone.state.project, changed.state.project);
}
