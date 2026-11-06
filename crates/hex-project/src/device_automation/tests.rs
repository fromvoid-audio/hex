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
        read_selection: crate::AutomationReadSelection::Authored,
        target: hex_lattice_contract::DeviceAutomationTarget::Parameter(ParameterId::from_static(
            1,
        )),
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
    assert!(Project::from_legacy_snapshot(snapshot.clone()).is_err());
    let injected = e
        .project()
        .device(instrument)
        .unwrap()
        .automation()
        .to_vec();
    snapshot.tracks[0].instrument.as_mut().unwrap().automation = injected;
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
            read_selection: crate::AutomationReadSelection::Authored,
            target: hex_lattice_contract::DeviceAutomationTarget::Parameter(
                ParameterId::new(id).unwrap(),
            ),
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
            read_selection: crate::AutomationReadSelection::Authored,
            target: hex_lattice_contract::DeviceAutomationTarget::Parameter(
                ParameterId::new(id).unwrap(),
            ),
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
        read_selection: crate::AutomationReadSelection::Authored,
        target: hex_lattice_contract::DeviceAutomationTarget::Parameter(
            ParameterId::new(1).unwrap(),
        ),
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

#[test]
fn shared_source_snapshots_keep_only_one_canonical_envelope_copy() {
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
    let source = e.project().track(track).unwrap().source_id().unwrap();
    for name in ["shared one", "shared two"] {
        let Created::Track(shared) = e.execute(Command::AddTrack { name: name.into() }).unwrap()
        else {
            panic!("track")
        };
        e.execute(Command::BindTrackSource {
            track: shared,
            source: Some(source),
        })
        .unwrap();
    }
    e.execute(Command::ReplaceDeviceAutomation {
        device: instrument,
        parameter: ParameterId::from_static(7),
        points: points(),
    })
    .unwrap();
    let snapshot = e.project().snapshot();
    assert_eq!(
        snapshot
            .tracks
            .iter()
            .filter(|track| track.instrument.is_some())
            .count(),
        3
    );
    assert!(
        snapshot
            .tracks
            .iter()
            .filter_map(|track| track.instrument.as_ref())
            .all(|instrument| instrument.automation.is_empty())
    );
    assert_eq!(
        snapshot.native_sources[0].devices[0].automation[0].points,
        points()
    );
    assert_eq!(
        Project::from_snapshot(snapshot.clone()).unwrap().snapshot(),
        snapshot
    );
    let mut injected = snapshot;
    injected.tracks[1].instrument.as_mut().unwrap().automation =
        injected.native_sources[0].devices[0].automation.clone();
    assert!(
        Project::from_snapshot(injected).is_err(),
        "even an identical envelope is not a valid binding projection"
    );
}

#[test]
fn typed_host_targets_share_history_bounds_and_keep_identity_across_provider_replacement() {
    let (mut e, _, device) = fixture();
    let initial = e.project().clone();
    for target in [
        DeviceAutomationTarget::HostInputGain,
        DeviceAutomationTarget::HostOutputGain,
    ] {
        e.execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target,
            points: points(),
        })
        .unwrap();
    }
    let authored = e.project().clone();
    assert_eq!(authored.device(device).unwrap().automation().len(), 2);
    assert!(
        authored
            .device(device)
            .unwrap()
            .automation_lane(ParameterId::from_static(1))
            .is_none()
    );
    assert_eq!(
        Project::from_snapshot(authored.snapshot()).unwrap(),
        authored
    );
    let replacement =
        NativeDeviceState::new(DeviceTypeId::from_static("test.other-provider"), 1, vec![3])
            .unwrap();
    e.execute(Command::ReplaceDeviceState {
        device,
        state: replacement,
    })
    .unwrap();
    assert_eq!(
        e.project().device(device).unwrap().automation(),
        authored.device(device).unwrap().automation()
    );
    assert!(e.undo());
    assert_eq!(*e.project(), authored);
    assert!(e.undo());
    assert!(e.undo());
    assert_eq!(*e.project(), initial);
    assert!(e.redo());
    assert!(e.redo());
    assert_eq!(*e.project(), authored);
    e.execute(Command::ReplaceDeviceAutomationTarget {
        device,
        target: DeviceAutomationTarget::HostInputGain,
        points: Vec::new(),
    })
    .unwrap();
    assert!(
        e.project()
            .device(device)
            .unwrap()
            .automation_target(DeviceAutomationTarget::HostInputGain)
            .is_none()
    );
}

