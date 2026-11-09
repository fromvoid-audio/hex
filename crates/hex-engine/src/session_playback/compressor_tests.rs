use super::*;
use hex_devices::compressor::{COMPRESSOR_DEFINITION, parameter as compressor_parameter};
use hex_lattice::NativeDeviceDefinition;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{DeviceSnapshot, ReturnTrackSnapshot, SendLevelSnapshot};

fn project_with_compressors(
    track_threshold: f32,
    return_threshold: f32,
    track_bypassed: bool,
    return_bypassed: bool,
) -> Project {
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    let track_device = snapshot.next_id;
    let return_track = track_device + 1;
    let return_device = track_device + 2;
    snapshot.next_id = return_device + 1;
    let state_with_threshold = |threshold| {
        COMPRESSOR_DEFINITION
            .patch_parameter(
                &COMPRESSOR_DEFINITION
                    .default_state()
                    .expect("compressor defaults"),
                compressor_parameter::THRESHOLD,
                ParameterValue::Float(threshold),
            )
            .expect("threshold patch")
    };
    snapshot.tracks[0].devices.push(DeviceSnapshot {
        modulation: Default::default(),
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id: track_device,
        bypassed: track_bypassed,
        state: state_with_threshold(track_threshold),
    });
    snapshot.tracks[0].sends.push(SendLevelSnapshot {
        return_track,
        gain: 0.5,
    });
    snapshot.return_tracks.push(ReturnTrackSnapshot {
        id: return_track,
        name: "compressor return".into(),
        gain: 0.75,
        muted: false,
        devices: vec![DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: return_device,
            bypassed: return_bypassed,
            state: state_with_threshold(return_threshold),
        }],
    });
    Project::from_snapshot(snapshot).expect("valid compressor track and Return project")
}

fn render_project(project: &Project) -> Vec<[f32; 2]> {
    render_project_frames(project, 512)
}

fn render_project_frames(project: &Project, frame_count: usize) -> Vec<[f32; 2]> {
    let mut playback =
        SessionPlayback::new(revision(project, Arc::new(hex_devices::SineTable::new())))
            .expect("prepared playback");
    playback
        .apply(SessionPlaybackCommand::Play)
        .expect("play command");
    let mut frames = vec![[0.0; 2]; frame_count];
    playback.render(&mut frames).expect("render");
    frames
}

