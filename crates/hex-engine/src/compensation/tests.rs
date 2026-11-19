use super::*;
use std::collections::VecDeque;

fn node(key: u64, latency: u32) -> LatencyNode {
    LatencyNode {
        key,
        latency_frames: Some(latency),
    }
}

fn edge(key: u64, from: u64, to: u64) -> LatencyEdge {
    LatencyEdge { key, from, to }
}

fn plan(nodes: &[LatencyNode], edges: &[LatencyEdge]) -> LatencyPlan {
    LatencyPlan::build(nodes, edges, CompensationLimits::default()).unwrap()
}

fn pair(delay: u32) -> DelayPair {
    DelayPair::new(delay, CompensationLimits::default()).unwrap()
}

#[test]
fn delay_quiescence_follows_committed_pcm_across_transfer_and_rollback() {
    let mut source = pair(2);
    let mut target = pair(2);
    let impulse = [0.5, -0.25];
    let silence = [0.0; 2];
    source.begin_transaction().unwrap();
    source.process_in_place(&mut [impulse]).unwrap();
    assert!(source.history_is_silent());
    source.commit_transaction().unwrap();
    assert!(!source.history_is_silent());
    assert!(target.can_copy_history_from(&source));
    target.copy_history_from(&source);

    target.begin_transaction().unwrap();
    target.process_in_place(&mut [silence; 2]).unwrap();
    target.rollback_transaction().unwrap();
    assert!(!target.history_is_silent());
    target.begin_transaction().unwrap();
    let mut tail = [silence; 2];
    target.process_in_place(&mut tail).unwrap();
    target.commit_transaction().unwrap();
    assert_eq!(tail, [silence, impulse]);
    assert!(target.history_is_silent());
    source.reset_epoch();
    assert!(source.history_is_silent());
}

fn sample(index: usize) -> Frame {
    [index as f32 + 1.0, -(index as f32) - 0.25]
}

#[test]
fn planner_sums_serial_latency_and_aligns_each_reconvergence() {
    // source -> a(3) -> c(4) -> merge(2) -> sink
    //       \-> b(5) ---------/    \----------> sink
    let nodes = [
        node(1, 0),
        node(2, 3),
        node(3, 5),
        node(4, 4),
        node(5, 2),
        node(6, 0),
    ];
    let edges = [
        edge(1, 1, 2),
        edge(2, 1, 3),
        edge(3, 2, 4),
        edge(4, 4, 5),
        edge(5, 3, 5),
        edge(6, 5, 6),
        edge(7, 3, 6),
    ];
    let compiled = plan(&nodes, &edges);
    assert_eq!(compiled.try_clone().unwrap(), compiled);
    assert_eq!(compiled.max_latency_frames(), 9);
    assert_eq!(compiled.nodes()[4].input_arrival_frames, 7);
    assert_eq!(compiled.nodes()[4].output_arrival_frames, 9);
    assert_eq!(compiled.edges()[4].delay_frames, 2);
    assert_eq!(compiled.edges()[6].delay_frames, 4);
    let mut reversed_nodes = nodes;
    let mut reversed_edges = edges;
    reversed_nodes.reverse();
    reversed_edges.reverse();
    let reversed = plan(&reversed_nodes, &reversed_edges);
    assert_eq!(compiled.nodes(), reversed.nodes());
    assert_eq!(compiled.edges(), reversed.edges());
    assert_eq!(compiled.topological_order(), reversed.topological_order());
}