#[test]
fn explicit_manual_targets_override_only_touched_lanes_and_reenable_is_one_history_edge() {
    let (mut e, track, device) = fixture();
    let first = DeviceAutomationTarget::Parameter(ParameterId::from_static(1));
    let second = DeviceAutomationTarget::Parameter(ParameterId::from_static(2));
    let targets = [
        first,
        second,
        DeviceAutomationTarget::HostInputGain,
        DeviceAutomationTarget::HostOutputGain,
    ];
    for target in targets {
        e.execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target,
            points: points(),
        })
        .unwrap();
    }
    let changed = NativeDeviceState::new(state().type_id(), 1, vec![3, 4]).unwrap();
    e.execute(Command::ReplaceDeviceState {
        device,
        state: changed.clone(),
    })
    .unwrap();
    assert!(
        !e.project().has_overridden_device_automation(),
        "ordinary state replacement is not a manual parameter touch"
    );
    let reading = e.project().clone();
    // Explicit touch of an unchanged base must still select Base for its lanes.
    e.execute(Command::ManualDeviceEdit {
        device,
        state: changed.clone(),
        targets: vec![first, DeviceAutomationTarget::HostInputGain],
    })
    .unwrap();
    let overridden = e.project().clone();
    assert_eq!(overridden.device(device).unwrap().state(), &changed);
    assert!(overridden.has_overridden_device_automation());
    for target in targets {
        let lane = overridden
            .device(device)
            .unwrap()
            .automation_target(target)
            .unwrap();
        assert_eq!(lane.points, points());
        assert_eq!(
            lane.read_selection,
            if [first, DeviceAutomationTarget::HostInputGain].contains(&target) {
                AutomationReadSelection::Base
            } else {
                AutomationReadSelection::Authored
            }
        );
    }
    let reading_targets: Vec<_> = overridden
        .device(device)
        .unwrap()
        .reading_automation()
        .map(|l| l.target)
        .collect();
    assert_eq!(
        reading_targets,
        [second, DeviceAutomationTarget::HostOutputGain]
    );
    assert!(e.undo());
    assert_eq!(e.project(), &reading);
    assert!(e.redo());
    assert_eq!(e.project(), &overridden);
    e.execute(Command::SetDeviceAutomationReadSelection {
        device,
        target: first,
        selection: AutomationReadSelection::Authored,
    })
    .unwrap();
    let partial = e.project().clone();
    assert_eq!(
        partial
            .device(device)
            .unwrap()
            .automation_target(first)
            .unwrap()
            .read_selection,
        AutomationReadSelection::Authored
    );
    assert_eq!(
        partial
            .device(device)
            .unwrap()
            .automation_target(DeviceAutomationTarget::HostInputGain)
            .unwrap()
            .read_selection,
        AutomationReadSelection::Base
    );
    assert!(e.undo());
    assert_eq!(e.project(), &overridden);
    assert!(e.redo());
    assert_eq!(e.project(), &partial);
    let Created::Device(other) = e
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: state(),
        })
        .unwrap()
    else {
        panic!("other device")
    };
    e.execute(Command::ReplaceDeviceAutomationTarget {
        device: other,
        target: first,
        points: points(),
    })
    .unwrap();
    e.execute(Command::ManualDeviceEdit {
        device: other,
        state: state(),
        targets: vec![first],
    })
    .unwrap();
    let before_global = e.project().clone();
    e.execute(Command::ReenableDeviceAutomation).unwrap();
    let resumed = e.project().clone();
    assert!(!resumed.has_overridden_device_automation());
    assert!(
        resumed
            .automation_devices()
            .flat_map(Device::automation)
            .all(|l| l.points == points())
    );
    assert!(e.undo());
    assert_eq!(e.project(), &before_global);
    assert!(e.redo());
    assert_eq!(e.project(), &resumed);
    // Re-enabling an already reading project is a no-op and preserves redo.
    e.execute(Command::RenameTrack {
        track,
        name: "temporary".into(),
    })
    .unwrap();
    assert!(e.undo());
    assert!(e.can_redo());
    e.execute(Command::ReenableDeviceAutomation).unwrap();
    assert!(e.can_redo());
    assert_eq!(e.project(), &resumed);
}

