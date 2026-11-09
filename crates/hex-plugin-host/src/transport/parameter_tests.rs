//! Copied transport oracles only. Native SDK parameter application has separate
//! integration evidence; these tests do not claim that processing boundary.
use super::*;
const BINDING: Binding = Binding {
    root: 1,
    instance: 2,
    graph: 3,
    epoch: 4,
};

#[test]
fn dense_input_retains_each_sample_through_physical_and_timing_cuts() {
    let (_control, mut callback, mut worker) = channel(BINDING, MAX_FRAMES as u16).unwrap();
    let values: [f64; MAX_FRAMES] =
        std::array::from_fn(|index| if index % 2 == 0 { 0.125 } else { 0.75 });
    let cut = 96;
    let mut stage = || {
        for (start, end, playing) in [(0, cut, true), (cut, MAX_FRAMES, false)] {
            let frames = end - start;
            callback.begin_physical_callback(frames).unwrap();
            callback
                .stage_effect_segment(
                    0,
                    &[[0.25; 2]; MAX_FRAMES][..frames],
                    &[],
                    &[ParameterPoint {
                        frame: 3,
                        id: 11,
                        normalized: 0.5,
                    }],
                    &mut [[0.0; 2]; MAX_FRAMES][..frames],
                    SegmentTiming {
                        playing: Some(playing),
                        at: |_: usize| {
                            Some(ProcessTiming {
                                arrangement_frame: start as i64,
                                playing,
                                arrangement_beat_raw: None,
                                tempo_micro_bpm: None,
                                meter: None,
                                cycle_beats_raw: None,
                            })
                        },
                    },
                    EffectSegmentOrigins {
                        dense: Some(DenseParameterSegment {
                            id: 0,
                            normalized: &values[start..end],
                        }),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert!(worker.receive_request().unwrap().is_none());
            callback.finish(true).unwrap();
        }
    };
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(&mut stage);
        assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
        assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
    }
    #[cfg(not(feature = "allocation-probe"))]
    stage();
    let block = worker.receive_request().unwrap().unwrap().block;
    let lane = block.dense_parameter().unwrap();
    assert_eq!(lane.id, 0);
    assert_eq!(lane.normalized, &values);
    assert_eq!(block.timing_changes.as_slice()[0].frame, cut as u16);
    assert_eq!(
        block
            .parameters
            .as_slice()
            .iter()
            .map(|point| point.frame)
            .collect::<Vec<_>>(),
        [3, cut as u16 + 3]
    );
    let mut bytes = Vec::new();
    assert!(wire::write(&mut bytes, &block).is_err());
    assert!(bytes.is_empty());
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        wire::write_authored(&mut bytes, &block, None, &|_| Some(0)).unwrap();
        assert_eq!(&bytes[4..6], &0x8806_u16.to_le_bytes());
        let tail = &bytes[bytes.len() - (8 + MAX_FRAMES * 8)..];
        assert_eq!(&tail[..8], &[0; 8]);
        for (&value, encoded) in values.iter().zip(tail[8..].chunks_exact(8)) {
            assert_eq!(encoded, &value.to_le_bytes());
        }
        assert!(
            wire::read(&mut bytes.as_slice()).is_err(),
            "dense input is request-only"
        );
    }
}

