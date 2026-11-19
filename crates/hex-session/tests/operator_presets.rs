use hex_engine::NativeDeviceClass;
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{Command, Created};
use hex_session::Session;

#[test]
fn lfo_history_save_and_above_nyquist_note_admission() {
    let mut session = Session::new(48_000, 64, 0).unwrap();
    let (Created::Track(track), graph) = session
        .command_prepared_revision(
            Command::InsertTrackWithDevice {
                name: "lfo".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            },
            None,
        )
        .unwrap()
    else {
        panic!("track")
    };
    let device = session
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let original = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let (_, mut graph) = session
        .command_prepared_revision(
            Command::ReplaceDeviceState {
                device,
                state: original.clone(),
            },
            Some(&graph),
        )
        .unwrap();
    for (id, value) in [
        (82, ParameterValue::Bool(true)),
        (83, ParameterValue::Choice(4)),
        (84, ParameterValue::Bool(true)),
        (85, ParameterValue::Float(0.01)),
        (86, ParameterValue::Choice(0)),
        (87, ParameterValue::Float(100.0)),
        (88, ParameterValue::Bool(false)),
        (89, ParameterValue::Bool(false)),
        (90, ParameterValue::Bool(false)),
        (91, ParameterValue::Bool(false)),
        (92, ParameterValue::Bool(false)),
    ] {
        let before = session.project().device(device).unwrap().state().clone();
        graph = session
            .commit_device_parameter(device, ParameterId::from_static(id), value, Some(&graph))
            .unwrap();
        let changed = session.project().device(device).unwrap().state().clone();
        assert_ne!(before, changed);
        let undone = session
            .undo_prepared_revision(Some(&graph))
            .unwrap()
            .unwrap();
        assert_eq!(session.project().device(device).unwrap().state(), &before);
        graph = session
            .redo_prepared_revision(Some(&undone))
            .unwrap()
            .unwrap();
        assert_eq!(session.project().device(device).unwrap().state(), &changed);
    }
    let bytes = hex_storage::encode(session.project()).unwrap();
    let loaded = hex_storage::decode(&bytes).unwrap();
    assert_eq!(loaded, *session.project());
    let (Created::Clip(clip), next) = session
        .command_prepared_revision(
            Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).unwrap(),
            },
            Some(&graph),
        )
        .unwrap()
    else {
        panic!("clip")
    };
    graph = next;
    // Operators the LFO could push past nyquist are silenced for this note
    // only (Pulse #3957); the note itself is admitted.
    let before = session.project().clone();
    session
        .command_prepared_revision(
            Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_quarters(1).unwrap(),
                pitch: 127,
                channel: 0,
                velocity: 127,
            },
            Some(&graph),
        )
        .expect("above-nyquist operators are silenced, not refused");
    assert_ne!(session.project(), &before);
}

#[test]
fn fixed_tuning_history_and_above_nyquist_effective_frequency() {
    let mut session = Session::new(48_000, 16, 0).expect("session");
    let (Created::Track(track), _) = session
        .command_prepared_revision(
            Command::InsertTrackWithDevice {
                name: "fixed operator".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            },
            None,
        )
        .expect("track")
    else {
        panic!("track");
    };
    let device = session
        .project()
        .track(track)
        .expect("track")
        .instrument()
        .expect("instrument")
        .id();
    let state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .expect("operator");
    let (_, mut graph) = session
        .command_prepared_revision(Command::ReplaceDeviceState { device, state }, None)
        .expect("state");
    let (Created::Clip(clip), next) = session
        .command_prepared_revision(
            Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("length"),
            },
            Some(&graph),
        )
        .expect("clip")
    else {
        panic!("clip");
    };
    graph = next;
    (_, graph) = session
        .command_prepared_revision(
            Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_quarters(2).expect("duration"),
                pitch: 60,
                channel: 0,
                velocity: 100,
            },
            Some(&graph),
        )
        .expect("note");
    graph = session
        .commit_device_parameter(
            device,
            ParameterId::from_static(63),
            ParameterValue::Float(12_000.0),
            Some(&graph),
        )
        .expect("inactive frequency");
    graph = session
        .commit_device_parameter(
            device,
            ParameterId::from_static(64),
            ParameterValue::Float(2.0),
            Some(&graph),
        )
        .expect("inactive multiplier");
    // A 24 kHz fixed tone silences that operator (Pulse #3957), not the edit.
    let before = session.project().clone();
    let admitted = session
        .commit_device_parameter(
            device,
            ParameterId::from_static(62),
            ParameterValue::Bool(true),
            Some(&graph),
        )
        .expect("above-nyquist fixed tone is admitted");
    assert_ne!(session.project().snapshot(), before.snapshot());
    graph = session
        .undo_prepared_revision(Some(&admitted))
        .expect("undo")
        .expect("one edit");
    assert_eq!(session.project().snapshot(), before.snapshot());
    graph = session
        .commit_device_parameter(
            device,
            ParameterId::from_static(63),
            ParameterValue::Float(250.0),
            Some(&graph),
        )
        .expect("valid frequency");
    let before_toggle = session
        .project()
        .device(device)
        .expect("device")
        .state()
        .clone();
    let changed = session
        .commit_device_parameter(
            device,
            ParameterId::from_static(62),
            ParameterValue::Bool(true),
            Some(&graph),
        )
        .expect("fixed on");
    let fixed = session
        .project()
        .device(device)
        .expect("device")
        .state()
        .clone();
    assert_ne!(fixed, before_toggle);
    let undone = session
        .undo_prepared_revision(Some(&changed))
        .expect("undo")
        .expect("one edit");
    assert_eq!(
        session.project().device(device).expect("device").state(),
        &before_toggle
    );
    session
        .redo_prepared_revision(Some(&undone))
        .expect("redo")
        .expect("one edit");
    assert_eq!(
        session.project().device(device).expect("device").state(),
        &fixed
    );
}

