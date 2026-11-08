use super::*;
use hex_devices::saturator::{SATURATOR_DEFINITION, parameter as saturator_parameter};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{DeviceSnapshot, ReturnTrackSnapshot, SendLevelSnapshot};

fn saturator_state(drive: f32) -> hex_lattice_contract::NativeDeviceState {
    let state = SATURATOR_DEFINITION
        .patch_parameter(
            &SATURATOR_DEFINITION
                .default_state()
                .expect("default saturator"),
            saturator_parameter::DRIVE,
            ParameterValue::Float(drive),
        )
        .expect("valid drive state");
    SATURATOR_DEFINITION
        .patch_parameter(
            &state,
            saturator_parameter::CURVE,
            ParameterValue::Choice(1),
        )
        .expect("digital clip state")
}

fn project_with_saturators(track_drive: f32, return_drive: f32) -> Project {
    project_with_saturators_bypassed(track_drive, return_drive, false, false)
}

fn project_with_saturators_bypassed(
    track_drive: f32,
    return_drive: f32,
    track_bypassed: bool,
    return_bypassed: bool,
) -> Project {
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    let track_device = snapshot.next_id;
    let return_track = track_device + 1;
    let return_device = track_device + 2;
    snapshot.next_id = return_device + 1;

    snapshot.tracks[0].devices.push(DeviceSnapshot {
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id: track_device,
        bypassed: track_bypassed,
        state: saturator_state(track_drive),
    });
    snapshot.tracks[0].sends.push(SendLevelSnapshot {
        return_track,
        gain: 0.5,
    });
    snapshot.return_tracks.push(ReturnTrackSnapshot {
        id: return_track,
        name: "return".into(),
        gain: 0.75,
        muted: false,
        devices: vec![DeviceSnapshot {
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: return_device,
            bypassed: return_bypassed,
            state: saturator_state(return_drive),
        }],
    });
    Project::from_snapshot(snapshot).expect("valid track and Return Saturator project")
}

fn render_project(project: &Project) -> Vec<[f32; 2]> {
    let mut playback =
        SessionPlayback::new(revision(project, Arc::new(hex_devices::SineTable::new())))
            .expect("prepared playback");
    playback
        .apply(SessionPlaybackCommand::Play)
        .expect("play command");
    let mut frames = vec![[0.0; 2]; 512];
    playback.render(&mut frames).expect("render");
    frames
}

#[test]
fn saturator_runs_on_track_and_return_chains_and_bypass_keeps_dry_alignment() {
    let no_devices = render_project(&project(1.0, 0, 8 << 32));
    let track_only = render_project(&project_with_saturators(36.0, 0.0));
    let return_only = render_project(&project_with_saturators(0.0, 36.0));
    let both = render_project(&project_with_saturators(36.0, 36.0));

    let differs = |candidate: &[[f32; 2]]| {
        candidate
            .iter()
            .zip(&no_devices)
            .any(|(candidate, dry)| candidate != dry)
    };
    assert!(differs(&track_only));
    assert!(differs(&return_only));
    assert!(differs(&both));

    // Oracle: standalone oversampled runtimes, aligned the way compensation
    // aligns the graph. Each active Saturator adds its declared latency, so the
    // track's direct path to Main waits for the return path (track -> send ->
    // return Saturator). A device's output is silent while its output origin
    // precedes the playback epoch, which discards the kernels' pre-ringing.
    let expected = |track_drive: f32, return_drive: f32| {
        let saturate = |drive: f32, input: &[[f32; 2]], input_arrival: usize| {
            let prepared = hex_devices::saturator::SaturatorDefinition::prepare_owned(
                &saturator_state(drive),
                48_000,
                &[],
            )
            .expect("standalone saturator");
            let arrival = input_arrival
                + hex_lattice::PreparedNativeProcessor::algorithmic_latency_frames(&prepared)
                    .expect("declared latency") as usize;
            let mut frames = input.to_vec();
            hex_lattice::PreparedNativeProcessor::create_runtime(&prepared)
                .process_block(&mut frames, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            frames[..arrival].fill([0.0; 2]);
            (frames, arrival)
        };
        let (track, track_arrival) = saturate(track_drive, &no_devices, 0);
        let sent: Vec<[f32; 2]> = track.iter().map(|f| [f[0] * 0.5, f[1] * 0.5]).collect();
        let (returned, return_arrival) = saturate(return_drive, &sent, track_arrival);
        assert_eq!((track_arrival, return_arrival), (11, 22));
        (0..no_devices.len())
            .map(|index| {
                let direct = index
                    .checked_sub(return_arrival - track_arrival)
                    .map_or([0.0; 2], |delayed| track[delayed]);
                [0, 1].map(|channel| {
                    f64::from(direct[channel]) + 0.75 * f64::from(returned[index][channel])
                })
            })
            .collect::<Vec<_>>()
    };
    for (actual, track_drive, return_drive) in [
        (&track_only, 36.0, 0.0),
        (&return_only, 0.0, 36.0),
        (&both, 36.0, 36.0),
    ] {
        let expected = expected(track_drive, return_drive);
        assert!(expected.iter().flatten().any(|value| value.abs() > 0.001));
        for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
            for channel in 0..2 {
                assert!(
                    (f64::from(actual[channel]) - expected[channel]).abs() <= 2.0e-6,
                    "drive {track_drive}/{return_drive} frame {index}: {actual:?} vs {expected:?}"
                );
            }
        }
    }

    // The two bypassed Saturators preserve the 11-frame latency on each
    // path. Both the aligned direct signal and the return are dry.
    let bypassed = render_project(&project_with_saturators_bypassed(36.0, 36.0, true, true));
    assert_eq!(&bypassed[..22], &[[0.0; 2]; 22]);
    for (actual, input) in bypassed[22..].iter().zip(&no_devices) {
        for channel in 0..2 {
            let expected = f64::from(input[channel]) * (1.0 + 0.75 * 0.5);
            assert!((f64::from(actual[channel]) - expected).abs() <= 2.0e-6);
        }
    }
}

