use hex_engine::{MidiSpanBuffer, PreparedRevision, SessionPlaybackOwner};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn applied_source_gate_query_covers_all_notes_not_the_sampled_drawing_preview() {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 64).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "activity".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip");
    };
    for pitch in 40..80 {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "activity".into(),
        })
        .unwrap()
    else {
        panic!("scene");
    };
    let Created::SessionClip(session) = editor
        .execute(Command::CopyArrangementMidiToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("session clip");
    };
    let prepared = PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        hex_engine::SharedSineTable::new_on_control(),
        None,
    )
    .unwrap();
    assert_eq!(
        prepared
            .midi_preview_on_control()
            .get(clip)
            .unwrap()
            .samples
            .len(),
        32
    );
    let mut spans = MidiSpanBuffer::default();
    for owner in [
        SessionPlaybackOwner::Arrangement,
        SessionPlaybackOwner::Session { clip: session },
    ] {
        let keys = prepared
            .midi_gate_keys(track, owner, Some(BeatTime::ZERO), (0, 0), &mut spans)
            .unwrap();
        for pitch in 0..128 {
            assert_eq!(
                keys[pitch / 64] & (1_u64 << (pitch % 64)) != 0,
                (40..80).contains(&pitch)
            );
        }
        assert_eq!(
            prepared
                .midi_gate_keys(
                    track,
                    owner,
                    Some(BeatTime::ZERO),
                    (12000, 12000),
                    &mut spans
                )
                .unwrap(),
            [0; 2],
            "release tails are not held gates"
        );
    }
    assert!(
        prepared
            .midi_gate_keys(
                track,
                SessionPlaybackOwner::Session { clip: session },
                None,
                (0, 0),
                &mut spans
            )
            .is_err()
    );
    assert!(
        prepared
            .midi_gate_keys(
                track,
                SessionPlaybackOwner::Arrangement,
                None,
                (u64::MAX, 0),
                &mut spans
            )
            .is_err()
    );
    assert_eq!(
        prepared
            .midi_gate_keys(
                track,
                SessionPlaybackOwner::Silence,
                None,
                (0, 0),
                &mut spans
            )
            .unwrap(),
        [0; 2]
    );
}

fn activity_fixture() -> (Editor, [hex_project::TrackId; 3], hex_project::DeviceId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
    let mut tracks = Vec::new();
    for _ in 0..3 {
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "activity".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track");
        };
        tracks.push(track);
    }
    let Created::Clip(arrangement) = editor
        .execute(Command::PlaceMidiClip {
            track: tracks[1],
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip");
    };
    editor
        .execute(Command::AddMidiNote {
            clip: arrangement,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 84,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let device = editor
        .project()
        .track(tracks[1])
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    (editor, [tracks[0], tracks[1], tracks[2]], device)
}
fn activity_prepare(editor: &Editor) -> Arc<PreparedRevision> {
    PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::new(),
        hex_engine::SharedSineTable::new_on_control(),
        None,
    )
    .unwrap()
    .into()
}
#[test]
fn review_followup_activity_honors_bypassed_and_missing_arrangement_instruments() {
    let (mut editor, tracks, device) = activity_fixture();
    let mut spans = MidiSpanBuffer::default();
    assert_eq!(
        activity_prepare(&editor)
            .midi_gate_keys(
                tracks[1],
                SessionPlaybackOwner::Arrangement,
                None,
                (0, 0),
                &mut spans
            )
            .unwrap(),
        [0, 1 << 20]
    );
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    assert_eq!(
        activity_prepare(&editor)
            .midi_gate_keys(
                tracks[1],
                SessionPlaybackOwner::Arrangement,
                None,
                (0, 0),
                &mut spans
            )
            .unwrap(),
        [0; 2],
        "bypassed instrument must not highlight its retained note template"
    );
    editor
        .execute(Command::RemoveInstrument { track: tracks[1] })
        .unwrap();
    assert_eq!(
        activity_prepare(&editor)
            .midi_gate_keys(
                tracks[1],
                SessionPlaybackOwner::Arrangement,
                None,
                (0, 0),
                &mut spans
            )
            .unwrap(),
        [0; 2]
    );
}
#[test]
fn review_followup_activity_uses_effective_routed_session_owner() {
    use hex_engine::{LaunchQuantization, SessionPlayback, SessionPlaybackCommand};
    let (mut editor, tracks, _) = activity_fixture();
    editor
        .execute(Command::RemoveInstrument { track: tracks[0] })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("scene");
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track: tracks[0],
            scene,
            name: "source".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("session");
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
    editor
        .execute(Command::SetMidiSource {
            destination: tracks[1],
            source: Some(tracks[0]),
        })
        .unwrap();
    editor
        .execute(Command::SetMidiSource {
            destination: tracks[2],
            source: Some(tracks[1]),
        })
        .unwrap();
    let prepared = activity_prepare(&editor);
    let mut runtime = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    runtime
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 64];
    runtime.render(&mut output).unwrap();
    assert!(output.iter().any(|frame| frame[0] != 0.0));
    let routes: Vec<_> = (0..runtime.route_count())
        .map(|i| runtime.route_status(i).unwrap())
        .collect();
    let mut spans = MidiSpanBuffer::default();
    for track in [tracks[1], tracks[2]] {
        let raw = routes.iter().find(|route| route.track == track).unwrap();
        assert_eq!(raw.owner, SessionPlaybackOwner::Arrangement);
        let effective_track = prepared.midi_activity_route_track(track).unwrap();
        let effective = routes
            .iter()
            .find(|route| route.track == effective_track)
            .unwrap();
        assert_eq!(
            prepared
                .midi_gate_keys(
                    track,
                    effective.owner,
                    effective.active_launch,
                    (runtime.arrangement_frame(), runtime.performance_frame()),
                    &mut spans
                )
                .unwrap(),
            [1 << 60, 0],
            "activity must follow the audible routed Session note, not the destination's raw Arrangement owner"
        );
    }
    assert_eq!(
        prepared
            .midi_gate_keys(
                tracks[0],
                SessionPlaybackOwner::Session { clip },
                Some(BeatTime::ZERO),
                (0, 0),
                &mut spans
            )
            .unwrap(),
        [0; 2]
    );
}
