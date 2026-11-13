//! Authoring and persistence through the same worker contract used by GPUI.
mod common;

use common::wait;
use hex_project::{BeatTime, Command, Created};
use hex_session::service::{Event, Request, Service};
use std::fs;

fn edit(service: &Service, command: Command) -> Event {
    service.submit(Request::Edit(command)).unwrap();
    wait(service)
}

#[test]
fn authored_notes_survive_edit_delete_undo_save_and_reopen() {
    let service = Service::new(44_100, 32, 8_000_000).unwrap();
    wait(&service);
    let Some(Created::Track(track)) = edit(
        &service,
        Command::AddMidiTrack {
            name: "original instrument".into(),
        },
    )
    .created
    else {
        panic!("track expected")
    };
    let Some(Created::Clip(clip)) = edit(
        &service,
        Command::PlaceMidiClip {
            track,
            start: BeatTime::ONE,
            length: BeatTime::from_quarters(4).unwrap(),
        },
    )
    .created
    else {
        panic!("clip expected")
    };
    let authored = edit(
        &service,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        },
    );
    let Some(Created::Note(note)) = authored.created else {
        panic!("note expected")
    };
    let edited = edit(
        &service,
        Command::UpdateMidiNote {
            note,
            start: BeatTime::from_ratio(1, 4).unwrap(),
            duration: BeatTime::from_ratio(3, 2).unwrap(),
            pitch: 64,
            channel: 0,
            velocity: 92,
        },
    );
    let expected = edited.state.project.clone();
    // A gesture's final compound update restores onset, gate and pitch together.
    service.submit(Request::Undo).unwrap();
    let gesture_undo = wait(&service);
    assert_eq!(gesture_undo.state.project, authored.state.project);
    assert!(gesture_undo.playback.is_some());
    service.submit(Request::Redo).unwrap();
    let gesture_redo = wait(&service);
    assert_eq!(gesture_redo.state.project, expected);
    assert!(gesture_redo.playback.is_some());
    let deleted = edit(&service, Command::RemoveMidiNote(note));
    assert!(
        deleted
            .state
            .project
            .midi_clip(clip)
            .unwrap()
            .notes()
            .is_empty()
    );
    service.submit(Request::Undo).unwrap();
    let undone = wait(&service);
    assert_eq!(undone.state.project, expected);
    assert!(undone.playback.is_some());
    let Some(Created::Clip(copy)) = edit(&service, Command::DuplicateMidiClip(clip)).created else {
        panic!("copy expected")
    };
    let moved = edit(
        &service,
        Command::MoveMidiClip {
            clip: copy,
            track,
            start: BeatTime::from_quarters(8).unwrap(),
        },
    );
    assert_eq!(
        moved.state.project.midi_clip(copy).unwrap().start(),
        BeatTime::from_quarters(8).unwrap()
    );
    let expected = moved.state.project.clone();
    let path = std::env::temp_dir().join(format!("hex-authored-midi-{}.hex", std::process::id()));
    service.submit(Request::SaveNew(path.clone())).unwrap();
    let saved = wait(&service);
    assert!(saved.save_complete && !saved.state.unsaved);
    assert_eq!(hex_storage::load(&path).unwrap(), *expected);
    let removed = edit(&service, Command::RemoveMidiClip(copy));
    assert!(removed.state.project.midi_clip(copy).is_none());
    service.submit(Request::Open(path.clone())).unwrap();
    let opened = wait(&service);
    fs::remove_file(path).unwrap();
    assert!(opened.open_complete && opened.project_replaced);
    assert_eq!(opened.state.project, expected);
    assert!(!opened.state.unsaved);
    assert_eq!(opened.state.project.midi_note(note).unwrap().pitch(), 64);
    assert_eq!(opened.state.project.midi_note(note).unwrap().velocity(), 92);
    assert!(opened.playback.is_some());
}