#[test]
fn dense_input_refuses_a_changed_parameter_before_dispatch() {
    let (_control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
    for id in [7, 8] {
        callback.begin_physical_callback(4).unwrap();
        let staged = callback.stage_effect_segment(
            0,
            &[[0.25; 2]; 4],
            &[],
            &[],
            &mut [[0.0; 2]; 4],
            SegmentTiming {
                playing: None,
                at: |_| None,
            },
            EffectSegmentOrigins {
                dense: Some(DenseParameterSegment {
                    id,
                    normalized: &[0.5; 4],
                }),
                ..Default::default()
            },
        );
        if id == 7 {
            staged.unwrap();
            callback.finish(true).unwrap();
        } else {
            assert!(matches!(staged, Err(Fault::InvalidParameters)));
        }
        assert!(worker.requests.pop().is_err());
    }
    assert!(callback.is_revoked());
}

#[test]
fn control_input_capacity_follows_whole_physical_callback_across_quantum_and_segment_boundaries() {
    let (_control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
    callback
        .stage(&[[0.25; 2]; 3], &[], &mut [[0.0; 2]; 3])
        .unwrap();
    callback.finish(true).unwrap();
    assert!(worker.receive_with_parameter_capacity().unwrap().is_none());
    // This physical callback fills one old quantum and starts a new partial
    // one. Its64 points split63/1 across them. Counting only the completed
    // block would wrongly permit a65th physical-callback point.
    let first: Vec<_> = (1..=63)
        .map(|id| ParameterPoint {
            frame: 0,
            id,
            normalized: 0.25,
        })
        .collect();
    callback.begin_physical_callback(8).unwrap();
    callback
        .stage_segment_with_parameters(0, &[[0.25; 2]; 4], &[], &first, &mut [[0.0; 2]; 4])
        .unwrap();
    callback
        .stage_segment_with_parameters(
            4,
            &[[0.25; 2]; 4],
            &[],
            &[ParameterPoint {
                frame: 3,
                id: 64,
                normalized: 0.25,
            }],
            &mut [[0.0; 2]; 4],
        )
        .unwrap();
    assert!(
        worker.receive_with_parameter_capacity().unwrap().is_none(),
        "staging is not host acceptance"
    );
    callback.finish(true).unwrap();
    let (mut block, capacity) = worker.receive_with_parameter_capacity().unwrap().unwrap();
    assert_eq!(
        (block.sequence, block.parameters.as_slice().len(), capacity),
        (0, 63, 0)
    );
    assert!(
        block
            .parameters
            .as_slice()
            .iter()
            .all(|point| point.frame == 3)
    );
    block.parameters = Parameters::default();
    worker.complete(block).unwrap();
    callback
        .stage(&[[0.25; 2]; 5], &[], &mut [[0.0; 2]; 5])
        .unwrap();
    callback.finish(true).unwrap();
    let (mut block, capacity) = worker.receive_with_parameter_capacity().unwrap().unwrap();
    assert_eq!(
        (block.sequence, block.parameters.as_slice().len(), capacity),
        (1, 1, 0),
        "partial quantum retains its contributor's exhausted budget"
    );
    block.parameters = Parameters::default();
    worker.complete(block).unwrap();
    callback
        .stage(&[[0.25; 2]; 8], &[], &mut [[0.0; 2]; 8])
        .unwrap();
    callback.finish(true).unwrap();
    let (block, capacity) = worker.receive_with_parameter_capacity().unwrap().unwrap();
    assert_eq!(
        (block.sequence, block.parameters.as_slice().len(), capacity),
        (2, 0, 64),
        "fresh quantum gets fresh physical budget"
    );
    worker.complete(block).unwrap();
}

#[test]
fn variable_callbacks_and_segments_keep_parameter_pcm_and_feedback_at_exact_pipeline_offsets() {
    let (_control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
    let timeline = [
        (3, 0.25),
        (7, 0.75),
        (8, 0.5),
        (15, 1.0),
        (23, 0.0),
        (32, 0.625),
    ];
    let mut gain = 0.125_f64;
    let mut actual = Vec::new();
    let mut feedback = Vec::new();
    let mut first = 0;
    for length in [3, 5, 1, 7, 4, 4, 2, 6, 8, 3, 5, 8, 8] {
        callback.begin_physical_callback(length).unwrap();
        let split = length.min(1);
        for (offset, count) in [(0, split), (split, length - split)] {
            if count == 0 {
                continue;
            }
            let begin = first + offset;
            let points: Vec<_> = timeline
                .iter()
                .copied()
                .filter(|(frame, _)| (begin..begin + count).contains(frame))
                .map(|(frame, normalized)| ParameterPoint {
                    frame: (frame - begin) as u16,
                    id: 0,
                    normalized,
                })
                .collect();
            let mut output = [[9.0; 2]; 8];
            let reply = callback
                .stage_segment_with_parameters(
                    offset,
                    &[[1.0; 2]; 8][..count],
                    &[],
                    &points,
                    &mut output[..count],
                )
                .unwrap();
            actual.extend_from_slice(&output[..count]);
            feedback.extend(
                reply
                    .parameters
                    .as_slice()
                    .iter()
                    .map(|point| (begin + usize::from(point.frame), point.normalized)),
            );
        }
        callback.finish(true).unwrap();
        if let Some(mut block) = worker.receive().unwrap() {
            let input = block.parameters;
            block.parameters = Parameters::default();
            for frame in 0..usize::from(block.frames) {
                for point in input
                    .as_slice()
                    .iter()
                    .filter(|point| usize::from(point.frame) == frame)
                {
                    gain = point.normalized;
                    block
                        .parameters
                        .push(ParameterPoint { id: 1, ..*point })
                        .unwrap();
                }
                block.pcm[frame] = [gain as f32, -(gain as f32)];
            }
            worker.complete(block).unwrap();
        }
        first += length;
    }
    assert_eq!(first, 64);
    let expected: Vec<_> = (0..64)
        .map(|frame| {
            if frame < 16 {
                return [0.0; 2];
            }
            let value = timeline
                .iter()
                .rev()
                .find(|(at, _)| *at <= frame - 16)
                .map_or(0.125, |(_, value)| *value) as f32;
            [value, -value]
        })
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(
        feedback,
        timeline
            .iter()
            .map(|(frame, value)| (frame + 16, *value))
            .collect::<Vec<_>>()
    );
}

#[test]
fn parameter_overflow_and_final_host_refusal_never_publish_partial_input() {
    for host_refusal in [false, true] {
        let (_control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
        let points: Vec<_> = (0..MAX_PARAMETER_POINTS)
            .map(|id| ParameterPoint {
                frame: 0,
                id: id as u32,
                normalized: 0.5,
            })
            .collect();
        callback
            .begin_physical_callback(if host_refusal { 8 } else { 1 })
            .unwrap();
        callback
            .stage_segment_with_parameters(
                0,
                &[[1.0; 2]; 8][..if host_refusal { 8 } else { 1 }],
                &[],
                &points,
                &mut [[0.0; 2]; 8][..if host_refusal { 8 } else { 1 }],
            )
            .unwrap();
        if host_refusal {
            assert_eq!(callback.finish(false), Err(Fault::Protocol));
        } else {
            callback.finish(true).unwrap();
            callback.begin_physical_callback(1).unwrap();
            assert!(matches!(
                callback.stage_segment_with_parameters(
                    0,
                    &[[1.0; 2]],
                    &[],
                    &[ParameterPoint {
                        frame: 0,
                        id: 64,
                        normalized: 1.0
                    }],
                    &mut [[0.0; 2]]
                ),
                Err(Fault::InvalidParameters)
            ));
        }
        assert!(callback.is_revoked());
        assert!(worker.requests.pop().is_err());
        assert!(matches!(worker.receive(), Err(Fault::Revoked)));
    }
}

#[cfg(feature = "allocation-probe")]
#[test]
fn parameter_exchange_keeps_zero_total_and_signed_net_allocations() {
    let (control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
    let points = [ParameterPoint {
        frame: 7,
        id: 0,
        normalized: 0.5,
    }];
    let measured = allocation_counter::measure(|| {
        for _ in 0..32 {
            callback.begin_physical_callback(8).unwrap();
            callback
                .stage_segment_with_parameters(0, &[[1.0; 2]; 8], &[], &points, &mut [[0.0; 2]; 8])
                .unwrap();
            callback.finish(true).unwrap();
            let reply = worker.receive().unwrap().unwrap();
            worker.complete(reply).unwrap();
        }
        control.revoke();
    });
    assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
    assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
    drop((control, callback, worker));
}