#[test]
fn planner_rejects_unknown_duplicate_missing_cyclic_and_overlimit_graphs() {
    let limits = CompensationLimits::default();
    assert!(matches!(
        LatencyPlan::build(
            &[LatencyNode {
                key: 1,
                latency_frames: None
            }],
            &[],
            limits
        ),
        Err(CompensationError::UnknownLatency)
    ));
    assert!(matches!(
        LatencyPlan::build(&[node(1, 0), node(1, 0)], &[], limits),
        Err(CompensationError::DuplicateNode)
    ));
    assert!(matches!(
        LatencyPlan::build(&[node(1, 0)], &[edge(1, 1, 2)], limits),
        Err(CompensationError::MissingNode)
    ));
    assert!(matches!(
        LatencyPlan::build(
            &[node(1, 0), node(2, 0)],
            &[edge(1, 1, 2), edge(1, 1, 2)],
            limits
        ),
        Err(CompensationError::DuplicateEdge)
    ));
    assert!(matches!(
        LatencyPlan::build(&[node(1, 0)], &[edge(1, 1, 1)], limits),
        Err(CompensationError::Cycle)
    ));
    assert!(matches!(
        LatencyPlan::build(
            &[node(1, 0), node(2, 0)],
            &[edge(1, 1, 2), edge(2, 2, 1)],
            limits
        ),
        Err(CompensationError::Cycle)
    ));
    assert!(matches!(
        LatencyPlan::build(&[node(1, 65_536), node(2, 1)], &[edge(1, 1, 2)], limits),
        Err(CompensationError::LatencyLimit)
    ));
    assert!(matches!(
        LatencyPlan::build(&[node(1, u32::MAX)], &[], limits),
        Err(CompensationError::LatencyLimit)
    ));
    assert!(matches!(
        LatencyPlan::build(&vec![node(1, 0); MAX_COMPENSATION_NODES + 1], &[], limits),
        Err(CompensationError::GraphLimit)
    ));
    assert!(matches!(
        LatencyPlan::build(
            &[],
            &vec![edge(1, 1, 1); MAX_COMPENSATION_EDGES + 1],
            limits
        ),
        Err(CompensationError::GraphLimit)
    ));
}

#[test]
fn stereo_delay_matches_fifo_oracle_over_partitions_and_repeated_transactions() {
    for delay in [0, 1, 7, 257, 1024] {
        let mut delayed = pair(delay);
        let mut oracle = VecDeque::from(vec![[0.0; 2]; delay as usize]);
        let mut source_index = 0;
        for count in [1, 13, 256, 907, 2, 1027] {
            delayed.begin_transaction().unwrap();
            let mut frames = (source_index..source_index + count)
                .map(sample)
                .collect::<Vec<_>>();
            let expected = frames
                .iter()
                .map(|&frame| {
                    oracle.push_back(frame);
                    oracle.pop_front().unwrap()
                })
                .collect::<Vec<_>>();
            // Arbitrary subchunks share one transaction, including >256 frames.
            for chunk in frames.chunks_mut(73) {
                delayed.process_in_place(chunk).unwrap();
            }
            assert_eq!(frames, expected, "delay {delay}, request {count}");
            delayed.commit_transaction().unwrap();
            source_index += count;
        }
    }
}

#[test]
fn rollback_after_prior_chunks_restores_partial_wrapped_and_full_rings() {
    for delay in [0, 1, 7, 513] {
        for failed_count in [1, 4, 255, 256, 257, 1029] {
            let mut delayed = pair(delay);
            let mut reference = pair(delay);
            for runtime in [&mut delayed, &mut reference] {
                runtime.begin_transaction().unwrap();
                runtime
                    .process_in_place(&mut (0..11).map(sample).collect::<Vec<_>>())
                    .unwrap();
                runtime.commit_transaction().unwrap();
            }
            delayed.begin_transaction().unwrap();
            let mut failed = (11..11 + failed_count).map(sample).collect::<Vec<_>>();
            for chunk in failed.chunks_mut(127) {
                delayed.process_in_place(chunk).unwrap();
            }
            delayed.rollback_transaction().unwrap();
            let mut actual = (11..1051).map(sample).collect::<Vec<_>>();
            let mut expected = actual.clone();
            delayed.begin_transaction().unwrap();
            reference.begin_transaction().unwrap();
            delayed.process_in_place(&mut actual).unwrap();
            reference.process_in_place(&mut expected).unwrap();
            assert_eq!(
                actual, expected,
                "delay {delay}, failed request {failed_count}"
            );
            delayed.commit_transaction().unwrap();
            reference.commit_transaction().unwrap();
        }
    }
}