#[test]
fn operator_preset_is_one_canonical_edit_with_exact_undo_redo() {
    let mut session = Session::new(48_000, 16, 0).expect("session");
    let (Created::Track(track), _) = session
        .command_prepared_revision(
            Command::InsertTrackWithDevice {
                name: "operator".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            },
            None,
        )
        .expect("track")
    else {
        panic!("expected track")
    };
    let device = session
        .project()
        .track(track)
        .expect("track")
        .instrument()
        .expect("instrument")
        .id();
    let original = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .expect("operator defaults");
    let original = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &original,
        ParameterId::from_static(75),
        ParameterValue::Float(0.75),
    )
    .expect("nonneutral velocity setting");
    let original = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &original,
        ParameterId::from_static(78),
        ParameterValue::Float(0.65),
    )
    .expect("authored feedback setting");
    let original = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &original,
        ParameterId::from_static(513),
        ParameterValue::Bool(false),
    )
    .expect("authored disabled oscillator");
    let (_, prepared) = session
        .command_prepared_revision(
            Command::ReplaceDeviceState {
                device,
                state: original.clone(),
            },
            None,
        )
        .expect("operator");
    let preset = ParameterId::from_static(61);
    let changed = session
        .commit_device_parameter(device, preset, ParameterValue::Choice(3), Some(&prepared))
        .expect("two pairs preset");
    let selected = session
        .project()
        .device(device)
        .expect("device")
        .state()
        .clone();
    assert_ne!(selected, original);
    assert_eq!(selected.state_version(), 17);
    assert_eq!(selected.payload().len(), 1760);
    for (id, enabled) in [(513, false), (514, true), (515, true), (516, true)] {
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                &selected,
                ParameterId::from_static(id),
            )
            .expect("preserved oscillator enable"),
            ParameterValue::Bool(enabled)
        );
    }
    assert_eq!(&selected.payload()[288..], &original.payload()[288..]);
    let undone = session
        .undo_prepared_revision(Some(&changed))
        .expect("undo")
        .expect("one edit");
    assert_eq!(
        session.project().device(device).expect("device").state(),
        &original
    );
    session
        .redo_prepared_revision(Some(&undone))
        .expect("redo")
        .expect("one edit");
    assert_eq!(
        session.project().device(device).expect("device").state(),
        &selected
    );
}

