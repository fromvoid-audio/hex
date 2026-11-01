use super::*;
use hex_devices::eq8::{EQ8_DEFINITION, parameter};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::DeviceSnapshot;

fn eq8_state(mode: u32) -> hex_lattice_contract::NativeDeviceState {
    let mut state = EQ8_DEFINITION.default_state().expect("default eq8");
    for (id, value) in [
        (parameter::enabled(0, 0), ParameterValue::Bool(true)),
        (parameter::frequency(0, 0), ParameterValue::Float(262.0)),
        (parameter::gain(0, 0), ParameterValue::Float(12.0)),
        (parameter::MODE, ParameterValue::Choice(mode)),
    ] {
        state = EQ8_DEFINITION
            .patch_parameter(&state, id, value)
            .expect("valid eq8 parameter");
    }
    state
}

fn project_with_eq8(mode: u32) -> Project {
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    let device = snapshot.next_id;
    snapshot.next_id = device + 1;
    snapshot.tracks[0].devices.push(DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: device,
        bypassed: false,
        state: eq8_state(mode),
    });
    Project::from_snapshot(snapshot).expect("valid eq8 project")
}

fn largest_step(frames: &[[f32; 2]]) -> f32 {
    frames
        .windows(2)
        .flat_map(|w| [(w[1][0] - w[0][0]).abs(), (w[1][1] - w[0][1]).abs()])
        .fold(0.0, f32::max)
}

/// Pulse #4106: a live EQ8 stereo to mid/side switch retargets the playing
/// chain and crossfades inside it. Before, the refused retarget detached the
/// old EQ as a ringing tail beside a fresh, empty EQ, and the output stepped.
#[test]
fn eq8_mode_switch_retargets_live_chain_without_a_detached_tail() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project_with_eq8(0);
    let original_revision = revision(&original, Arc::clone(&table));
    let mut previous = SessionPlayback::new(Arc::clone(&original_revision)).expect("previous");
    previous
        .apply(SessionPlaybackCommand::Play)
        .expect("previous play");
    let mut prefix = vec![[0.0; 2]; 2048];
    previous.render(&mut prefix).expect("warmup");
    assert!(prefix[1024..].iter().flatten().any(|v| v.abs() > 1e-3));

    let device = original.tracks()[0].devices()[0].id();
    let mut editor = Editor::new(original, 8).expect("editor");
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: eq8_state(2),
        })
        .expect("switch mode");
    let changed_revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(original_revision.session()),
        )
        .expect("prepare mode revision"),
    );
    let mut changed = SessionPlayback::new(changed_revision).expect("changed playback");
    changed
        .try_inherit_state_from(&mut previous)
        .expect("mode switch retargets");
    assert_eq!(changed.tail_groups.iter().flatten().count(), 0);

    let mut after = vec![[0.0; 2]; 2048];
    changed.render(&mut after).expect("changed render");
    let mut joined = prefix[1024..].to_vec();
    joined.extend_from_slice(&after);
    let steady = largest_step(&prefix[1024..]).max(largest_step(&after[1024..]));
    let across = largest_step(&joined[1023..1024 + 512]);
    assert!(
        across <= steady * 1.25,
        "switch step {across} vs steady {steady}"
    );
}