#[test]
fn impulses_align_through_serial_processors_and_parallel_runtime_edges() {
    let graph = plan(
        &[node(1, 3), node(2, 4), node(3, 2), node(4, 0)],
        &[edge(1, 1, 2), edge(2, 2, 4), edge(3, 3, 4)],
    );
    let mut runtime = CompensationRuntime::new(graph, CompensationLimits::default()).unwrap();
    let mut first = pair(3);
    let mut second = pair(4);
    let mut parallel = pair(2);
    let mut impulse = [[0.0; 2]; 31];
    impulse[0] = [1.0, -0.5];
    let mut serial_audio = impulse;
    let mut parallel_audio = impulse;
    first.begin_transaction().unwrap();
    second.begin_transaction().unwrap();
    parallel.begin_transaction().unwrap();
    first.process_in_place(&mut serial_audio).unwrap();
    second.process_in_place(&mut serial_audio).unwrap();
    parallel.process_in_place(&mut parallel_audio).unwrap();
    runtime.begin_transaction().unwrap();
    runtime.process_edge_in_place(1, &mut serial_audio).unwrap();
    runtime
        .process_edge_in_place(2, &mut parallel_audio)
        .unwrap();
    assert_eq!(serial_audio, parallel_audio);
    assert_eq!(serial_audio[7], [1.0, -0.5]);
    assert_eq!(
        serial_audio
            .iter()
            .filter(|&&frame| frame != [0.0; 2])
            .count(),
        1
    );
    runtime.commit_transaction().unwrap();
}

#[test]
fn silence_advances_pending_impulse_and_epoch_reset_removes_old_content() {
    let graph = plan(
        &[node(1, 0), node(2, 5), node(3, 0)],
        &[edge(1, 1, 3), edge(2, 2, 3)],
    );
    let mut runtime = CompensationRuntime::new(graph, CompensationLimits::default()).unwrap();
    runtime.begin_transaction().unwrap();
    runtime
        .process_edge_in_place(0, &mut [[1.0, -1.0]])
        .unwrap();
    runtime.commit_transaction().unwrap();
    runtime.begin_transaction().unwrap();
    let mut silence = [[99.0; 2]; 6];
    runtime.process_edge_silence(0, &mut silence).unwrap();
    assert_eq!(silence[4], [1.0, -1.0]);
    assert_eq!(
        silence.iter().filter(|&&frame| frame != [0.0; 2]).count(),
        1
    );
    runtime.rollback_transaction().unwrap();
    runtime.reset_epoch();
    runtime.begin_transaction().unwrap();
    runtime.process_edge_silence(0, &mut silence).unwrap();
    assert_eq!(silence, [[0.0; 2]; 6]);
    assert!(!runtime.history_ready());
}

fn stamp(index: usize) -> TransportStamp {
    TransportStamp {
        authored_clock_epoch: NonZeroU64::new((index / 4) as u64 + 1),
        tempo_micro_bpm: Some(123_000_000),
        performance_frame: index as u64,
        transport_is_running: !index.is_multiple_of(3),
        arrangement_frame: (index % 4) as u64,
        loop_pass: (index / 4) as u64,
        performance_beat_raw: (index as u64) << 30,
        arrangement_beat_raw: Some(((index % 4) as u64) << 30),
        meter: None,
        cycle_beats_raw: None,
    }
}

