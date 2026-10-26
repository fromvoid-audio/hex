//! Observable partial-print behavior, independent of the retained input representation.
use super::*;

#[test]
fn exact_in_place_range_preserves_outside_history_and_deletion_stays_silent() {
    let folder = Folder::new();
    let (mut session, track, _) = fixture_at_rate(8_000);
    session.command(Command::Gain { track, gain: 0.7 }).unwrap();
    let Created::Clip(outside) = session
        .command(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(3).unwrap(),
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("outside clip")
    };
    session
        .command(Command::AddMidiNote {
            clip: outside,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 67,
            channel: 0,
            velocity: 90,
        })
        .unwrap();
    let Created::Scene(scene) = session
        .command(Command::AddScene {
            name: "console".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(console) = session
        .command(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "console source".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("console clip")
    };
    session
        .command(Command::AddMidiNote {
            clip: console,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 72,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let utility = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let Created::Device(device) = session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: utility,
        })
        .unwrap()
    else {
        panic!("utility")
    };
    session
        .command(Command::ReplaceDeviceAutomation {
            device,
            parameter: hex_lattice_contract::ParameterId::from_static(1),
            points: vec![
                hex_project::AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: hex_project::AutomationValue::new(0.45).unwrap(),
                    outgoing: hex_project::AutomationInterpolation::Linear,
                },
                hex_project::AutomationPoint {
                    beat: BeatTime::from_quarters(2).unwrap(),
                    value: hex_project::AutomationValue::new(0.55).unwrap(),
                    outgoing: hex_project::AutomationInterpolation::Step,
                },
            ],
        })
        .unwrap();
    session
        .command(Command::ReplaceDeviceAutomationTarget {
            device,
            target: hex_lattice_contract::DeviceAutomationTarget::HostOutputGain,
            points: vec![
                hex_project::AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: hex_project::AutomationValue::new(0.45).unwrap(),
                    outgoing: hex_project::AutomationInterpolation::Linear,
                },
                hex_project::AutomationPoint {
                    beat: BeatTime::from_quarters(2).unwrap(),
                    value: hex_project::AutomationValue::new(0.55).unwrap(),
                    outgoing: hex_project::AutomationInterpolation::Step,
                },
            ],
        })
        .unwrap();
    let delay = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.delay"),
    )
    .unwrap();
    session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: delay,
        })
        .unwrap();
    let before = session.project().snapshot();
    let outside_notes = session
        .project()
        .midi_clip(outside)
        .unwrap()
        .notes()
        .to_vec();
    let revision = session.compile_prepared_revision(None).unwrap();
    let expected = pcm(Arc::clone(&revision), 24_000);
    let (start, end) = (5_003_u64, 9_007_u64);
    assert!(
        expected[4_000..start as usize]
            .iter()
            .flatten()
            .any(|x| x.abs() > 1e-5)
    );
    assert!(
        expected[end as usize..]
            .iter()
            .flatten()
            .any(|x| x.abs() > 1e-5)
    );
    session
        .bounce_arrangement(
            BounceTarget::Range(hex_project::TimeRange::new(vec![track], start, end).unwrap()),
            true,
            &folder.0,
            &hex_export::ExportControl::default(),
            Some(&revision),
        )
        .unwrap();
    let check_source = |session: &Session| {
        let after = session.project().snapshot();
        assert_eq!(after.tracks.len(), before.tracks.len());
        assert_eq!(after.tracks[0].devices, before.tracks[0].devices);
        assert_eq!(after.tracks[0].instrument, before.tracks[0].instrument);
        assert_eq!(after.tracks[0].gain, before.tracks[0].gain);
        assert_eq!(after.native_sources, before.native_sources);
        assert_eq!(after.session, before.session);
        assert_eq!(
            session.project().midi_clip(outside).unwrap().notes(),
            outside_notes
        );
    };
    check_source(&session);
    let print = &session.project().track(track).unwrap().clips()[0];
    assert_eq!((print.start(), print.duration()), (start, end - start));
    let printed = print.id();
    let actual = pcm(
        session.compile_prepared_revision(None).unwrap(),
        expected.len(),
    );
    assert_eq!(
        actual, expected,
        "inside and outside sound must remain exact"
    );
    let saved_print = folder.0.join("with-printed-range.hex");
    hex_storage::save_new(&saved_print, session.project()).unwrap();
    assert_eq!(hex_storage::load(&saved_print).unwrap(), *session.project());
    session.command(Command::RemoveClip(printed)).unwrap();
    check_source(&session);
    let mut deleted = expected.clone();
    deleted[start as usize..end as usize].fill([0.0; 2]);
    assert_eq!(
        pcm(
            session.compile_prepared_revision(None).unwrap(),
            expected.len()
        ),
        deleted
    );
    assert!(session.undo());
    assert_eq!(
        pcm(
            session.compile_prepared_revision(None).unwrap(),
            expected.len()
        ),
        expected
    );
    assert!(session.redo());
    assert_eq!(
        pcm(
            session.compile_prepared_revision(None).unwrap(),
            expected.len()
        ),
        deleted
    );
    let saved = folder.0.join("printed-range.hex");
    hex_storage::save_new(&saved, session.project()).unwrap();
    assert_eq!(hex_storage::load(&saved).unwrap(), *session.project());
}
