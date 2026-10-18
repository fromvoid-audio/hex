//! Actual control worker/prepared playback only; no native audio backend.
use hex_project::{BeatTime, Command, Created};
use hex_session::service::{Event, Request, Service};
fn next(service: &Service) -> Event {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match service.try_event() {
            Ok(e) => {
                assert!(e.error.is_none(), "{:?}", e.error);
                return e;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                std::thread::yield_now()
            }
            Err(e) => panic!("worker event: {e}"),
        }
    }
}
fn edit(s: &Service, c: Command) -> Event {
    s.submit(Request::Edit(c)).unwrap();
    next(s)
}
fn render(e: &mut Event) -> Vec<[f32; 2]> {
    let p = e.playback.as_mut().expect("accepted canonical playback");
    p.apply(hex_engine::playback::PlaybackCommand::Seek(0));
    p.apply(hex_engine::playback::PlaybackCommand::Play);
    let mut out = vec![[0.0; 2]; 512];
    p.render(&mut out).unwrap();
    out
}
#[test]
fn note_bearing_empty_track_is_silent_then_explicit_instrument_plays_with_undo_redo() {
    let service = Service::new(48_000, 16, 8_000_000).unwrap();
    let initial = next(&service);
    assert!(initial.created.is_none());
    let added = edit(
        &service,
        Command::AddMidiTrack {
            name: "empty keys".into(),
        },
    );
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    assert!(
        added
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .is_none()
    );
    let placed = edit(
        &service,
        Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        },
    );
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("clip")
    };
    let mut notes = edit(
        &service,
        Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 69,
            channel: 0,
            velocity: 100,
        },
    );
    assert!(
        notes
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .is_none()
    );
    assert!(render(&mut notes).iter().flatten().all(|s| *s == 0.0));
    let mut inserted = edit(
        &service,
        Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        },
    );
    let instrument = inserted
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let sounded = render(&mut inserted);
    assert!(sounded.iter().flatten().any(|s| s.abs() > 0.001));
    service.submit(Request::Undo).unwrap();
    let mut undone = next(&service);
    assert!(
        undone
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .is_none()
    );
    assert!(render(&mut undone).iter().flatten().all(|s| *s == 0.0));
    service.submit(Request::Redo).unwrap();
    let mut redone = next(&service);
    assert_eq!(
        redone
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        instrument
    );
    assert_eq!(render(&mut redone), sounded);
}
