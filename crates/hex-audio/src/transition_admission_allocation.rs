//! Pulse #4178 callback-side live-transition admission: the busy query,
//! predecessor rendering that drains it, and the admitted history copy.
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
