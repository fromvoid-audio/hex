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
        modulation: Default::default(),
        parent: None,
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

/// Prepare the editor's project after switching the flexible EQ to `mode`.
fn switch_mode(
    editor: &mut Editor,
    mode: u8,
    table: &Arc<hex_devices::SineTable>,
    base: &PreparedRevision,
) -> Arc<PreparedRevision> {
    let device = editor.project().tracks()[0].devices()[0].id();
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: eq_settings(mode).encode().expect("eq state"),
        })
        .expect("switch mode");
    Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(table),
            Some(base.session()),
        )
        .expect("prepare mode revision"),
    )
}

/// Pulse #4122: a second flexible EQ mode switch while the first is still
/// fading finds the device's single fade branch full. The whole handoff is
/// refused as transiently busy with the predecessor intact (no detached tail,
/// no dropped routing); once the predecessor renders through the fade, the
/// retry is admitted and the output stays continuous across both switches.
#[test]
fn flexible_eq_second_mode_switch_defers_until_the_first_fade_drains() {
    let table = Arc::new(hex_devices::SineTable::new());
    let original = project_with_eq(0);
    let original_revision = revision(&original, Arc::clone(&table));
    let mut first = SessionPlayback::new(Arc::clone(&original_revision)).expect("first");
    first.apply(SessionPlaybackCommand::Play).expect("play");
    let mut output = vec![[0.0; 2]; 2048];
    first.render(&mut output).expect("warmup");

    let mut editor = Editor::new(original, 8).expect("editor");
    let mid_side = switch_mode(&mut editor, 2, &table, &original_revision);
    let mut second = SessionPlayback::new(Arc::clone(&mid_side)).expect("mid/side");
    assert!(!second.device_transition_busy(&first));
    second
        .try_inherit_state_from(&mut first)
        .expect("first switch retargets");
    let mut block = vec![[0.0; 2]; 64];
    second.render(&mut block).expect("fading render");
    output.extend_from_slice(&block);

    let left_right = switch_mode(&mut editor, 1, &table, &mid_side);
    let mut third = SessionPlayback::new(left_right).expect("left/right");
    assert!(third.device_transition_busy(&second));
    assert_eq!(
        third.try_inherit_state_from(&mut second).err(),
        Some(SessionPlaybackError::DeviceTransitionBusy)
    );
    assert!(third.require_fresh_candidate().is_ok());
    assert!(second.playing);
    assert!(second.tail_groups.iter().all(Option::is_none));
    // 256 fade frames at 48 kHz, 64 already rendered: drain in odd blocks.
    let mut rendered = 64;
    while third.device_transition_busy(&second) {
        assert!(rendered < 256, "fade never drained");
        let mut block = vec![[0.0; 2]; 37];
        second.render(&mut block).expect("draining render");
        output.extend_from_slice(&block);
        rendered += 37;
    }
    assert!(rendered >= 256);
    third
        .try_inherit_state_from(&mut second)
        .expect("drained fade admits the second switch");
    assert_eq!(third.tail_groups.iter().flatten().count(), 0);
    let mut after = vec![[0.0; 2]; 2048];
    third.render(&mut after).expect("left/right render");
    output.extend_from_slice(&after);

    let steady =
        largest_step(&output[1024..2048]).max(largest_step(&output[output.len() - 1024..]));
    let across = largest_step(&output[1024..output.len() - 1024]);
    assert!(
        across <= steady * 1.25,
        "switch step {across} vs steady {steady}"
    );
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
