use hex_engine::{NativeDeviceClass, PlaybackCommand, PreparedRevision, SineTable};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use hex_session::arrangement_seek::{
    ArrangementSeekMode, ArrangementSeekSpec, ArrangementSeekWorker,
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};

#[test]
fn feedback_operator_arrangement_seek_matches_play_from_without_replay() {
    operator_preroll(None);
}

#[test]
fn lfo_free_sync_and_hold_arrangement_seek_match_play_from_without_replay() {
    for sync in [false, true] {
        operator_preroll(Some(sync));
    }
}

fn operator_preroll(lfo: Option<bool>) {
    const RATE: u32 = 48_000;
    const OPERATOR: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.operator");
    let mut editor = Editor::new(Project::new(RATE).expect("project"), 16).expect("editor");
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "operator seek".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("MIDI track")
    else {
        panic!("expected MIDI track")
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .expect("Operator slot")
        .id();
    let state =
        hex_engine::default_native_device_state_for_class(NativeDeviceClass::Instrument, OPERATOR)
            .expect("Operator default state");
    let state = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &state,
        ParameterId::from_static(78),
        ParameterValue::Float(0.4),
    )
    .expect("feedback amount");
    let mut state = state;
    if let Some(sync) = lfo {
        for (id, value) in [
            (82, ParameterValue::Bool(true)),
            (83, ParameterValue::Choice(4)),
            (84, ParameterValue::Bool(sync)),
            (85, ParameterValue::Float(100.0)),
            (87, ParameterValue::Float(100.0)),
            (88, ParameterValue::Bool(false)),
        ] {
            state = hex_engine::patch_native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                &state,
                ParameterId::from_static(id),
                value,
            )
            .unwrap();
        }
    }
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state,
        })
        .expect("enable feedback");
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).expect("clip length"),
        })
        .expect("MIDI clip")
    else {
        panic!("expected MIDI clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 4).expect("note duration"),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("MIDI note");
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .expect("prepared feedback Operator"),
    );
    assert!(revision.has_arrangement_mutable_instruments());
    assert!(revision.arrangement_seek_requires_preroll());

    let destination = 1_000;
    let worker = ArrangementSeekWorker::new().expect("seek worker");
    let request_id = worker
        .submit(ArrangementSeekSpec {
            mode: ArrangementSeekMode::Interactive,
            generation: 7,
            revision: 3,
            prepared_revision: Arc::clone(&revision),
            destination_frame: destination,
            work_budget_frames: destination,
            start_playing: false,
        })
        .expect("submit mutable Operator preroll");
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    let result = loop {
        if let Some(result) = worker.try_result() {
            break result;
        }
        assert!(Instant::now() < deadline, "Operator seek did not finish");
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert_eq!(result.request_id, request_id);
    assert_eq!(result.mode, ArrangementSeekMode::Interactive);
    assert_eq!(result.destination_frame, destination);
    assert!(Arc::ptr_eq(&result.prepared_revision, &revision));
    let mut sought = result.result.expect("mutable Operator seek candidate");
    sought.apply(PlaybackCommand::Play);

    // Seeks share the interactive Play-from law: Operator feedback/LFO
    // history starts empty and the held note is chased at the destination.
    let mut reference = hex_engine::playback::Playback::try_from_prepared_with_session(revision)
        .expect("live reference");
    reference.apply(PlaybackCommand::PlayFrom(destination));
    let mut actual = [[0.0; 2]; 256];
    let mut expected = [[0.0; 2]; 256];
    sought.render(&mut actual).expect("render seek candidate");
    reference
        .render(&mut expected)
        .expect("render play-from reference");
    assert_eq!(actual, expected);
    assert!(actual.iter().any(|frame| frame[0].abs() > 1.0e-6));
}