#[test]
fn transport_origins_preserve_loop_identity_and_rollback_long_requests() {
    let graph = plan(&[node(1, 3), node(2, 2)], &[edge(1, 1, 2)]);
    let mut runtime = CompensationRuntime::new(graph, CompensationLimits::default()).unwrap();
    runtime.begin_transaction().unwrap();
    for index in 0..19 {
        runtime.push_transport_stamp(stamp(index)).unwrap();
        assert_eq!(
            runtime.node_input_stamp(1).unwrap(),
            index.checked_sub(3).map(stamp)
        );
        assert_eq!(
            runtime.node_output_stamp(1).unwrap(),
            index.checked_sub(5).map(stamp)
        );
        assert_eq!(runtime.history_ready(), index >= 5);
    }
    runtime.commit_transaction().unwrap();
    runtime.begin_transaction().unwrap();
    for index in 19..1027 {
        runtime.push_transport_stamp(stamp(index)).unwrap();
    }
    runtime.rollback_transaction().unwrap();
    assert_eq!(runtime.node_input_stamp(1).unwrap(), Some(stamp(15)));
    assert_eq!(runtime.node_output_stamp(1).unwrap(), Some(stamp(13)));
    runtime.begin_transaction().unwrap();
    runtime.push_transport_stamp(stamp(19)).unwrap();
    assert_eq!(runtime.node_output_stamp(1).unwrap(), Some(stamp(14)));
    runtime.reset_epoch();
    assert!(!runtime.transaction_active());
    assert_eq!(runtime.node_input_stamp(1).unwrap(), None);
    assert_eq!(runtime.stamp_at_delay(u32::MAX), None);
}

#[test]
fn stage_origin_preview_matches_actual_pushes_and_rollback() {
    let graph = plan(&[node(1, 3), node(2, 2)], &[edge(1, 1, 2)]);
    let mut runtime = CompensationRuntime::new(graph, CompensationLimits::default()).unwrap();
    let preview = |runtime: &CompensationRuntime, delay, offset, start| {
        CompensationRuntime::preview_stage_frame(Some(runtime), delay, offset, |i| {
            Ok::<_, ()>(Some(stamp((start + i) as usize).arrangement_frame))
        })
        .unwrap()
    };
    assert_eq!(preview(&runtime, 5, 0, 0), None);
    assert_eq!(preview(&runtime, 5, 5, 0), Some(stamp(0).arrangement_frame));
    runtime.begin_transaction().unwrap();
    for index in 0..7 {
        runtime.push_transport_stamp(stamp(index)).unwrap();
    }
    runtime.commit_transaction().unwrap();
    let incoming = std::array::from_fn::<_, 9, _>(|index| Some(stamp(7 + index)));
    for delay in [0, 3, 5] {
        let predicted: Vec<_> = (0..9)
            .map(|offset| preview(&runtime, delay, offset, 7))
            .collect();
        let window = OriginWindow {
            history: &runtime,
            incoming: &incoming,
            delay,
        };
        let projected = std::array::from_fn::<_, 9, _>(|index| window.stamp(index));
        runtime.begin_transaction().unwrap();
        for (offset, expected) in predicted.into_iter().enumerate() {
            runtime.push_transport_stamp(stamp(7 + offset)).unwrap();
            assert_eq!(runtime.stamp_at_delay(delay), projected[offset]);
            assert_eq!(
                runtime
                    .stamp_at_delay(delay)
                    .map(|stamp| stamp.arrangement_frame),
                expected
            );
        }
        runtime.rollback_transaction().unwrap();
        assert_eq!(runtime.stamp_at_delay(0), Some(stamp(6)));
    }
    runtime.reset_epoch();
    assert_eq!(preview(&runtime, 3, 0, 100), None);
    assert_eq!(
        preview(&runtime, 3, 3, 100),
        Some(stamp(100).arrangement_frame)
    );
}