fn runtime(
    state: &hex_lattice_contract::NativeDeviceState,
) -> Box<dyn hex_lattice::NativeProcessorRuntime> {
    COMPRESSOR_DEFINITION
        .prepare_processor(
            state,
            &hex_lattice::DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .expect("prepared compressor")
        .expect("stateful compressor")
        .create_runtime()
}

#[test]
fn compressor_renders_on_track_and_return_in_send_order_and_bypass_is_transparent() {
    let mut route_snapshot = project(1.0, 0, 8 << 32).snapshot();
    let track_device = route_snapshot.next_id;
    let return_track = track_device + 1;
    let return_device = track_device + 2;
    route_snapshot.next_id = return_device + 1;
    route_snapshot.tracks[0].sends.push(SendLevelSnapshot {
        return_track,
        gain: 0.5,
    });
    route_snapshot.return_tracks.push(ReturnTrackSnapshot {
        id: return_track,
        name: "compressor return".into(),
        gain: 0.75,
        muted: false,
        devices: Vec::new(),
    });
    let baseline_project = Project::from_snapshot(route_snapshot).expect("uncompressed route");
    let baseline = render_project(&baseline_project);

    let track_only = render_project(&project_with_compressors(-40.0, 0.0, false, true));
    let return_only = render_project(&project_with_compressors(0.0, -40.0, true, false));
    let combined_project = project_with_compressors(-40.0, -30.0, false, false);
    let combined = render_project(&combined_project);
    assert_ne!(track_only, baseline);
    assert_ne!(return_only, baseline);
    assert_ne!(combined, baseline);

    let track_state = combined_project.tracks()[0].devices()[0].state();
    let return_state = combined_project.return_tracks()[0].devices()[0].state();
    let mut track_processor = runtime(track_state);
    let mut return_processor = runtime(return_state);
    let mut oracle = baseline.clone();
    let dry_route_gain = 1.0 + 0.5 * 0.75;
    for frame in &mut oracle {
        frame[0] /= dry_route_gain;
        frame[1] /= dry_route_gain;
        track_processor.process_block(
            std::slice::from_mut(frame),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        let mut send = [frame[0] * 0.5, frame[1] * 0.5];
        return_processor.process_block(
            std::slice::from_mut(&mut send),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        frame[0] += send[0] * 0.75;
        frame[1] += send[1] * 0.75;
    }
    for (actual, expected) in combined.iter().flatten().zip(oracle.iter().flatten()) {
        assert!((actual - expected).abs() < 2.0e-6);
    }

    let bypassed = render_project(&project_with_compressors(-40.0, -30.0, true, true));
    assert_eq!(bypassed, baseline);
}

#[test]
fn compressor_parameter_revisions_keep_detector_and_gain_history_live() {
    let original = project_with_compressors(-18.0, -18.0, false, false);
    let table = Arc::new(hex_devices::SineTable::new());
    let original_revision = revision(&original, Arc::clone(&table));
    let mut previous = SessionPlayback::new(Arc::clone(&original_revision)).expect("previous");
    let mut reference = SessionPlayback::new(Arc::clone(&original_revision)).expect("reference");
    previous
        .apply(SessionPlaybackCommand::Play)
        .expect("previous play");
    reference
        .apply(SessionPlaybackCommand::Play)
        .expect("reference play");
    let mut warmup = [[0.0; 2]; 512];
    let mut reference_warmup = warmup;
    previous.render(&mut warmup).expect("warmup");
    reference
        .render(&mut reference_warmup)
        .expect("reference warmup");
    assert_eq!(warmup, reference_warmup);

    let mut dry_snapshot = original.snapshot();
    dry_snapshot.tracks[0].devices.clear();
    dry_snapshot.return_tracks[0].devices.clear();
    let dry_project = Project::from_snapshot(dry_snapshot).expect("dry routed source");
    let dry_route = render_project_frames(&dry_project, 640);
    let mut old_track_runtime = runtime(original.tracks()[0].devices()[0].state());
    let mut old_return_runtime = runtime(original.return_tracks()[0].devices()[0].state());
    let recover_track_source = |frame: [f32; 2]| [frame[0] / 1.375, frame[1] / 1.375];
    for frame in &dry_route[..512] {
        let mut track = recover_track_source(*frame);
        old_track_runtime.process_block(
            std::slice::from_mut(&mut track),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
        let mut sent = [track[0] * 0.5, track[1] * 0.5];
        old_return_runtime.process_block(
            std::slice::from_mut(&mut sent),
            &hex_lattice::EMPTY_DEVICE_RESOURCES,
        );
    }

    let track_device = original.tracks()[0].devices()[0].id();
    let return_device = original.return_tracks()[0].devices()[0].id();
    let mut editor = Editor::new(original, 8).expect("editor");
    let old_states = [
        (
            track_device,
            editor.project().tracks()[0].devices()[0].state().clone(),
        ),
        (
            return_device,
            editor.project().return_tracks()[0].devices()[0]
                .state()
                .clone(),
        ),
    ];
    for (device, old_state) in old_states {
        let state = COMPRESSOR_DEFINITION
            .patch_parameter(
                &old_state,
                compressor_parameter::THRESHOLD,
                ParameterValue::Float(-42.0),
            )
            .expect("edited threshold");
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .expect("replace processor state");
    }
    let snapshot = editor.project().snapshot();
    assert_eq!(
        Project::from_snapshot(snapshot.clone()).unwrap().snapshot(),
        snapshot
    );
    let changed_revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(original_revision.session()),
        )
        .expect("prepare changed revision"),
    );
    let mut changed = SessionPlayback::new(changed_revision).expect("changed playback");
    changed
        .try_inherit_state_from(&mut previous)
        .expect("compatible compressor histories retarget");

    let mut expected_track = runtime(editor.project().tracks()[0].devices()[0].state());
    let mut expected_return = runtime(editor.project().return_tracks()[0].devices()[0].state());
    assert!(expected_track.can_copy_history_from(old_track_runtime.as_ref()));
    assert!(expected_return.can_copy_history_from(old_return_runtime.as_ref()));
    expected_track.copy_history_from(old_track_runtime.as_ref());
    expected_return.copy_history_from(old_return_runtime.as_ref());
    let expected_after = dry_route[512..640]
        .iter()
        .copied()
        .map(|frame| {
            let mut track = recover_track_source(frame);
            expected_track.process_block(
                std::slice::from_mut(&mut track),
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
            let mut sent = [track[0] * 0.5, track[1] * 0.5];
            expected_return.process_block(
                std::slice::from_mut(&mut sent),
                &hex_lattice::EMPTY_DEVICE_RESOURCES,
            );
            [track[0] + sent[0] * 0.75, track[1] + sent[1] * 0.75]
        })
        .collect::<Vec<_>>();
    let mut fresh_track = runtime(editor.project().tracks()[0].devices()[0].state());
    let mut fresh_return = runtime(editor.project().return_tracks()[0].devices()[0].state());
    let first_dry = recover_track_source(dry_route[512]);
    let mut fresh_track_frame = first_dry;
    fresh_track.process_block(
        std::slice::from_mut(&mut fresh_track_frame),
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    let mut fresh_sent = [fresh_track_frame[0] * 0.5, fresh_track_frame[1] * 0.5];
    fresh_return.process_block(
        std::slice::from_mut(&mut fresh_sent),
        &hex_lattice::EMPTY_DEVICE_RESOURCES,
    );
    let fresh_first = [
        fresh_track_frame[0] + fresh_sent[0] * 0.75,
        fresh_track_frame[1] + fresh_sent[1] * 0.75,
    ];

    let mut before_edit = [[0.0; 2]; 128];
    let mut after_edit = [[0.0; 2]; 128];
    reference
        .render(&mut before_edit)
        .expect("old settings continue");
    changed
        .render(&mut after_edit)
        .expect("new settings continue");
    for (actual, expected) in after_edit
        .iter()
        .flatten()
        .zip(expected_after.iter().flatten())
    {
        assert!((actual - expected).abs() < 2.0e-6);
    }
    assert_ne!(expected_after[0], fresh_first);
    assert_ne!(before_edit, after_edit);

    assert!(editor.undo());
    assert!(editor.undo());
    for state in [
        editor.project().tracks()[0].devices()[0].state(),
        editor.project().return_tracks()[0].devices()[0].state(),
    ] {
        assert_eq!(
            COMPRESSOR_DEFINITION
                .read_parameter(state, compressor_parameter::THRESHOLD)
                .expect("undo state"),
            ParameterValue::Float(-18.0)
        );
    }
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(
        COMPRESSOR_DEFINITION
            .read_parameter(
                editor.project().tracks()[0].devices()[0].state(),
                compressor_parameter::THRESHOLD,
            )
            .expect("redo state"),
        ParameterValue::Float(-42.0)
    );
}
