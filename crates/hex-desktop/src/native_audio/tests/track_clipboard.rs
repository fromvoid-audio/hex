//! Real resident constructor/publication with controlled callback pumping. No
//! physical output or native UI acceptance is claimed by this headless fixture.
use super::{resident_preview::ResidentHarness, *};
use hex_project::TrackSelectionIds;
use hex_session::service::TrackClipboardAction;
fn request(e: &Event, action: TrackClipboardAction) -> Request {
    Request::TrackClipboard {
        project_generation: 0,
        revision: e.state.revision,
        action,
    }
}
fn applied(h: &mut ResidentHarness, previous: &mut u64, request: Request) -> Event {
    let event = h.run(request);
    *previous = h.applied(*previous).0;
    event
}
#[test]
fn track_clipboard_native_cut_refusal_preserves_clipboard_pcm_output_and_history() {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(48_000, 4096);
    let mut h = ResidentHarness::with_frame_limit(32_768);
    h.next();
    let (mut revision, _) = h.applied(0);
    let original = applied(&mut h, &mut revision, Request::Import(path.clone()));
    let audio = original.state.project.tracks()[0].id();
    let second = applied(
        &mut h,
        &mut revision,
        Request::InsertBrowserInstrumentTrack {
            new_track_gain: None,
            project_generation: 0,
            revision: original.state.revision,
            class: hex_engine::NativeDeviceClass::Instrument,
            type_id: hex_engine::DeviceTypeId::from_static("hex.instrument.sine"),
        },
    );
    let instrument = second.state.project.tracks()[1].id();
    let copied = h.run(request(
        &second,
        TrackClipboardAction::Copy(TrackSelectionIds::new(&[audio]).unwrap()),
    ));
    std::fs::remove_file(path).unwrap();
    let output = h.audio.as_ref().unwrap().control.output_identity();
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let baseline = h.render();
    assert!(baseline.iter().flatten().any(|sample| *sample != 0.0));
    h.refuse.store(true, std::sync::atomic::Ordering::Release);
    h.service
        .submit(request(
            &copied,
            TrackClipboardAction::Cut(TrackSelectionIds::new(&[instrument]).unwrap()),
        ))
        .unwrap();
    let refused = h.next();
    assert!(
        refused
            .error
            .as_deref()
            .unwrap()
            .contains("fixture final admission refusal")
    );
    assert_eq!(refused.state.project, copied.state.project);
    assert_eq!(refused.state.revision, copied.state.revision);
    assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), output);
    h.refuse.store(false, std::sync::atomic::Ordering::Release);
    let pasted = applied(
        &mut h,
        &mut revision,
        request(&refused, TrackClipboardAction::Paste { before: None }),
    );
    assert_eq!(pasted.state.project.tracks().len(), 3);
    assert_eq!(
        pasted.state.project.tracks()[2].clips().len(),
        1,
        "refused instrument cut retained the prior audio copy"
    );
    let source = |e: &Event| {
        let (_, hex_session::AudioSourceBinding::Resident(audio)) = e
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
            .next()
            .unwrap()
        else {
            panic!("resident")
        };
        audio.clone()
    };
    assert!(source(&pasted).shares_pcm_with(&source(&original)));
    let doubled = h.render();
    for (actual, expected) in doubled.iter().zip(baseline) {
        assert_eq!(
            *actual,
            expected.map(|sample| sample * 2.0),
            "full paste renders both independent placements"
        );
    }
    let setup = applied(
        &mut h,
        &mut revision,
        request(
            &pasted,
            TrackClipboardAction::Duplicate {
                tracks: TrackSelectionIds::new(&[audio, instrument]).unwrap(),
                without_clips: true,
            },
        ),
    );
    assert_eq!(setup.state.project.tracks().len(), 5);
    assert!(setup.state.project.tracks()[2].clips().is_empty());
    let cloned = setup.state.project.tracks()[3].instrument().unwrap();
    let old = setup
        .state
        .project
        .track(instrument)
        .unwrap()
        .instrument()
        .unwrap();
    assert_ne!(cloned.id(), old.id());
    assert_eq!(cloned.state(), old.state());
    h.render(); // ResidentHarness measures total and signed-net allocations.
    let undo = applied(&mut h, &mut revision, Request::Undo);
    assert_eq!(undo.state.project.tracks(), pasted.state.project.tracks());
    let redo = applied(&mut h, &mut revision, Request::Redo);
    assert_eq!(redo.state.project, setup.state.project);
    assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), output);
    // A successful cut installs the new typed copy, with one undoable edit.
    let cut = applied(
        &mut h,
        &mut revision,
        request(
            &redo,
            TrackClipboardAction::Cut(TrackSelectionIds::new(&[instrument]).unwrap()),
        ),
    );
    let restored = applied(
        &mut h,
        &mut revision,
        request(&cut, TrackClipboardAction::Paste { before: None }),
    );
    assert!(
        restored
            .state
            .project
            .tracks()
            .last()
            .unwrap()
            .instrument()
            .is_some()
    );
    assert!(restored.state.project.track(instrument).is_none());
}