#[test]
fn authored_origins_use_running_content_after_compensation_and_epoch_reset() {
    use crate::signal_graph::OriginClock;
    use hex_lattice::NativeInstrumentClock;

    let graph = plan(&[node(1, 3), node(2, 0)], &[edge(1, 1, 2)]);
    let mut runtime = CompensationRuntime::new(graph, CompensationLimits::default()).unwrap();
    let incoming = std::array::from_fn::<_, 8, _>(|index| Some(stamp(index)));
    let window = OriginWindow {
        history: &runtime,
        incoming: &incoming,
        delay: 3,
    };
    let expected = [None, None, None, None, Some(1), Some(2), None, Some(0)];
    assert_eq!(
        std::array::from_fn(|offset| window.authored_arrangement_frame_at_offset(offset)),
        expected
    );

    assert_eq!(
        std::array::from_fn::<_, 8, _>(|offset| window.authored_clock_epoch_at_offset(offset)),
        std::array::from_fn(|offset| offset
            .checked_sub(3)
            .and_then(|source| stamp(source).authored_clock_epoch)),
    );

    runtime.begin_transaction().unwrap();
    for (index, expected) in expected.into_iter().enumerate() {
        runtime.push_transport_stamp(stamp(index)).unwrap();
        let stamp = runtime.node_input_stamp(1).unwrap();
        let clock = OriginClock(stamp.as_ref());
        assert_eq!(clock.authored_arrangement_frame_at_offset(0), expected);
        assert_eq!(clock.authored_arrangement_frame_at_offset(1), None);
        assert_eq!(
            clock.authored_clock_epoch_at_offset(0),
            index
                .checked_sub(3)
                .and_then(|source| self::stamp(source).authored_clock_epoch)
        );
        assert_eq!(clock.authored_clock_epoch_at_offset(1), None);
    }
    runtime.commit_transaction().unwrap();
    runtime.reset_epoch();
    runtime.begin_transaction().unwrap();
    for index in 101..105 {
        runtime.push_transport_stamp(stamp(index)).unwrap();
        let stamp = runtime.node_input_stamp(1).unwrap();
        let clock = OriginClock(stamp.as_ref());
        assert_eq!(
            clock.authored_arrangement_frame_at_offset(0),
            (index == 104).then_some(1)
        );
    }
}

#[test]
fn invalid_calls_refuse_before_mutating_ring_state() {
    let mut delayed = pair(2);
    assert_eq!(
        delayed.process_in_place(&mut [[1.0; 2]]),
        Err(CompensationError::TransactionInactive)
    );
    delayed.begin_transaction().unwrap();
    assert_eq!(
        delayed.begin_transaction(),
        Err(CompensationError::TransactionActive)
    );
    assert_eq!(
        delayed.process(&[[1.0; 2]], &mut []),
        Err(CompensationError::BufferLength)
    );
    let mut frames = [[3.0; 2]; 3];
    delayed.process_in_place(&mut frames).unwrap();
    assert_eq!(frames, [[0.0; 2], [0.0; 2], [3.0; 2]]);
    delayed.rollback_transaction().unwrap();
    assert_eq!(
        delayed.commit_transaction(),
        Err(CompensationError::TransactionInactive)
    );
    let mut runtime =
        CompensationRuntime::new(plan(&[node(1, 0)], &[]), CompensationLimits::default()).unwrap();
    runtime.begin_transaction().unwrap();
    assert_eq!(
        runtime.process_edge_in_place(0, &mut []),
        Err(CompensationError::InvalidIndex)
    );
    assert_eq!(
        runtime.node_output_stamp(9),
        Err(CompensationError::InvalidIndex)
    );
    assert!(!runtime.history_ready());
    let exact = TransportStamp {
        performance_beat_raw: u64::MAX,
        ..stamp(0)
    };
    runtime.push_transport_stamp(exact).unwrap();
    assert_eq!(runtime.stamp_at_delay(0), Some(exact));
    assert!(runtime.history_ready());
    runtime.rollback_transaction().unwrap();
    assert!(!runtime.history_ready());
}

