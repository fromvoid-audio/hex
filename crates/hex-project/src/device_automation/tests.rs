use super::*;
use crate::{Command, Editor, TrackSelectionIds};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
fn state() -> NativeDeviceState {
    NativeDeviceState::new(DeviceTypeId::new("test.automation").unwrap(), 1, vec![1, 2]).unwrap()
}
fn point(quarters: u32, value: f32, outgoing: AutomationInterpolation) -> AutomationPoint {
    AutomationPoint {
        beat: BeatTime::from_quarters(quarters.into()).unwrap(),
        value: AutomationValue::new(value).unwrap(),
        outgoing,
    }
}
fn points() -> Vec<AutomationPoint> {
    vec![
        point(1, 0.25, AutomationInterpolation::Linear),
        point(3, 0.75, AutomationInterpolation::Step),
    ]
}
fn fixture() -> (Editor, crate::TrackId, DeviceId) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "automation".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Device(device) = e
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    (e, track, device)
}
#[test]
fn normalized_values_and_envelope_evaluation_have_explicit_boundaries() {
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
        assert!(AutomationValue::new(value).is_err());
    }
    assert_eq!(
        AutomationValue::new(-0.0).unwrap(),
        AutomationValue::new(0.0).unwrap()
    );
    let mut lane = DeviceAutomation {
        parameter: ParameterId::from_static(1),
        points: points(),
    };
    for (beat, expected) in [(0, 0.25), (1, 0.25), (2, 0.5), (3, 0.75), (4, 0.75)] {
        assert_eq!(
            lane.value_at(BeatTime::from_quarters(beat).unwrap())
                .unwrap()
                .get(),
            expected
        );
    }
    lane.points[0].outgoing = AutomationInterpolation::Step;
    assert_eq!(
        lane.value_at(BeatTime::from_quarters(2).unwrap())
            .unwrap()
            .get(),
        0.25
    );
    assert_eq!(
        lane.value_at(BeatTime::from_quarters(3).unwrap())
            .unwrap()
            .get(),
        0.75
    );
    lane.points.clear();
    assert_eq!(lane.value_at(BeatTime::ZERO), None);
}
#[test]
fn edits_are_atomic_target_stable_and_clear_restores_no_override() {
    let (mut e, track, device) = fixture();
    let parameter = ParameterId::from_static(7);
    assert!(!e.project().has_device_automation());
    let original = e.project().device(device).unwrap().state().clone();
    e.execute(Command::ReplaceDeviceAutomation {
        device,
        parameter,
        points: points(),
    })
    .unwrap();
    let authored = e.project().clone();
    assert_eq!(authored.device(device).unwrap().state(), &original);
    let mut duplicate = points();
    duplicate[1].beat = duplicate[0].beat;
    assert!(
        e.execute(Command::ReplaceDeviceAutomation {
            device,
            parameter,
            points: duplicate
        })
        .is_err()
    );
    assert_eq!(e.project(), &authored);
    let Created::Track(other) = e
        .execute(Command::AddTrack {
            name: "other".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    e.execute(Command::MoveDevice {
        device,
        track: other,
        before: None,
    })
    .unwrap();
    assert!(e.project().track(track).unwrap().devices().is_empty());
    assert_eq!(
        e.project().track(other).unwrap().devices()[0].automation()[0].points,
        points()
    );
    e.execute(Command::ReplaceDeviceAutomation {
        device,
        parameter,
        points: vec![],
    })
    .unwrap();
    assert!(!e.project().has_device_automation());
    assert!(e.undo());
    assert!(e.project().has_device_automation());
    assert!(e.redo());
    assert!(!e.project().has_device_automation());
    e.execute(Command::RemoveDevice(device)).unwrap();
    assert!(
        e.execute(Command::ReplaceDeviceAutomation {
            device,
            parameter,
            points: points()
        })
        .is_err()
    );
}
#[test]
fn snapshots_and_track_duplication_preserve_envelopes_with_independent_device_identity() {
    let (mut e, track, device) = fixture();
    let parameter = ParameterId::from_static(1);
    e.execute(Command::ReplaceDeviceAutomation {
        device,
        parameter,
        points: points(),
    })
    .unwrap();
    let source = e.project().snapshot();
    assert_eq!(
        Project::from_snapshot(source.clone()).unwrap().snapshot(),
        source
    );
    e.execute(Command::DuplicateTracks {
        tracks: TrackSelectionIds::new(&[track]).unwrap(),
        without_clips: true,
    })
    .unwrap();
    let copied = &e.project().tracks()[1].devices()[0];
    assert_ne!(copied.id(), device);
    assert_eq!(copied.automation_lane(parameter).unwrap().points, points());
    let before = e.project().clone();
    let mut snapshot = before.snapshot();
    let duplicate = snapshot.tracks[0].devices[0].automation[0].clone();
    snapshot.tracks[0].devices[0].automation.push(duplicate);
    assert!(Project::from_snapshot(snapshot).is_err());
}
#[test]
fn source_main_and_return_envelopes_are_canonical_and_survive_history() {
    let (mut e, track, _) = fixture();
    let Created::Device(instrument) = e
        .execute(Command::InsertInstrument {
            track,
            state: state(),
        })
        .unwrap()
    else {
        panic!("instrument")
    };
    let Created::Device(main) = e
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(),
        })
        .unwrap()
    else {
        panic!("main")
    };
    let Created::ReturnTrack(return_track) = e
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    let Created::Device(ret) = e
        .execute(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: state(),
        })
        .unwrap()
    else {
        panic!("return device")
    };
    for device in [instrument, main, ret] {
        e.execute(Command::ReplaceDeviceAutomation {
            device,
            parameter: ParameterId::from_static(1),
            points: points(),
        })
        .unwrap();
    }
    assert_eq!(
        e.project()
            .automation_devices()
            .filter(|d| !d.automation().is_empty())
            .count(),
        3
    );
    assert_eq!(
        e.project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .automation(),
        e.project().device(instrument).unwrap().automation()
    );
    assert_eq!(
        Project::from_snapshot(e.project().snapshot()).unwrap(),
        *e.project()
    );
    let mut snapshot = e.project().snapshot();
    snapshot.tracks[0]
        .instrument
        .as_mut()
        .unwrap()
        .automation
        .clear();
    assert!(Project::from_snapshot(snapshot).is_err());
    e.execute(Command::RemoveTrack(track)).unwrap();
    assert!(
        e.project()
            .device(instrument)
            .unwrap()
            .automation_lane(ParameterId::from_static(1))
            .is_some()
    );
}