#[test]
fn legacy_seventeenth_overlapping_feedback_voice_refuses_canonical_edit_and_history() {
    let mut session = Session::new(48_000, 16, 0).expect("session");
    let (Created::Track(track), mut graph) = session
        .command_prepared_revision(
            Command::InsertTrackWithDevice {
                name: "operator capacity".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            },
            None,
        )
        .expect("track")
    else {
        panic!("expected track")
    };
    let device = session
        .project()
        .track(track)
        .expect("track")
        .instrument()
        .expect("instrument")
        .id();
    let mut state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .expect("Operator defaults");
    state = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &state,
        ParameterId::from_static(78),
        ParameterValue::Float(0.4),
    )
    .expect("feedback amount");
    // A is unmodulated in the legacy parallel fixture, enabling its feedback.
    state = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &state,
        ParameterId::from_static(61),
        ParameterValue::Choice(2),
    )
    .expect("legacy parallel routing");
    // The historical runtime retains its 16-voice admission policy; new
    // operator mode uses an independent 64-occurrence/32-sounding-voice policy.
    state = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &state,
        ParameterId::from_static(608),
        ParameterValue::Choice(0),
    )
    .expect("legacy processing");
    (_, graph) = session
        .command_prepared_revision(Command::ReplaceDeviceState { device, state }, Some(&graph))
        .expect("mutable feedback runtime admitted");
    let (Created::Clip(clip), next) = session
        .command_prepared_revision(
            Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("clip length"),
            },
            Some(&graph),
        )
        .expect("clip")
    else {
        panic!("expected clip")
    };
    graph = next;
    for index in 0..16 {
        (_, graph) = session
            .command_prepared_revision(
                Command::AddMidiNote {
                    clip,
                    start: hex_project::BeatTime::ZERO,
                    duration: hex_project::BeatTime::from_quarters(2).expect("note duration"),
                    pitch: 60 + index,
                    channel: 0,
                    velocity: 100,
                },
                Some(&graph),
            )
            .expect("voice fits declared Operator capacity");
    }

    let before_refusal = session.project().snapshot();
    assert!(
        session
            .command_prepared_revision(
                Command::AddMidiNote {
                    clip,
                    start: hex_project::BeatTime::ZERO,
                    duration: hex_project::BeatTime::from_quarters(2).expect("note duration"),
                    pitch: 76,
                    channel: 0,
                    velocity: 100,
                },
                Some(&graph),
            )
            .is_err(),
        "the seventeenth overlapping voice exceeds the declared 16-voice limit"
    );
    assert_eq!(session.project().snapshot(), before_refusal);

    let undone = session
        .undo_prepared_revision(Some(&graph))
        .expect("undo after refused edit")
        .expect("sixteenth note remains in history");
    assert_eq!(
        session
            .project()
            .track(track)
            .expect("track")
            .midi_clips()
            .iter()
            .find(|candidate| candidate.id() == clip)
            .expect("clip")
            .notes()
            .len(),
        15,
        "failed seventeenth edit did not enter history"
    );
    session
        .redo_prepared_revision(Some(&undone))
        .expect("redo after refusal")
        .expect("sixteenth note redo");
    assert_eq!(
        session
            .project()
            .track(track)
            .expect("track")
            .midi_clips()
            .iter()
            .find(|candidate| candidate.id() == clip)
            .expect("clip")
            .notes()
            .len(),
        16
    );
}

#[test]
fn pitch_keytrack_history_and_disk_reopen_preserve_independent_amounts() {
    let mut session = Session::new(48_000, 16, 0).unwrap();
    let state = hex_engine::default_native_device_state_for_class(
        NativeDeviceClass::Instrument,
        DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    // Each iteration must be a real edit: leave op4 at0 before testing100.
    let state = hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &state,
        ParameterId::from_static(616),
        ParameterValue::Float(0.0),
    )
    .unwrap();
    let (Created::Track(track), mut graph) = session
        .command_prepared_revision(
            Command::InsertTrackWithDevice {
                name: "keytrack".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state,
            },
            None,
        )
        .unwrap()
    else {
        panic!("track")
    };
    let device = session
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    for (i, amount) in [0.0, 25.0, 50.0, 100.0].into_iter().enumerate() {
        let before = session.project().device(device).unwrap().state().clone();
        graph = session
            .commit_device_parameter(
                device,
                ParameterId::from_static(613 + i as u32),
                ParameterValue::Float(amount),
                Some(&graph),
            )
            .unwrap();
        let after = session.project().device(device).unwrap().state().clone();
        let undone = session
            .undo_prepared_revision(Some(&graph))
            .unwrap()
            .unwrap();
        assert_eq!(session.project().device(device).unwrap().state(), &before);
        graph = session
            .redo_prepared_revision(Some(&undone))
            .unwrap()
            .unwrap();
        assert_eq!(session.project().device(device).unwrap().state(), &after);
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "hex-pitch-keytrack-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    struct Saved(std::path::PathBuf);
    impl Drop for Saved {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let saved = Saved(directory);
    let path = saved.0.join("keytrack.hex");
    hex_storage::save_new(&path, session.project()).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let loaded = hex_storage::load(&path).unwrap();
    assert_eq!(&loaded, session.project());
    let reopened = Session::from_prepared(loaded, 16, 0, vec![]).unwrap();
    for (i, amount) in [0.0, 25.0, 50.0, 100.0].into_iter().enumerate() {
        let state = reopened.project().device(device).unwrap().state();
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                state,
                ParameterId::from_static(613 + i as u32)
            )
            .unwrap(),
            ParameterValue::Float(amount)
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}
