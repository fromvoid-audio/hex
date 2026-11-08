//! Pulse #4558: a paused predecessor renders no device chains, so a full
//! live-transition capacity could never drain and a deferred handoff waited
//! until playback resumed. While the predecessor is idle its output is silent
//! (and resume ramps in from silence), so the planner settles the committed
//! transitions instead of deferring. A rendering predecessor still defers.
use super::*;
use hex_devices::auto_pan::AUTO_PAN_DEFINITION;
use hex_lattice::conformance::transition_fixture::{self, CAPACITY, Probe, TRANSITION_FRAMES};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::DeviceSnapshot;

/// One track with a single zero-latency stateful device slot (which the
/// fixture replaces) and no source instrument, so a paused transport has no
/// source monitoring and renders no device chains.
fn project_with_device_slot() -> (Project, u64) {
    let mut snapshot = audio_project(0, 32, 4).snapshot();
    let device = snapshot.next_id;
    snapshot.next_id = device + 1;
    snapshot.tracks[0].devices.push(DeviceSnapshot {
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
    let audio = crate::Audio::new(48_000, vec![[0.3, -0.1]; 32]).expect("audio");
    let media = audio_media(project, &audio, &audio);
    let revision = PreparedRevision::prepare(project, &media, Arc::clone(table), None)
        .expect("prepared revision");
    SessionPlayback::new(Arc::new(revision)).expect("playback")
}

/// A playing predecessor whose fixture slot holds `CAPACITY` live
/// transitions; returns it with the mode a busy candidate must use.
fn saturated(
    project: &Project,
    device: u64,
    table: &Arc<hex_devices::SineTable>,
    probe: &Arc<Probe>,
) -> (SessionPlayback, u8) {
    let mut previous = playback(project, table);
    previous
        .apply(SessionPlaybackCommand::Play)
        .expect("previous play");
    let mut warm = vec![[0.0; 2]; 64];
    previous.render(&mut warm).expect("warm render");
    let mut mode = 0;
    let mut chain = fixture_chain(device, mode, probe);
    for _ in 0..CAPACITY {
        mode = 1 - mode;
        let mut next = fixture_chain(device, mode, probe);
        next.inherit_from(&chain);
        chain = next;
    }
    previous.device_chains[0] = chain;
    (previous, 1 - mode)
}

#[test]
fn paused_predecessor_settles_full_transitions_instead_of_deferring_forever() {
    let table = Arc::new(hex_devices::SineTable::new());
    let (project, device) = project_with_device_slot();
    let probe = Arc::new(Probe::default());
    let (mut previous, busy_mode) = saturated(&project, device, &table, &probe);
    previous
        .apply(SessionPlaybackCommand::Pause)
        .expect("pause");
    // Paused rendering is silent and never advances the frozen chain.
    for _ in 0..16 {
        let mut frames = vec![[1.0; 2]; 64];
        previous.render(&mut frames).expect("paused render");
        assert!(frames.iter().flatten().all(|v| *v == 0.0));
    }
    assert!(!previous.device_chains[0].committed_range_is_quiescent(0..1));
    let mut candidate = playback(&project, &table);
    candidate.device_chains[0] = fixture_chain(device, busy_mode, &probe);
    assert!(candidate.device_transition_busy(&previous));
    candidate
        .try_inherit_state_from(&mut previous)
        .expect("an idle predecessor settles instead of deferring");
    assert_eq!(probe.dropped_transitions(), 0);
    // Both legacy twins held (and settled) the same CAPACITY transitions.
    assert_eq!(probe.settled_transitions(), 2 * CAPACITY as u32);
    assert!(!candidate.playing);
    assert!(candidate.tail_groups.iter().all(Option::is_none));
    // The candidate owns exactly the new switch's transition: on resume it
    // renders once for TRANSITION_FRAMES and ends, neither dropped nor doubled.
    assert!(!candidate.device_chains[0].committed_range_is_quiescent(0..1));
    candidate
        .apply(SessionPlaybackCommand::Play)
        .expect("resume");
    let mut frames = vec![[0.0; 2]; TRANSITION_FRAMES as usize - 1];
    candidate.render(&mut frames).expect("resumed render");
    assert!(!candidate.device_chains[0].committed_range_is_quiescent(0..1));
    candidate.render(&mut [[0.0; 2]]).expect("resumed render");
    assert!(candidate.device_chains[0].committed_range_is_quiescent(0..1));
    assert_eq!(probe.dropped_transitions(), 0);
}

#[test]
fn rendering_predecessor_still_defers_and_stop_clears_transitions() {
    let table = Arc::new(hex_devices::SineTable::new());
    let (project, device) = project_with_device_slot();
    let probe = Arc::new(Probe::default());
    let (mut previous, busy_mode) = saturated(&project, device, &table, &probe);
    let mut candidate = playback(&project, &table);
    candidate.device_chains[0] = fixture_chain(device, busy_mode, &probe);
    // Playing: its transitions are audible, so the planner defers.
    assert_eq!(
        candidate.try_inherit_state_from(&mut previous).err(),
        Some(SessionPlaybackError::DeviceTransitionBusy)
    );
    assert_eq!(probe.settled_transitions(), 0);
    assert!(!previous.device_chains[0].committed_range_is_quiescent(0..1));
    // Stop is an explicit discontinuity: it already resets every chain.
    previous.apply(SessionPlaybackCommand::Stop).expect("stop");
    assert!(!candidate.device_transition_busy(&previous));
    candidate
        .try_inherit_state_from(&mut previous)
        .expect("stopped predecessor hands off");
    assert_eq!(probe.dropped_transitions(), 0);
    assert_eq!(probe.settled_transitions(), 0);
}