#[test]
fn storage_counts_actual_twin_capacities_and_enforces_exact_budget() {
    let zero = DelayPair::new(
        0,
        CompensationLimits {
            max_cumulative_frames: 0,
            max_storage_bytes: 0,
        },
    )
    .unwrap();
    assert_eq!(zero.storage_bytes(), 0);
    assert!(zero.history_ready());
    let delay = pair(513);
    assert_eq!(delay.storage_bytes(), 2 * 513 * size_of::<Frame>());
    assert!(matches!(
        DelayPair::new(
            513,
            CompensationLimits {
                max_storage_bytes: delay.storage_bytes() - 1,
                ..CompensationLimits::default()
            }
        ),
        Err(CompensationError::StorageLimit)
    ));
    assert!(matches!(
        DelayPair::new(MAX_COMPENSATION_FRAMES + 1, CompensationLimits::default()),
        Err(CompensationError::LatencyLimit)
    ));
    assert!(matches!(
        DelayPair::new(
            1,
            CompensationLimits {
                max_cumulative_frames: u32::MAX,
                ..CompensationLimits::default()
            }
        ),
        Err(CompensationError::PolicyLimit)
    ));
    let graph = || {
        plan(
            &[node(1, 0), node(2, 7), node(3, 0)],
            &[edge(1, 1, 3), edge(2, 2, 3)],
        )
    };
    let runtime = CompensationRuntime::new(graph(), CompensationLimits::default()).unwrap();
    let actual = runtime.plan.storage_bytes().unwrap()
        + vector_bytes(&runtime.edges).unwrap()
        + runtime.stamps.storage_bytes().unwrap()
        + runtime
            .edges
            .iter()
            .map(DelayPair::storage_bytes)
            .sum::<usize>();
    assert_eq!(runtime.storage_bytes(), actual);
    assert!(
        CompensationRuntime::new(
            graph(),
            CompensationLimits {
                max_storage_bytes: actual,
                ..CompensationLimits::default()
            }
        )
        .is_ok()
    );
    assert!(matches!(
        CompensationRuntime::new(
            graph(),
            CompensationLimits {
                max_storage_bytes: actual - 1,
                ..CompensationLimits::default()
            }
        ),
        Err(CompensationError::StorageLimit)
    ));
}

#[test]
fn runtime_rejects_combined_ring_storage_exceeding_its_policy() {
    let mut nodes = vec![node(100, MAX_COMPENSATION_FRAMES), node(101, 0)];
    let mut edges = vec![edge(100, 100, 101)];
    for key in 0..14 {
        nodes.push(node(key, 0));
        edges.push(edge(key, key, 101));
    }
    assert!(matches!(
        CompensationRuntime::new(plan(&nodes, &edges), CompensationLimits::default()),
        Err(CompensationError::StorageLimit)
    ));
}

#[test]
fn bounded_history_copy_keeps_wrapped_full_origins_and_transaction_rollback() {
    for count in [2, 11] {
        let mut source = delay::RingPair::<Option<TransportStamp>>::new(4).unwrap();
        source.begin();
        for frame in 0..count {
            source.push(Some(TransportStamp {
                authored_clock_epoch: NonZeroU64::new(frame + 1),
                tempo_micro_bpm: Some(123_000_000),
                performance_frame: 900 + frame,
                transport_is_running: frame % 2 == 0,
                arrangement_frame: frame % 3,
                loop_pass: frame / 3,
                performance_beat_raw: (frame << 30) + 27,
                arrangement_beat_raw: (frame % 2 == 0).then_some(frame << 28),
                meter: Some(hex_project::Meter::COMMON_TIME),
                cycle_beats_raw: Some((0, 4 << 32)),
            }));
        }
        source.commit();
        let mut target = delay::RingPair::<Option<TransportStamp>>::new(7).unwrap();
        target.begin();
        for _ in 0..7 {
            target.push(Some(TransportStamp {
                authored_clock_epoch: None,
                tempo_micro_bpm: Some(123_000_000),
                performance_frame: 1,
                transport_is_running: false,
                arrangement_frame: 1,
                loop_pass: 0,
                performance_beat_raw: 0,
                arrangement_beat_raw: None,
                meter: None,
                cycle_beats_raw: None,
            }));
        }
        target.commit();
        let expected: Vec<_> = (0..7).map(|offset| source.at_delay(offset)).collect();
        target.copy_history_from(&source);
        for (offset, stamp) in expected.iter().enumerate() {
            assert_eq!(
                target.at_delay(offset),
                *stamp,
                "all origin fields and unavailable history"
            );
        }
        target.begin();
        for _ in 0..9 {
            target.push(None);
        }
        target.rollback();
        for (offset, stamp) in expected.iter().enumerate() {
            assert_eq!(target.at_delay(offset), *stamp);
            assert_eq!(source.at_delay(offset), *stamp, "copy cannot mutate donor");
        }
        target.reset();
        assert_eq!(target.at_delay(0), None);
        assert!(source.at_delay(0).unwrap().is_some());
    }
}