#[test]
fn saturator_parameter_revision_retargets_live_track_and_return_processors() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project_with_saturators(0.0, 0.0);
    let original_revision = revision(&original, Arc::clone(&table));
    let track_device = original.tracks()[0].devices()[0].id();
    let return_device = original.return_tracks()[0].devices()[0].id();

    let mut previous = SessionPlayback::new(Arc::clone(&original_revision)).expect("previous");
    let mut reference = SessionPlayback::new(Arc::clone(&original_revision)).expect("reference");
    previous
        .apply(SessionPlaybackCommand::Play)
        .expect("previous play");
    reference
        .apply(SessionPlaybackCommand::Play)
        .expect("reference play");
    let mut previous_prefix = [[0.0; 2]; 64];
    let mut reference_prefix = [[0.0; 2]; 64];
    previous
        .render(&mut previous_prefix)
        .expect("previous warmup");
    reference
        .render(&mut reference_prefix)
        .expect("reference warmup");
    assert_eq!(previous_prefix, reference_prefix);

    let mut editor = Editor::new(original, 8).expect("editor");
    for device in [track_device, return_device] {
        let old_state = if device == track_device {
            editor.project().tracks()[0].devices()[0].state().clone()
        } else {
            editor.project().return_tracks()[0].devices()[0]
                .state()
                .clone()
        };
        let state = SATURATOR_DEFINITION
            .patch_parameter(
                &old_state,
                saturator_parameter::DRIVE,
                ParameterValue::Float(36.0),
            )
            .expect("patched state");
        editor
            .execute(Command::ReplaceDeviceState { device, state })
            .expect("replace device state");
    }
    let persisted_snapshot = editor.project().snapshot();
    let reloaded_project =
        Project::from_snapshot(persisted_snapshot.clone()).expect("reload snapshot");
    assert_eq!(reloaded_project.snapshot(), persisted_snapshot);
    let changed_revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(original_revision.session()),
        )
        .expect("prepare live parameter revision"),
    );
    let mut changed = SessionPlayback::new(changed_revision).expect("changed playback");
    changed
        .try_inherit_state_from(&mut previous)
        .expect("stateless track and Return runtimes can retarget");

    let mut old_output = [[0.0; 2]; 128];
    let mut changed_output = [[0.0; 2]; 128];
    reference.render(&mut old_output).expect("old render");
    changed.render(&mut changed_output).expect("changed render");
    assert_ne!(changed_output, old_output);

    assert!(editor.undo());
    assert!(editor.undo());
    for state in [
        editor.project().tracks()[0].devices()[0].state(),
        editor.project().return_tracks()[0].devices()[0].state(),
    ] {
        assert_eq!(
            SATURATOR_DEFINITION
                .read_parameter(state, saturator_parameter::DRIVE)
                .expect("undo state"),
            ParameterValue::Float(0.0)
        );
    }
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(
        SATURATOR_DEFINITION
            .read_parameter(
                editor.project().tracks()[0].devices()[0].state(),
                saturator_parameter::DRIVE,
            )
            .expect("redo track state"),
        ParameterValue::Float(36.0)
    );

    let mut undo = [[0.0; 2]; 128];
    previous
        .render(&mut undo)
        .expect("previous remains renderable");
    assert!(
        undo.iter()
            .all(|frame| frame.iter().all(|sample| sample.is_finite()))
    );
}
