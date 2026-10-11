use super::*;
use hex_devices::saturator::{SATURATOR_DEFINITION, parameter as saturator_parameter};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{DeviceSnapshot, ReturnTrackSnapshot, SendLevelSnapshot};

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

    let state_with_drive = |drive| {
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
    };
    snapshot.tracks[0].devices.push(DeviceSnapshot {
        id: track_device,
        bypassed: track_bypassed,
        state: state_with_drive(track_drive),
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
            id: return_device,
            bypassed: return_bypassed,
            state: state_with_drive(return_drive),
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
fn saturator_runs_on_track_and_return_chains_and_bypass_is_transparent() {
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

    let drive_gain = 10.0_f64.powf(36.0 / 20.0);
    let digital_clip = |sample: f64, gain: f64| (sample * gain).clamp(-1.0, 1.0);
    for (input, track_output, return_output, combined_output) in no_devices
        .iter()
        .zip(&track_only)
        .zip(&return_only)
        .zip(&both)
        .map(|(((input, track), return_), combined)| (input, track, return_, combined))
    {
        for channel in 0..2 {
            let input = f64::from(input[channel]);
            let track_only_track = digital_clip(input, drive_gain);
            let track_expected =
                track_only_track + 0.75 * digital_clip(track_only_track * 0.5, 1.0);
            let return_expected = input + 0.75 * digital_clip(input * 0.5, drive_gain);
            let combined_track = digital_clip(input, drive_gain);
            let combined_expected =
                combined_track + 0.75 * digital_clip(combined_track * 0.5, drive_gain);
            for (actual, expected) in [
                (track_output[channel], track_expected),
                (return_output[channel], return_expected),
                (combined_output[channel], combined_expected),
            ] {
                assert!((f64::from(actual) - expected).abs() <= 2.0e-6);
            }
        }
    }

    let bypassed = render_project(&project_with_saturators_bypassed(36.0, 36.0, true, true));
    let unity = render_project(&project_with_saturators(0.0, 0.0));
    assert_eq!(bypassed, unity);
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
