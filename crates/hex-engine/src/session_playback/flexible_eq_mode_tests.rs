use super::*;
use hex_devices::eq::{EqState, Node, NodeId};
use hex_project::DeviceSnapshot;

fn eq_settings(mode: u8) -> EqState {
    let mut settings = EqState::default();
    settings
        .add(Node::new(NodeId::new(1).expect("node id"), 262.0, 12.0, 0).expect("node"))
        .expect("add node");
    settings.mode = mode;
    settings
}

fn project_with_eq(mode: u8) -> Project {
    let mut snapshot = project(1.0, 0, 8 << 32).snapshot();
    let device = snapshot.next_id;
    snapshot.next_id = device + 1;
    snapshot.tracks[0].devices.push(DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
        id: device,
        bypassed: false,
        state: eq_settings(mode).encode().expect("eq state"),
    });
    Project::from_snapshot(snapshot).expect("valid flexible eq project")
}

fn largest_step(frames: &[[f32; 2]]) -> f32 {
    frames
        .windows(2)
        .flat_map(|w| [(w[1][0] - w[0][0]).abs(), (w[1][1] - w[0][1]).abs()])
        .fold(0.0, f32::max)
}

/// Pulse #4122: a live flexible EQ stereo to mid/side switch retargets the
/// playing chain and crossfades inside it. Before, the refused retarget
/// detached the old EQ as a ringing tail beside a fresh, empty EQ.
#[test]
fn flexible_eq_mode_switch_retargets_live_chain_without_a_detached_tail() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project_with_eq(0);
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
            state: eq_settings(2).encode().expect("mid/side state"),
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