#[test]
fn invalid_manual_target_sets_and_missing_lane_preserve_state_selection_and_redo_atomically() {
    let (mut e, track, device) = fixture();
    let target = DeviceAutomationTarget::Parameter(ParameterId::from_static(1));
    e.execute(Command::ReplaceDeviceAutomationTarget {
        device,
        target,
        points: points(),
    })
    .unwrap();
    e.execute(Command::RenameTrack {
        track,
        name: "temporary".into(),
    })
    .unwrap();
    assert!(e.undo());
    let before = e.project().clone();
    let changed = NativeDeviceState::new(state().type_id(), 1, vec![9, 9]).unwrap();
    for targets in [
        vec![],
        vec![target, target],
        vec![DeviceAutomationTarget::HostOutputGain, target],
        vec![DeviceAutomationTarget::Parameter(ParameterId::from_static(
            0,
        ))],
    ] {
        assert!(
            e.execute(Command::ManualDeviceEdit {
                device,
                state: changed.clone(),
                targets
            })
            .is_err()
        );
        assert_eq!(e.project(), &before);
        assert!(e.can_redo());
    }
    assert!(
        e.execute(Command::SetDeviceAutomationReadSelection {
            device,
            target: DeviceAutomationTarget::HostInputGain,
            selection: AutomationReadSelection::Base
        })
        .is_err()
    );
    assert_eq!(e.project(), &before);
    assert!(e.can_redo());
    let foreign =
        NativeDeviceState::new(DeviceTypeId::from_static("test.other-provider"), 1, vec![1])
            .unwrap();
    assert!(
        e.execute(Command::ManualDeviceEdit {
            device,
            state: foreign,
            targets: vec![target]
        })
        .is_err()
    );
    assert_eq!(e.project(), &before);
    assert!(e.can_redo());
}

#[test]
fn base_selection_retains_authored_evaluation_and_still_enforces_point_bounds() {
    let (mut e, _, device) = fixture();
    let target = DeviceAutomationTarget::Parameter(ParameterId::from_static(1));
    let retained: Vec<_> = (0..MAX_DEVICE_AUTOMATION_POINTS_PER_LANE)
        .map(|i| AutomationPoint {
            beat: BeatTime::from_raw(i as u64),
            value: AutomationValue::new(0.5).unwrap(),
            outgoing: AutomationInterpolation::Step,
        })
        .collect();
    e.execute(Command::ReplaceDeviceAutomationTarget {
        device,
        target,
        points: retained.clone(),
    })
    .unwrap();
    e.execute(Command::ManualDeviceEdit {
        device,
        state: state(),
        targets: vec![target],
    })
    .unwrap();
    let before = e.project().clone();
    let lane = before
        .device(device)
        .unwrap()
        .automation_target(target)
        .unwrap();
    assert_eq!(lane.read_selection, AutomationReadSelection::Base);
    assert_eq!(lane.points, retained);
    assert_eq!(
        lane.value_at(BeatTime::ZERO).unwrap().get(),
        0.5,
        "authored curve query is independent of reading selection"
    );
    assert_eq!(
        before.device(device).unwrap().reading_automation().count(),
        0
    );
    let mut oversized = retained;
    oversized.push(AutomationPoint {
        beat: BeatTime::from_raw(MAX_DEVICE_AUTOMATION_POINTS_PER_LANE as u64),
        value: AutomationValue::new(0.5).unwrap(),
        outgoing: AutomationInterpolation::Step,
    });
    assert!(
        e.execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target,
            points: oversized
        })
        .is_err()
    );
    assert_eq!(e.project(), &before);
}

#[test]
fn vst3_automation_uses_raw_ids_and_refuses_replacement_by_a_different_class() {
    let plugin = |class, state| {
        hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new(class).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [4; 32],
                    info_sha256: [5; 32],
                },
            },
            component: Some(state),
            controller: None,
        })
        .unwrap()
    };
    let class = [1; 16];
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: plugin(class, &[1]),
        })
        .unwrap()
    else {
        panic!("expected a vst3 device");
    };
    let before = editor.project().clone();
    let target = |class, parameter| DeviceAutomationTarget::Vst3 { class, parameter };
    for invalid in [
        target([0; 16], 0),
        target([2; 16], 0),
        target(class, u32::MAX),
    ] {
        assert!(
            editor
                .execute(Command::ReplaceDeviceAutomationTarget {
                    device,
                    target: invalid,
                    points: points(),
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
    }
    let zero = target(class, 0);
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: zero,
            points: points(),
        })
        .unwrap();
    let authored = editor.project().clone();
    assert_eq!(
        authored
            .device(device)
            .unwrap()
            .automation_target(zero)
            .unwrap()
            .points,
        points()
    );
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(editor.project(), &authored);
    assert!(
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: plugin([2; 16], &[2]),
            })
            .is_err()
    );
    assert_eq!(editor.project(), &authored);
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: plugin(class, &[2]),
        })
        .unwrap();
    assert!(
        editor
            .project()
            .device(device)
            .unwrap()
            .automation_target(zero)
            .is_some()
    );
}