#[test]
fn provider_replacement_refuses_to_reinterpret_existing_parameter_ids() {
    let (mut e, _, device) = fixture();
    let parameter = ParameterId::from_static(7);
    e.execute(Command::ReplaceDeviceAutomation {
        device,
        parameter,
        points: points(),
    })
    .unwrap();
    let original = e.project().clone();
    let replacement =
        NativeDeviceState::new(DeviceTypeId::new("test.other").unwrap(), 1, vec![]).unwrap();
    for command in [
        Command::ReplaceDeviceState {
            device,
            state: replacement.clone(),
        },
        Command::ReplaceDeviceComparison {
            device,
            state: replacement.clone(),
            comparison: None,
        },
        Command::ReplaceDeviceStateWithIdentityReservation {
            device,
            expected_state: state(),
            expected_variant: crate::DeviceVariant::A,
            state: replacement.clone(),
            expected_next_id: original.snapshot().next_id,
            reserved_ids: 1,
        },
    ] {
        assert!(e.execute(command).is_err());
        assert_eq!(e.project(), &original);
    }
    e.execute(Command::ReplaceDeviceAutomation {
        device,
        parameter,
        points: vec![],
    })
    .unwrap();
    e.execute(Command::ReplaceDeviceState {
        device,
        state: replacement,
    })
    .unwrap();
    assert!(e.project().device(device).unwrap().automation().is_empty());
}
#[test]
fn unsupported_time_transforms_refuse_without_separating_envelopes_from_music() {
    let (mut e, track, device) = fixture();
    let Created::ArrangementSection(section) = e
        .execute(Command::AddArrangementSection {
            name: "section".into(),
            color: crate::ElementColor::Cyan,
            start: BeatTime::ZERO,
            end: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("section")
    };
    e.execute(Command::ReplaceDeviceAutomation {
        device,
        parameter: ParameterId::from_static(1),
        points: points(),
    })
    .unwrap();
    let original = e.project().clone();
    let range = crate::TimeRange::new(vec![track], 0, 48_000).unwrap();
    assert_eq!(
        e.project().capture_time_range(&range).unwrap_err().0,
        crate::AUTOMATION_EDIT_UNSUPPORTED
    );
    for command in [
        Command::DuplicateArrangementSection(section),
        Command::DeleteArrangementSection(section),
        Command::ResizeArrangementSection {
            section,
            edge: crate::TrimEdge::Right,
            length: BeatTime::from_quarters(8).unwrap(),
        },
    ] {
        assert_eq!(
            e.execute(command).unwrap_err().0,
            crate::AUTOMATION_EDIT_UNSUPPORTED
        );
        assert_eq!(e.project(), &original);
    }
}
#[test]
fn malformed_snapshot_limits_are_checked_at_the_canonical_boundary() {
    let (e, _, _) = fixture();
    let initial = e.project().snapshot();
    let one = point(0, 0.5, AutomationInterpolation::Step);
    let mut snapshot = initial.clone();
    snapshot.tracks[0].devices[0].automation = (1..=257)
        .map(|id| DeviceAutomation {
            parameter: ParameterId::new(id).unwrap(),
            points: vec![one],
        })
        .collect();
    assert!(Project::from_snapshot(snapshot).is_err());
    let dense: Vec<_> = (0..1024)
        .map(|index| AutomationPoint {
            beat: BeatTime::from_raw(index),
            ..one
        })
        .collect();
    let mut snapshot = initial.clone();
    snapshot.tracks[0].devices[0].automation = (1..=65)
        .map(|id| DeviceAutomation {
            parameter: ParameterId::new(id).unwrap(),
            points: dense.clone(),
        })
        .collect();
    assert!(
        Project::from_snapshot(snapshot).is_err(),
        "aggregate point budget applies even to valid individual lanes"
    );
    let mut too_many = dense;
    too_many.push(AutomationPoint {
        beat: BeatTime::from_raw(1024),
        ..one
    });
    let mut snapshot = initial;
    snapshot.tracks[0].devices[0].automation = vec![DeviceAutomation {
        parameter: ParameterId::new(1).unwrap(),
        points: too_many,
    }];
    assert!(Project::from_snapshot(snapshot).is_err());
}
#[test]
fn clearing_an_unassigned_parameter_preserves_redo_and_does_not_assign_a_lane() {
    let (mut e, _, device) = fixture();
    e.execute(Command::RenameTrack {
        track: e.project().tracks()[0].id(),
        name: "renamed".into(),
    })
    .unwrap();
    assert!(e.undo());
    assert!(e.can_redo());
    let original = e.project().clone();
    e.execute(Command::ReplaceDeviceAutomation {
        device,
        parameter: ParameterId::from_static(7),
        points: vec![],
    })
    .unwrap();
    assert_eq!(e.project(), &original);
    assert!(e.can_redo());
    assert!(e.project().device(device).unwrap().automation().is_empty());
}
