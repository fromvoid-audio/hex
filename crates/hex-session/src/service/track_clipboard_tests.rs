//! Clipboard metadata is worker-owned; media and publication use existing owners.
use super::service_tests::{TestDir, next_event, wav_mono};
use super::*;
use hex_project::TrackSelectionIds;

fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = next_event(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}
fn request(event: &Event, action: TrackClipboardAction) -> Request {
    Request::TrackClipboard {
        project_generation: 0,
        revision: event.state.revision,
        action,
    }
}
fn members(event: &Event) -> TrackSelectionIds {
    TrackSelectionIds::new(
        &event
            .state
            .project
            .tracks()
            .iter()
            .map(|t| t.id())
            .collect::<Vec<_>>(),
    )
    .unwrap()
}
#[test]
fn track_clipboard_copy_cut_paste_duplicate_reuses_missing_media_and_one_history_edge() {
    let dir = TestDir::new();
    let path = dir.0.join("original.wav");
    std::fs::write(&path, wav_mono(&[1000; 512])).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next_event(&service);
    let imported = run(&service, Request::Import(path.clone()));
    let original = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "empty".into(),
        }),
    );
    let asset = original.state.project.tracks()[0].clips()[0].asset();
    let copied = run(
        &service,
        request(&original, TrackClipboardAction::Copy(members(&original))),
    );
    assert_eq!(copied.state.revision, original.state.revision);
    assert_eq!(copied.state.project, original.state.project);
    std::fs::remove_file(path).unwrap();
    let pasted = run(
        &service,
        request(&copied, TrackClipboardAction::Paste { before: None }),
    );
    assert_eq!(pasted.state.project.tracks().len(), 4);
    assert_eq!(pasted.state.revision, copied.state.revision + 1);
    assert!(matches!(pasted.created, Some(Created::Tracks { .. })));
    assert!(Arc::ptr_eq(
        &pasted.state.waveforms[&asset],
        &imported.state.waveforms[&asset]
    ));
    let resident = |e: &Event| {
        let source = e.state.canonical_audio_source.as_ref().unwrap();
        let (_, crate::AudioSourceBinding::Resident(audio)) =
            source.required_sources().next().unwrap()
        else {
            panic!("pcm")
        };
        audio.clone()
    };
    assert!(resident(&pasted).shares_pcm_with(&resident(&original)));
    let undone = run(&service, Request::Undo);
    let mut expected = original.state.project.snapshot();
    expected.next_id = undone.state.project.snapshot().next_id;
    assert_eq!(undone.state.project.snapshot(), expected);
    let redone = run(&service, Request::Redo);
    assert_eq!(redone.state.project, pasted.state.project);
    let setup = run(
        &service,
        request(
            &redone,
            TrackClipboardAction::Duplicate {
                tracks: members(&original),
                without_clips: true,
            },
        ),
    );
    assert_eq!(setup.state.project.tracks().len(), 6);
    assert!(
        setup.state.project.tracks()[2..4]
            .iter()
            .all(|t| t.clips().is_empty() && t.midi_clips().is_empty())
    );
    let cut = run(
        &service,
        request(&setup, TrackClipboardAction::Cut(members(&original))),
    );
    assert_eq!(cut.state.project.tracks().len(), 4);
    let restored = run(
        &service,
        request(&cut, TrackClipboardAction::Paste { before: None }),
    );
    assert_eq!(restored.state.project.tracks().len(), 6);
    assert_eq!(restored.state.project.tracks()[4].clips().len(), 1);
    assert!(resident(&restored).shares_pcm_with(&resident(&original)));
}
#[test]
fn track_clipboard_stale_cut_preserves_previous_copy_and_replacement_clears_it() {
    let service = Service::new(48_000, 32, 0).unwrap();
    next_event(&service);
    let first = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "copied".into(),
        }),
    );
    let copy = run(
        &service,
        request(&first, TrackClipboardAction::Copy(members(&first))),
    );
    let second = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "keep".into(),
        }),
    );
    service
        .submit(request(&copy, TrackClipboardAction::Cut(members(&second))))
        .unwrap();
    let refused = next_event(&service);
    assert!(
        refused
            .error
            .as_deref()
            .unwrap()
            .contains("project changed")
    );
    assert_eq!(refused.state.project, second.state.project);
    let pasted = run(
        &service,
        request(&refused, TrackClipboardAction::Paste { before: None }),
    );
    assert_eq!(pasted.state.project.tracks().len(), 3);
    assert_eq!(pasted.state.project.tracks()[2].name(), "copied");
    let new = run(
        &service,
        Request::NewWithAudioMode {
            reservation: None,
            mode: crate::DocumentAudioMode::Resident,
        },
    );
    service
        .submit(Request::TrackClipboard {
            project_generation: 1,
            revision: new.state.revision,
            action: TrackClipboardAction::Paste { before: None },
        })
        .unwrap();
    let empty = next_event(&service);
    assert!(
        empty
            .error
            .as_deref()
            .unwrap()
            .contains("clipboard is empty")
    );
    assert!(empty.state.project.tracks().is_empty());
}

#[test]
fn track_clipboard_full_midi_copy_renders_both_independent_instruments_across_note_off() {
    let service = Service::new(48_000, 32, 0).unwrap();
    let initial = next_event(&service);
    let inserted = run(
        &service,
        Request::InsertBrowserInstrumentTrack {
            project_generation: 0,
            revision: initial.state.revision,
            class: hex_engine::NativeDeviceClass::Instrument,
            type_id: hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sine"),
            new_track_gain: None,
        },
    );
    let track = inserted.state.project.tracks()[0].id();
    let placed = run(
        &service,
        Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE,
        }),
    );
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    let mut original = run(
        &service,
        Request::Edit(Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 80,
        }),
    );
    let mut copied = run(
        &service,
        request(
            &original,
            TrackClipboardAction::Duplicate {
                tracks: members(&original),
                without_clips: false,
            },
        ),
    );
    let mut before = original.playback.take().unwrap();
    let mut after = copied.playback.take().unwrap();
    before.apply(hex_engine::PlaybackCommand::PlayFrom(23_800));
    after.apply(hex_engine::PlaybackCommand::PlayFrom(23_800));
    let mut signal = false;
    for _ in 0..8 {
        let mut expected = [[0.0; 2]; 64];
        let mut actual = [[0.0; 2]; 64];
        before.render(&mut expected).unwrap();
        after.render(&mut actual).unwrap();
        signal |= expected.iter().flatten().any(|sample| *sample != 0.0);
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert_eq!(actual, expected.map(|sample| sample * 2.0));
        }
    }
    assert!(signal, "actual prepared instrument output was exercised");
}

#[test]
fn grouping_commits_and_undo_redo_preserve_canonical_hierarchy() {
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    next_event(&service);
    let added = run(
        &service,
        Request::Edit(Command::AddTrack {
            name: "child".into(),
        }),
    );
    let track = added.state.project.tracks()[0].id();
    let grouped = run(
        &service,
        Request::Edit(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&[track]).unwrap(),
            name: "group".into(),
        }),
    );
    assert!(grouped.state.project.has_group_tracks());
    let undone = run(&service, Request::Undo);
    let mut before = added.state.project.snapshot();
    // Undo preserves the monotonic identity high-water mark.
    before.next_id = grouped.state.project.snapshot().next_id;
    assert_eq!(undone.state.project.snapshot(), before);
    let redone = run(&service, Request::Redo);
    assert_eq!(
        redone.state.project.snapshot(),
        grouped.state.project.snapshot()
    );
}
