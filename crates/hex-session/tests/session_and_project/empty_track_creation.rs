//! Actual control worker/prepared playback only; no native audio backend.
use hex_project::{BeatTime, Command, Created};
use hex_session::service::{Event, Request, Service};
fn next(service: &Service) -> Event {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match service.try_event() {
            Ok(e) => {
                assert!(e.error.is_none(), "{:?}", e.error);
                return e;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(1))
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

#[test]
fn worker_midi_clip_transfer_retains_source_devices_and_empty_destination_with_one_undo() {
    for copy in [false, true] {
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        next(&service);
        let added = edit(
            &service,
            Command::InsertTrackWithDevice {
                name: "authored source".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            },
        );
        let Some(Created::Track(source)) = added.created else {
            panic!("source")
        };
        edit(
            &service,
            Command::InsertDevice {
                track: source,
                before: None,
                state: hex_engine::default_native_device_state_for_class(
                    hex_engine::NativeDeviceClass::Effect,
                    hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            },
        );
        let placed = edit(
            &service,
            Command::PlaceMidiClip {
                track: source,
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
        let baseline = std::sync::Arc::clone(&notes.state.project);
        assert!(
            render(&mut notes)
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 0.001)
        );
        let mut transferred = edit(
            &service,
            if copy {
                Command::CopyMidiClipToNewTrack {
                    initial_gain: Some(0.25),
                    clip,
                    name: "copied clip".into(),
                    before_track: None,
                    start: BeatTime::ZERO,
                }
            } else {
                Command::MoveMidiClipToNewTrack {
                    initial_gain: Some(0.25),
                    clip,
                    name: "moved clip".into(),
                    before_track: None,
                    start: BeatTime::ZERO,
                }
            },
        );
        let Some(Created::Track(destination)) = transferred.created else {
            panic!("destination")
        };
        let project = std::sync::Arc::clone(&transferred.state.project);
        let target = project.track(destination).unwrap();
        assert_eq!(target.gain(), 0.25);
        assert!(target.instrument().is_none());
        assert!(target.devices().is_empty());
        assert_eq!(target.ordered_device_chain().count(), 0);
        let retained = project.track(source).unwrap();
        let original = baseline.track(source).unwrap();
        assert_eq!(retained.instrument(), original.instrument());
        assert_eq!(retained.devices(), original.devices());
        assert_eq!(retained.midi_clips().len(), usize::from(copy));
        let result = &target.midi_clips()[0];
        assert_eq!(result.start(), BeatTime::ZERO);
        assert_eq!(result.length(), BeatTime::ONE);
        assert_eq!(result.notes().len(), 1);
        let note = &result.notes()[0];
        assert_eq!(
            (
                note.start(),
                note.duration(),
                note.pitch(),
                note.channel(),
                note.velocity()
            ),
            (BeatTime::ZERO, BeatTime::ONE, 69, 0, 100)
        );
        if copy {
            assert_eq!(retained.midi_clips(), original.midi_clips());
            assert_ne!(result.id(), clip);
            assert!(
                render(&mut transferred)
                    .iter()
                    .flatten()
                    .any(|sample| sample.abs() > 0.001)
            );
        } else {
            assert_eq!(result.id(), clip);
            assert!(
                render(&mut transferred)
                    .iter()
                    .flatten()
                    .all(|sample| *sample == 0.0)
            );
        }
        service.submit(Request::Undo).unwrap();
        let undone = next(&service);
        assert_eq!(undone.state.project.tracks(), baseline.tracks());
        assert!(undone.state.project.track(destination).is_none());
        service.submit(Request::Redo).unwrap();
        let redone = next(&service);
        assert_eq!(redone.state.project.as_ref(), project.as_ref());
    }
}
