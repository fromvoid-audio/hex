//! Pulse #4558 for the legacy linear (Arrangement-only) runtime: a paused
//! linear graph renders nothing, so a full live-transition capacity could
//! never drain. The handoff settles the inaudible transitions instead of
//! deferring; a playing predecessor still defers.
use super::*;
use hex_lattice::conformance::transition_fixture::{self, CAPACITY, Probe};

fn main_chain(mode: u8, probe: &Arc<Probe>) -> hex_lattice::NativeRuntimeChain {
    hex_lattice::NativeRuntimeChain::new_prepared(vec![
        transition_fixture::instance(
            hex_lattice::ProcessorChainOwner::Main,
            9,
            48_000,
            mode,
            probe,
        )
        .expect("fixture instance"),
    ])
    .expect("fixture chain")
}

/// A playing linear predecessor whose Main fixture holds `CAPACITY` live
/// transitions, and a candidate whose mode change would need one more.
fn saturated(probe: &Arc<Probe>) -> (Playback, Playback) {
    let project = Arc::new(crate::Project::new(48_000, Vec::new()).expect("empty project"));
    let mut previous = Playback::from_shared(Arc::clone(&project));
    let mut mode = 0;
    let mut chain = main_chain(mode, probe);
    for _ in 0..CAPACITY {
        mode = 1 - mode;
        let mut next = main_chain(mode, probe);
        next.inherit_from(&chain);
        chain = next;
    }
    previous
        .linear_runtime
        .as_mut()
        .expect("linear runtime")
        .main_chain = chain;
    previous.apply(PlaybackCommand::Play);
    let mut candidate = Playback::from_shared(project);
    candidate
        .linear_runtime
        .as_mut()
        .expect("linear runtime")
        .main_chain = main_chain(1 - mode, probe);
    (previous, candidate)
}

#[test]
fn paused_linear_predecessor_settles_and_playing_one_defers() {
    let probe = Arc::new(Probe::default());
    let (mut previous, mut candidate) = saturated(&probe);
    assert!(matches!(
        candidate.try_inherit_runtime_from(&mut previous),
        Err(PlaybackHandoffError::Session(
            crate::SessionPlaybackError::DeviceTransitionBusy
        ))
    ));
    assert_eq!(probe.settled_transitions(), 0);

    previous.apply(PlaybackCommand::Pause);
    assert!(!previous.playing());
    for _ in 0..16 {
        previous.render(&mut [[0.0; 2]; 64]).expect("paused render");
    }
    candidate
        .try_inherit_runtime_from(&mut previous)
        .expect("a paused linear predecessor settles instead of deferring");
    // Both legacy twins held (and settled) the same CAPACITY transitions.
    assert_eq!(probe.settled_transitions(), 2 * CAPACITY as u32);
    assert_eq!(probe.dropped_transitions(), 0);
    assert!(
        !candidate
            .linear_runtime
            .as_ref()
            .expect("linear runtime")
            .main_chain
            .committed_range_is_quiescent(0..1),
        "the candidate keeps exactly its own new transition"
    );
}
