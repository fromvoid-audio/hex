use super::*;
use hex_devices::gate::{GATE_DEFINITION, parameter as gate_parameter};
use hex_lattice::NativeDeviceDefinition;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{DeviceSnapshot, ReturnTrackSnapshot, SendLevelSnapshot};

fn project_with_gates(
    threshold_track: f32,
    threshold_return: f32,
    track_bypassed: bool,
    return_bypassed: bool,
) -> Project {
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    let track_device = snapshot.next_id;
    let return_track = track_device + 1;
    let return_device = track_device + 2;
    snapshot.next_id = return_device + 1;
    let state = |threshold: f32| {
        let defaults = GATE_DEFINITION.default_state().expect("gate defaults");
        let with_return = GATE_DEFINITION
            .patch_parameter(
                &defaults,
                gate_parameter::RETURN,
                ParameterValue::Float(threshold.min(-45.0)),
            )
            .expect("valid gate return");
        GATE_DEFINITION
            .patch_parameter(
                &with_return,
                gate_parameter::THRESHOLD,
                ParameterValue::Float(threshold),
            )
            .expect("gate threshold")
    };
    snapshot.tracks[0].devices.push(DeviceSnapshot {
        modulation: Default::default(),
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id: track_device,
        bypassed: track_bypassed,
        state: state(threshold_track),
    });
    snapshot.tracks[0].sends.push(SendLevelSnapshot {
        return_track,
        gain: 0.5,
    });
    snapshot.return_tracks.push(ReturnTrackSnapshot {
        id: return_track,
        name: "gate return".into(),
        gain: 0.75,
        muted: false,
        devices: vec![DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: return_device,
            bypassed: return_bypassed,
            state: state(threshold_return),
        }],
    });
    Project::from_snapshot(snapshot).expect("valid track and Return Gate project")
}

fn render(project: &Project, frame_count: usize) -> Vec<[f32; 2]> {
    let mut playback =
        SessionPlayback::new(revision(project, Arc::new(hex_devices::SineTable::new())))
            .expect("prepared playback");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut frames = vec![[0.0; 2]; frame_count];
    playback.render(&mut frames).expect("render");
    frames
}

