//! Observe exact edge and clock facts for the two-route move fixture.
use super::*;

type EdgeKey = (NodeId, NodeId);

pub(crate) fn verify_held_move_clock(
    previous: Arc<SignalGraph>,
    candidate: Arc<SignalGraph>,
    donor: TrackId,
    receiver: TrackId,
) {
    let mut old = SignalRuntime::new(previous).unwrap();
    let mut next = SignalRuntime::new(Arc::clone(&candidate)).unwrap();
    let dry = |track| (NodeId::TrackInput(track), NodeId::Merge(track));
    let output = |track| (NodeId::Fader(track), NodeId::Main);
    let generator = |track| (NodeId::Generator(track), NodeId::Merge(track));
    let donor_id = donor.value();
    let receiver_id = receiver.value();
    let changed = [
        (dry(donor_id), Some(512), Some(0)),
        (dry(receiver_id), Some(0), Some(512)),
        (output(donor_id), Some(0), Some(512)),
        (output(receiver_id), Some(512), Some(0)),
        (generator(donor_id), Some(0), None),
        (generator(receiver_id), None, Some(0)),
    ];
    for (key, expected_old, expected_next) in changed {
        let delay =
            |graph: &SignalGraph| plan_edge(graph, key).map(|i| graph.plan.edges()[i].delay_frames);
        assert_eq!(delay(&old.graph), expected_old, "old edge {key:?}");
        assert_eq!(delay(&next.graph), expected_next, "new edge {key:?}");
    }
    assert_eq!(old.graph.main_input_arrival(), 512);
    assert_eq!(next.graph.main_input_arrival(), 512);
    assert_eq!(old.graph.main_latency(), next.graph.main_latency());
    let horizon = old.horizon();
    assert_eq!(horizon, next.horizon());
    let stamp = |frame| TransportStamp {
        authored_clock_epoch: std::num::NonZeroU64::new(1),
        arrangement_frame: frame,
        performance_frame: frame,
        transport_is_running: true,
        loop_pass: 0,
        tempo_micro_bpm: None,
        performance_beat_raw: frame,
        arrangement_beat_raw: Some(frame),
        meter: None,
        cycle_beats_raw: None,
    };
    let mut emitted = [[0.0; 2]];
    for frame in 0..=u64::from(horizon) {
        old.delays.begin_transaction().unwrap();
        old.delays.push_transport_stamp(stamp(frame)).unwrap();
        for &edge in &old.graph.pre_main_edges {
            old.delays.process_edge_silence(edge, &mut emitted).unwrap();
        }
        old.delays.commit_transaction().unwrap();
    }
    assert!(old.delays.history_ready());
    assert!(
        !next.can_copy_from(&old, None),
        "ordinary mapping cannot copy the swapped delay edges"
    );
    assert!(next.can_copy_hosted_move_from(&old, donor, receiver));
    next.copy_hosted_move_from(&old, donor, receiver).unwrap();
    for age in 0..=horizon {
        assert_eq!(
            next.delays.stamp_at_delay(age),
            old.delays.stamp_at_delay(age)
        );
    }

    let old_receiver_output = plan_edge(&old.graph, output(receiver_id)).unwrap();
    old.delays.begin_transaction().unwrap();
    old.delays
        .push_transport_stamp(stamp(u64::from(horizon) + 1))
        .unwrap();
    old.delays
        .process_edge(old_receiver_output, &[[0.125, -0.25]], &mut emitted)
        .unwrap();
    old.delays.commit_transaction().unwrap();
    assert_eq!(emitted, [[0.0; 2]], "the test sample is in flight");
    let mut refused = SignalRuntime::new(candidate).unwrap();
    assert!(!refused.can_copy_hosted_move_from(&old, donor, receiver));
    assert!(
        refused
            .copy_hosted_move_from(&old, donor, receiver)
            .is_err()
    );
    assert!(refused.delays.latest_stamp().is_none());
    assert!(!old.delays.edge_history_is_silent(old_receiver_output));
}

fn plan_edge(graph: &SignalGraph, key: EdgeKey) -> Option<usize> {
    graph
        .pre_main_edges
        .iter()
        .copied()
        .enumerate()
        .find_map(|(position, edge)| {
            (graph.pre_main_endpoints(position) == Some(key)).then_some(edge)
        })
}
