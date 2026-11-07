//! Mixed-track split through the real prepare-before-commit worker.
use super::service_tests::{TestDir, next_event, wav_mono};
use super::*;
use hex_project::BeatTime;

fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let event = next_event(service);
    assert!(event.error.is_none(), "{:?}", event.error);
    event
}

#[test]
fn mixed_track_split_with_subframe_carry_prepares_and_undoes_atomically() {
    let directory = TestDir::new();
    let path = directory.0.join("source.wav");
    std::fs::write(&path, wav_mono(&vec![8192; 48_000])).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next_event(&service);
    let imported = run(&service, Request::Import(path));
    let audio = imported.state.project.tracks()[0].id();
    let added = run(
        &service,
        Request::Edit(Command::AddMidiTrack {
            name: "midi".into(),
        }),
    );
    let midi = added.state.project.tracks()[1].id();
    let placed = run(
        &service,
        Request::Edit(Command::PlaceMidiClip {
            track: midi,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(2).unwrap(),
        }),
    );
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    let before = run(
        &service,
        Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_raw(BeatTime::ONE.raw() + 1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        }),
    );
    service
        .submit(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
            duration: BeatTime::from_raw(1),
            pitch: 67,
            channel: 0,
            velocity: 100,
        }))
        .unwrap();
    let refused = next_event(&service);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.contains("quantizes to zero frames"))
    );
    assert_eq!(refused.state.revision, before.state.revision);
    assert_eq!(refused.state.project, before.state.project);
    assert!(refused.playback.is_none());
    let frame = before
        .state
        .project
        .tempo_map()
        .beat_to_frame(BeatTime::ONE)
        .unwrap();
    let split = run(
        &service,
        Request::Edit(Command::SplitTracksAtFrame {
            tracks: vec![audio, midi],
            frame,
        }),
    );
    assert!(
        split.playback.is_some(),
        "split publishes prepared playback"
    );
    assert_eq!(split.state.project.track(audio).unwrap().clips().len(), 2);
    assert_eq!(
        split.state.project.track(midi).unwrap().midi_clips().len(),
        2
    );
    let after = split.state.project.snapshot();
    let undo = run(&service, Request::Undo);
    let mut expected = before.state.project.snapshot();
    expected.next_id = undo.state.project.snapshot().next_id;
    assert_eq!(undo.state.project.snapshot(), expected);
    let redo = run(&service, Request::Redo);
    assert_eq!(redo.state.project.snapshot(), after);
}