fn runtime(
    state: &hex_lattice_contract::NativeDeviceState,
) -> Box<dyn hex_lattice::NativeProcessorRuntime> {
    GATE_DEFINITION
        .prepare_processor(
            state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("prepared Gate")
        .expect("stateful Gate")
        .create_runtime()
}

#[test]
fn gate_processes_ordered_track_and_return_routes_and_bypass_is_transparent() {
    let mut dry_snapshot = project(1.0, 0, 8 << 32).snapshot();
    let return_track = dry_snapshot.next_id;
    dry_snapshot.next_id += 1;
    dry_snapshot.tracks[0].sends.push(SendLevelSnapshot {
        return_track,
        gain: 0.5,
    });
    dry_snapshot.return_tracks.push(ReturnTrackSnapshot {
        id: return_track,
        name: "dry return".into(),
        gain: 0.75,
        muted: false,
        devices: Vec::new(),
    });
    let dry_project = Project::from_snapshot(dry_snapshot).expect("dry route");
    let dry = render(&dry_project, 512);
    let track_only = render(&project_with_gates(-20.0, 0.0, false, true), 512);
    let return_only = render(&project_with_gates(0.0, -20.0, true, false), 512);
    let combined_project = project_with_gates(-20.0, -20.0, false, false);
    let combined = render(&combined_project, 512);

    let combined_track = combined_project.tracks()[0].devices()[0].state();
    let combined_return = combined_project.return_tracks()[0].devices()[0].state();
    let mut track = runtime(combined_track);
    let mut return_ = runtime(combined_return);
    for (dry_frame, actual) in dry.iter().zip(&combined) {
        let raw = [dry_frame[0] / 1.375, dry_frame[1] / 1.375];
        let mut direct = raw;
        track.process_block(
            std::slice::from_mut(&mut direct),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        let mut send = [direct[0] * 0.5, direct[1] * 0.5];
        return_.process_block(
            std::slice::from_mut(&mut send),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        let expected = [direct[0] + send[0] * 0.75, direct[1] + send[1] * 0.75];
        assert!((actual[0] - expected[0]).abs() < 2.0e-6);
        assert!((actual[1] - expected[1]).abs() < 2.0e-6);
    }
    assert_ne!(track_only, dry);
    assert_ne!(return_only, dry);
    assert_ne!(combined, dry);
    assert_eq!(
        render(&project_with_gates(-20.0, -20.0, true, true), 512),
        dry
    );
}

#[test]
fn live_parameter_revision_keeps_track_and_return_gate_ramp_history() {
    let original = project_with_gates(-72.0, -72.0, false, false);
    let table = Arc::new(hex_devices::SineTable::new());
    let original_revision = revision(&original, Arc::clone(&table));
    let mut playback = SessionPlayback::new(Arc::clone(&original_revision)).expect("playback");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut reference = SessionPlayback::new(Arc::clone(&original_revision)).expect("reference");
    reference
        .apply(SessionPlaybackCommand::Play)
        .expect("play reference");
    let mut prefix = [[0.0; 2]; 24];
    let mut same_prefix = prefix;
    playback.render(&mut prefix).expect("warmup");
    reference
        .render(&mut same_prefix)
        .expect("reference warmup");
    assert_eq!(prefix, same_prefix);

    let previous_states = [
        original.tracks()[0].devices()[0].state().clone(),
        original.return_tracks()[0].devices()[0].state().clone(),
    ];
    let track_device = original.tracks()[0].devices()[0].id();
    let return_device = original.return_tracks()[0].devices()[0].id();
    let mut editor = Editor::new(original, 8).expect("gate editor");
    for (device, old_state) in [
        (track_device, previous_states[0].clone()),
        (return_device, previous_states[1].clone()),
    ] {
        let state = GATE_DEFINITION
            .patch_parameter(
                &old_state,
                gate_parameter::THRESHOLD,
                ParameterValue::Float(-71.0),
            )
            .expect("edited threshold");
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .expect("replace Gate state");
    }
    let snapshot = editor.project().snapshot();
    let edited = Project::from_snapshot(snapshot.clone()).expect("edited project roundtrip");
    assert_eq!(edited.snapshot(), snapshot);
    let changed_revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(original_revision.session()),
        )
        .expect("prepare edited revision"),
    );
    let mut changed = SessionPlayback::new(changed_revision).expect("edited playback");
    changed
        .try_inherit_state_from(&mut playback)
        .expect("same-rate Gate state transfers");

    let new_states = [
        editor.project().tracks()[0].devices()[0].state().clone(),
        editor.project().return_tracks()[0].devices()[0]
            .state()
            .clone(),
    ];
    let mut after = [[0.0; 2]; 64];
    changed.render(&mut after).expect("edited frames");
    let dry_project = project(1.0, 0, 8 << 32);
    let mut dry_playback =
        SessionPlayback::new(revision(&dry_project, Arc::clone(&table))).unwrap();
    dry_playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut dry_all = [[0.0; 2]; 88];
    dry_playback.render(&mut dry_all).unwrap();
    let mut expected_track = runtime(&new_states[0]);
    let mut expected_return = runtime(&new_states[1]);
    let mut prior_track = runtime(&previous_states[0]);
    let mut prior_return = runtime(&previous_states[1]);
    for frame in &mut dry_all[..24] {
        prior_track.process_block(
            std::slice::from_mut(frame),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        let mut sent = [frame[0] * 0.5, frame[1] * 0.5];
        prior_return.process_block(
            std::slice::from_mut(&mut sent),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
    }
    expected_track.copy_history_from(prior_track.as_ref());
    expected_return.copy_history_from(prior_return.as_ref());
    let expected_after = dry_all[24..]
        .iter()
        .copied()
        .map(|mut frame| {
            expected_track.process_block(
                std::slice::from_mut(&mut frame),
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
            let mut sent = [frame[0] * 0.5, frame[1] * 0.5];
            expected_return.process_block(
                std::slice::from_mut(&mut sent),
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
            [frame[0] + sent[0] * 0.75, frame[1] + sent[1] * 0.75]
        })
        .collect::<Vec<_>>();
    for (actual, expected) in after.iter().flatten().zip(expected_after.iter().flatten()) {
        assert!((actual - expected).abs() < 2.0e-6);
    }

    let mut fresh_track = runtime(&new_states[0]);
    let mut fresh_return = runtime(&new_states[1]);
    let mut fresh_first = dry_all[24];
    fresh_track.process_block(
        std::slice::from_mut(&mut fresh_first),
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    let mut fresh_send = [fresh_first[0] * 0.5, fresh_first[1] * 0.5];
    fresh_return.process_block(
        std::slice::from_mut(&mut fresh_send),
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    let fresh_first = [
        fresh_first[0] + fresh_send[0] * 0.75,
        fresh_first[1] + fresh_send[1] * 0.75,
    ];
    assert_ne!(expected_after[0], fresh_first);
    assert!(editor.undo());
    assert!(editor.undo());
    assert_eq!(
        editor.project().tracks()[0].devices()[0].state(),
        &previous_states[0]
    );
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(
        editor.project().return_tracks()[0].devices()[0].state(),
        &new_states[1]
    );
}
