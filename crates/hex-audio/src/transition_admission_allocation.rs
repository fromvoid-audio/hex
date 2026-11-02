//! Pulse #4178 callback-side live-transition admission (the busy query,
//! predecessor rendering that drains it, Pulse #4558 idle settlement and the
//! admitted history copy).
//! Construction stays outside the measured scope. This covers the lattice
//! chain/provider boundary with the original fixture; `SessionPlayback`'s
//! planner pairing only iterates existing chains (inspection, not measured).
use hex_lattice::{
    NativeRuntimeChain, ProcessorChainOwner,
    conformance::transition_fixture::{self, CAPACITY, Probe, TRANSITION_FRAMES},
};
use std::sync::Arc;

fn chain(mode: u8, probe: &Arc<Probe>) -> NativeRuntimeChain {
    NativeRuntimeChain::new_prepared(vec![
        transition_fixture::instance(ProcessorChainOwner::Track(1), 3, 48_000, mode, probe)
            .unwrap(),
    ])
    .unwrap()
}

#[test]
fn busy_query_drain_and_admitted_inherit_allocate_and_free_nothing() {
    let probe = Arc::new(Probe::default());
    let mut chains: Vec<_> = (0..=CAPACITY)
        .map(|step| chain((step % 2) as u8, &probe))
        .collect();
    let mut candidate = chain(((CAPACITY + 1) % 2) as u8, &probe);
    let mut frames = [[0.25, -0.5]; TRANSITION_FRAMES as usize];
    let mut busy = [false; 2];
    let measurement = allocation_counter::measure(|| {
        for index in 1..chains.len() {
            let (done, rest) = chains.split_at_mut(index);
            rest[0].inherit_from(&done[index - 1]);
        }
        let Some(full) = chains.last_mut() else {
            return;
        };
        busy[0] = candidate.transition_busy_from(full);
        full.begin_block();
        full.process_block(&mut frames).unwrap();
        full.commit_block();
        busy[1] = candidate.transition_busy_from(full);
        candidate.inherit_from(full);
    });
    assert_eq!(
        (
            measurement.count_total,
            measurement.bytes_total,
            measurement.count_current,
            measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(busy, [true, false]);
    assert_eq!(probe.dropped_transitions(), 0);
    assert_eq!(frames, [[0.25, -0.5]; TRANSITION_FRAMES as usize]);
}

fn pair(modes: [u8; 2], probe: &Arc<Probe>) -> NativeRuntimeChain {
    NativeRuntimeChain::new_prepared(
        [3, 4]
            .into_iter()
            .zip(modes)
            .map(|(device, mode)| {
                transition_fixture::instance(
                    ProcessorChainOwner::Track(1),
                    device,
                    48_000,
                    mode,
                    probe,
                )
                .unwrap()
            })
            .collect(),
    )
    .unwrap()
}

/// Pulse #4558: settling an idle predecessor touches only the runtime the
/// candidate is busy from, unblocks the handoff without rendering, and the
/// busy query, settlement and admitted inherit allocate and free nothing.
#[test]
fn idle_settlement_of_busy_pairs_allocates_and_frees_nothing() {
    let probe = Arc::new(Probe::default());
    // Device 3 flips mode every step (CAPACITY live transitions); device 4
    // switches once and keeps one live transition.
    let mut chains: Vec<_> = (0..=CAPACITY)
        .map(|step| pair([(step % 2) as u8, u8::from(step > 0)], &probe))
        .collect();
    let mut candidate = pair([((CAPACITY + 1) % 2) as u8, 1], &probe);
    let mut busy = [false; 2];
    let measurement = allocation_counter::measure(|| {
        for index in 1..chains.len() {
            let (done, rest) = chains.split_at_mut(index);
            rest[0].inherit_from(&done[index - 1]);
        }
        let Some(full) = chains.last_mut() else {
            return;
        };
        busy[0] = candidate.transition_busy_from(full);
        full.settle_idle_transitions_for(&candidate);
        busy[1] = candidate.transition_busy_from(full);
        candidate.inherit_from(full);
    });
    assert_eq!(
        (
            measurement.count_total,
            measurement.bytes_total,
            measurement.count_current,
            measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(busy, [true, false]);
    // Only device 3's two legacy twins were settled; device 4's transition
    // is still live in the predecessor and carried by the candidate.
    assert_eq!(probe.settled_transitions(), 2 * CAPACITY as u32);
    assert_eq!(probe.dropped_transitions(), 0);
    let full = chains.last().unwrap();
    assert!(full.committed_range_is_quiescent(0..1));
    assert!(!full.committed_range_is_quiescent(1..2));
    assert!(!candidate.committed_range_is_quiescent(1..2));
}
