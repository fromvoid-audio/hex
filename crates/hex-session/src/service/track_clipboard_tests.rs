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
