//! Pulse #4178: a device whose fixed live-transition capacity is full defers
//! the whole handoff as a transient preflight refusal, then admits it after
//! rendering drains the predecessor, without dropping a live branch or
//! detaching the old chain as a tail.
use super::*;
use crate::PlaybackHandoffError;
use hex_devices::auto_pan::AUTO_PAN_DEFINITION;
use hex_lattice::conformance::transition_fixture::{self, CAPACITY, Probe, TRANSITION_FRAMES};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::DeviceSnapshot;

/// One track whose single zero-latency stateful device slot the fixture
/// replaces in both graphs, keeping the prepared layout and identity.
fn project_with_device_slot() -> (Project, u64) {
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
        state: AUTO_PAN_DEFINITION
            .default_state()
            .expect("default auto pan"),
    });
    (
        Project::from_snapshot(snapshot).expect("valid device project"),
        device,
    )
}

fn fixture_chain(device: u64, mode: u8, probe: &Arc<Probe>) -> hex_lattice::NativeRuntimeChain {
    hex_lattice::NativeRuntimeChain::new_prepared(vec![
        transition_fixture::instance(
            hex_lattice::ProcessorChainOwner::Track(1),
            device,
            48_000,
            mode,
            probe,
        )
        .expect("fixture instance"),
    ])
    .expect("fixture chain")
}

fn playback(project: &Project, table: &Arc<hex_devices::SineTable>) -> SessionPlayback {
    SessionPlayback::new(revision(project, Arc::clone(table))).expect("playback")
}

#[test]
fn full_device_transition_capacity_defers_the_whole_handoff_until_it_drains() {
    let table = Arc::new(hex_devices::SineTable::new());
    let (project, device) = project_with_device_slot();
    let probe = Arc::new(Probe::default());
    let mut previous = playback(&project, &table);
    previous
        .apply(SessionPlaybackCommand::Play)
        .expect("previous play");
    // Saturate the committed runtime through ordinary mode-switch handoffs.
    let mut mode = 0;
    let mut chain = fixture_chain(device, mode, &probe);
    for _ in 0..CAPACITY {
        mode = 1 - mode;
        let mut next = fixture_chain(device, mode, &probe);
        next.inherit_from(&chain);
        chain = next;
    }
    assert_eq!(previous.device_chains.len(), 1);
    previous.device_chains[0] = chain;
    assert!(!previous.device_chains[0].committed_range_is_quiescent(0..1));

    let mut candidate = playback(&project, &table);
    candidate.device_chains[0] = fixture_chain(device, 1 - mode, &probe);
    assert!(candidate.device_transition_busy(&previous));
    let refused = candidate.try_inherit_state_from(&mut previous);
    assert_eq!(
        refused.err(),
        Some(SessionPlaybackError::DeviceTransitionBusy)
    );
    assert!(
        PlaybackHandoffError::Session(SessionPlaybackError::DeviceTransitionBusy).is_transient()
    );
    assert_eq!(
        PlaybackHandoffError::Session(SessionPlaybackError::DeviceTransitionBusy).rejection(),
        None
    );
    // Nothing moved: the candidate stays admissible and the predecessor keeps
    // its chain, playing state and an empty retirement bank.
    assert!(candidate.require_fresh_candidate().is_ok());
    assert!(previous.playing);
    assert!(previous.tail_groups.iter().all(Option::is_none));
    assert!(!previous.device_chains[0].committed_range_is_quiescent(0..1));

    // Rendering the predecessor across block partitions drains the oldest
    // transition; the retry then inherits without a capacity violation.
    let mut rendered = 0;
    for block in [1, 13, 50] {
        assert!(candidate.device_transition_busy(&previous));
        let mut frames = vec![[0.0; 2]; block];
        previous.render(&mut frames).expect("predecessor render");
        rendered += block;
    }
    assert_eq!(rendered, TRANSITION_FRAMES as usize);
    assert!(!candidate.device_transition_busy(&previous));
    candidate
        .try_inherit_state_from(&mut previous)
        .expect("drained transition admits the handoff");
    assert!(candidate.tail_groups.iter().all(Option::is_none));
    assert!(!candidate.device_chains[0].committed_range_is_quiescent(0..1));
    assert_eq!(probe.dropped_transitions(), 0);
}

#[test]
fn same_mode_revision_and_reset_predecessor_are_never_busy() {
    let table = Arc::new(hex_devices::SineTable::new());
    let (project, device) = project_with_device_slot();
    let probe = Arc::new(Probe::default());
    let mut previous = playback(&project, &table);
    let mut chain = fixture_chain(device, 0, &probe);
    for mode in [1, 0] {
        let mut next = fixture_chain(device, mode, &probe);
        next.inherit_from(&chain);
        chain = next;
    }
    previous.device_chains[0] = chain;
    let mut same_mode = playback(&project, &table);
    same_mode.device_chains[0] = fixture_chain(device, 0, &probe);
    assert!(!same_mode.device_transition_busy(&previous));
    previous.device_chains[0].reset();
    let mut switched = playback(&project, &table);
    switched.device_chains[0] = fixture_chain(device, 1, &probe);
    assert!(!switched.device_transition_busy(&previous));
    switched
        .try_inherit_state_from(&mut previous)
        .expect("reset predecessor hands off");
    assert_eq!(probe.dropped_transitions(), 0);
}

#[test]
fn idle_busy_transition_with_return_tail_refuses_without_settling_donor() {
    let table = Arc::new(hex_devices::SineTable::new());
    let (project, device) = project_with_device_slot();
    let mut editor = hex_project::Editor::new(project, 8).unwrap();
    editor
        .execute(hex_project::Command::AddReturnTrack {
            name: "room".into(),
        })
        .unwrap();
    let project = editor.project();
    let probe = Arc::new(Probe::default());
    let mut previous = playback(project, &table);
    // A paused transport away from the origin freezes the retained tail.
    previous.performance_frame = 1;
    previous.arrangement_frame = 1;
    let mut mode = 0;
    let mut chain = fixture_chain(device, mode, &probe);
    for _ in 0..CAPACITY {
        mode = 1 - mode;
        let mut next = fixture_chain(device, mode, &probe);
        next.inherit_from(&chain);
        chain = next;
    }
    previous.device_chains[0] = chain;
    let bus = &previous.revision.arrangement().return_buses()[0];
    let tail = super::super::return_effect_tails::ReturnEffectTail::new(
        hex_lattice::NativeRuntimeChain::new_prepared(vec![]).unwrap(),
        bus,
    );
    previous.return_effect_tails.slots_mut()[0] = Some(tail);
    let mut candidate = playback(project, &table);
    candidate.device_chains[0] = fixture_chain(device, 1 - mode, &probe);
    assert!(previous.renders_no_device_chains());
    assert!(candidate.device_transition_busy(&previous));
    assert_eq!(
        candidate.try_inherit_state_live_from(&mut previous),
        Err(SessionPlaybackError::DeviceTransitionBusy)
    );
    assert!(candidate.device_transition_busy(&previous));
    assert!(previous.return_effect_tails.slots()[0].is_some());
    assert_eq!(probe.dropped_transitions(), 0);
}
