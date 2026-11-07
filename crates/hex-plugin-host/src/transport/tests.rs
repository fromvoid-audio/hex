use super::*;

const BINDING: Binding = Binding {
    root: 1,
    instance: 2,
    graph: 3,
    epoch: 4,
};

#[test]
fn constructor_storage_requires_the_original_pair_and_survives_exchange_and_retirement() {
    let quote = channel_storage_bound().unwrap();
    let (control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
    let (other_control, other_callback, _) = channel(BINDING, 8).unwrap();
    assert_eq!(
        callback.constructor_storage_on_control(&other_control),
        None
    );
    assert_eq!(
        other_callback.constructor_storage_on_control(&control),
        None
    );
    let storage = callback.constructor_storage_on_control(&control).unwrap();
    assert_eq!(storage, quote);
    // Both queue allocations retain all slots, even when no blocks are queued.
    assert_eq!(
        storage.data_bytes,
        QUEUE_CAPACITY * (std::mem::size_of::<InputRequest>() + std::mem::size_of::<OutputReply>())
            + std::mem::size_of::<RingBuffer<InputRequest>>()
            + std::mem::size_of::<RingBuffer<OutputReply>>()
            + std::mem::size_of::<Revocation>()
    );
    assert_eq!((storage.shared_groups, storage.shared_handle_bound), (3, 7));
    for _ in 0..4 {
        callback
            .stage(&[[0.5; 2]; 8], &[], &mut [[0.0; 2]; 8])
            .unwrap();
        callback.finish(true).unwrap();
        echo(&mut worker);
        assert_eq!(
            callback.constructor_storage_on_control(&control),
            Some(storage)
        );
    }
    assert!(!callback.is_revoked());
    control.revoke();
    assert!(
        callback.is_revoked(),
        "control revocation is visible before another stage"
    );
    assert!(
        callback.fault().is_none(),
        "revocation is separate from a visited fault"
    );
    assert_eq!(control.take_callback_fault(), None);
    drop(worker);
    assert_eq!(
        callback.constructor_storage_on_control(&control),
        Some(storage)
    );
}

#[cfg(feature = "allocation-probe")]
#[test]
fn visiting_retained_channel_storage_neither_allocates_nor_reclaims() {
    let (control, callback, worker) = channel(BINDING, 8).unwrap();
    let expected = callback.constructor_storage_on_control(&control);
    let measured = allocation_counter::measure(|| {
        assert_eq!(channel_storage_bound(), expected);
        assert_eq!(callback.constructor_storage_on_control(&control), expected);
    });
    assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
    assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
    drop((control, callback, worker));
}

#[test]
fn minimum_and_maximum_quantum_keep_the_declared_two_quantum_delay() {
    for q in [1_u16, MAX_FRAMES as u16] {
        let (_control, mut callback, mut worker) = channel(BINDING, q).unwrap();
        let count = usize::from(q);
        let input = [[0.5, -1.0]; MAX_FRAMES];
        let mut output = [[9.0; 2]; MAX_FRAMES];
        for block in 0..4 {
            callback
                .stage(&input[..count], &[], &mut output[..count])
                .unwrap();
            callback.finish(true).unwrap();
            echo(&mut worker);
            assert_eq!(
                output[..count],
                vec![if block < 2 { [0.0; 2] } else { [0.25, -0.5] }; count]
            );
        }
        assert_eq!(callback.latency_frames(), u32::from(q) * 2);
    }
}

fn echo(worker: &mut Worker) {
    while let Some(mut block) = worker.receive().expect("live exchange") {
        for frame in &mut block.pcm[..usize::from(block.frames)] {
            frame[0] *= 0.5;
            frame[1] *= 0.5;
        }
        worker.complete(block).expect("matching reply");
    }
}

#[test]
fn sample_timeline_latency_and_midi_survive_physical_callback_partitions() {
    for partitions in [&[8][..], &[1, 3, 2, 7, 5][..], &[5, 8, 1, 2][..]] {
        let (control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
        let mut position = 0;
        let mut outputs = Vec::new();
        let mut output_events = Vec::new();
        for size in partitions.iter().copied().cycle().take(48) {
            let input: Vec<_> = (position..position + size)
                .map(|i| [i as f32 + 1.0, -1.0])
                .collect();
            let events: Vec<_> = (position..position + size)
                .enumerate()
                .filter(|(_, frame)| frame % 8 == 3)
                .map(|(offset, _)| MidiEvent {
                    frame: offset as u16,
                    bytes: [0x90, 60, 100],
                    len: 3,
                    note_id: None,
                })
                .collect();
            let mut output = vec![[99.0; 2]; size];
            let result = callback.stage(&input, &events, &mut output).unwrap();
            // Even a complete quantum is invisible until host conversion accepts.
            assert!(worker.receive().unwrap().is_none());
            callback.finish(true).unwrap();
            echo(&mut worker);
            output_events.extend(
                result
                    .as_slice()
                    .iter()
                    .map(|event| position + usize::from(event.frame)),
            );
            outputs.extend(output);
            position += size;
        }
        assert_eq!(callback.latency_frames(), 16);
        for (frame, output) in outputs.iter().enumerate() {
            let expected = if frame < 16 {
                [0.0; 2]
            } else {
                [(frame - 16) as f32 * 0.5 + 0.5, -0.5]
            };
            assert_eq!(*output, expected, "partition {partitions:?}, frame {frame}");
        }
        assert_eq!(output_events, (19..position).step_by(8).collect::<Vec<_>>());
        assert!(!control.is_revoked());
    }
}

#[test]
fn host_refusal_never_dispatches_and_late_output_cannot_resume_the_epoch() {
    let (control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
    let mut output = [[9.0; 2]; 8];
    callback.stage(&[[1.0; 2]; 8], &[], &mut output).unwrap();
    assert_eq!(callback.finish(false), Err(Fault::Protocol));
    assert!(worker.requests.is_empty());
    assert!(control.is_revoked());
    assert!(matches!(worker.receive(), Err(Fault::Revoked)));
    assert!(matches!(
        callback.stage(&[[1.0; 2]; 8], &[], &mut output),
        Err(Fault::Revoked)
    ));
    assert_eq!(output, [[0.0; 2]; 8]);
}

#[test]
fn missed_deadline_silences_and_revokes_without_replaying_last_output() {
    let (control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
    let mut output = [[0.0; 2]; 8];
    for _ in 0..2 {
        callback.stage(&[[1.0; 2]; 8], &[], &mut output).unwrap();
        callback.finish(true).unwrap();
    }
    let late = worker.receive().unwrap().unwrap();
    assert!(matches!(
        callback.stage(&[[1.0; 2]; 8], &[], &mut output),
        Err(Fault::MissingOutput)
    ));
    assert_eq!(output, [[0.0; 2]; 8]);
    assert!(control.is_revoked());
    assert_eq!(worker.complete(late), Err(Fault::Revoked));
    assert!(callback.replies.is_empty());
    callback.reject(Fault::Protocol);
    assert_eq!(control.take_callback_fault(), Some(Fault::MissingOutput));
    assert_eq!(
        control.deadline_miss(),
        Some(DeadlineMiss {
            reply_sequence: 0,
            worker_phase: DeadlineWorkerPhase::Processing,
        })
    );
    assert_eq!(control.take_callback_fault(), None);
    assert_eq!(callback.fault(), Some(Fault::MissingOutput));
}

#[test]
fn deadline_observation_distinguishes_a_worker_that_has_not_picked_up_the_request() {
    let (control, mut callback, _worker) = channel_from_completed_prefix(BINDING, 8, 37).unwrap();
    let mut output = [[0.0; 2]; 8];
    for _ in 0..2 {
        callback.stage(&[[1.0; 2]; 8], &[], &mut output).unwrap();
        callback.finish(true).unwrap();
    }
    assert_eq!(
        callback.stage(&[[1.0; 2]; 8], &[], &mut output).err(),
        Some(Fault::MissingOutput)
    );
    assert_eq!(
        control.deadline_miss(),
        Some(DeadlineMiss {
            reply_sequence: 37,
            worker_phase: DeadlineWorkerPhase::AwaitingPickup,
        })
    );
}

#[test]
fn native_instrument_keeps_ordered_input_and_recovers_from_one_late_reply() {
    let (control, mut callback, mut worker) =
        instrument_channel_from_completed_prefix(BINDING, 8, 37).unwrap();
    let mut output = [[0.0; 2]; 8];
    let counts = allocation_counter::measure(|| {
        for value in [1.0, 2.0, 3.0] {
            callback.stage(&[[value; 2]; 8], &[], &mut output).unwrap();
            callback.finish(true).unwrap();
            assert_eq!(output, [[0.0; 2]; 8]);
        }
        assert!(!control.is_revoked());
        for (sequence, value) in [(37, 1.0), (38, 2.0), (39, 3.0)] {
            let reply = worker.receive().unwrap().unwrap();
            assert_eq!(reply.sequence, sequence);
            assert_eq!(reply.pcm[0], [value; 2]);
            worker.complete(reply).unwrap();
        }
        for value in [2.0, 3.0] {
            callback.stage(&[[0.0; 2]; 8], &[], &mut output).unwrap();
            callback.finish(true).unwrap();
            assert_eq!(output, [[value; 2]; 8]);
        }
        assert!(!control.is_revoked());
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn native_instrument_revokes_after_two_consecutive_missed_replies() {
    let (control, mut callback, mut worker) =
        instrument_channel_from_completed_prefix(BINDING, 8, 0).unwrap();
    let mut output = [[0.0; 2]; 8];
    for _ in 0..3 {
        callback.stage(&[[0.0; 2]; 8], &[], &mut output).unwrap();
        callback.finish(true).unwrap();
    }
    // The stale reply does not turn a second missed deadline into a success.
    let late = worker.receive().unwrap().unwrap();
    worker.complete(late).unwrap();
    assert_eq!(
        callback.stage(&[[0.0; 2]; 8], &[], &mut output).err(),
        Some(Fault::MissingOutput)
    );
    assert!(control.is_revoked());
    assert_eq!(control.deadline_miss().unwrap().reply_sequence, 1);
}

#[test]
fn consumer_rejects_each_identity_field_and_nonfinite_or_invalid_events() {
    for case in 0..10 {
        let (_control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
        let mut output = [[0.0; 2]; 8];
        for _ in 0..2 {
            callback.stage(&[[1.0; 2]; 8], &[], &mut output).unwrap();
            callback.finish(true).unwrap();
        }
        let mut reply = worker.receive().unwrap().unwrap();
        match case {
            0 => reply.binding.root += 1,
            1 => reply.binding.instance += 1,
            2 => reply.binding.graph += 1,
            3 => reply.binding.epoch += 1,
            4 => reply.sequence += 1,
            5 => reply.first_frame += 1,
            6 => reply.frames -= 1,
            7 => reply.pcm[7][1] = f32::NAN,
            8 => reply
                .events
                .push(MidiEvent {
                    frame: 8,
                    bytes: [0x90, 60, 1],
                    len: 3,
                    note_id: None,
                })
                .unwrap(),
            9 => reply
                .events
                .push(MidiEvent {
                    frame: 0,
                    bytes: [0xf0, 0, 0],
                    len: 3,
                    note_id: None,
                })
                .unwrap(),
            _ => unreachable!(),
        }
        // Deliberately bypass the original producer to test consumer validation.
        worker
            .replies
            .push(OutputReply {
                block: reply,
                manual: Some(ManualExchange {
                    input: ManualParameterInput {
                        operation: NonZeroU64::MIN,
                        action: ManualParameterAction::Point(ParameterPoint {
                            id: 0,
                            frame: 0,
                            normalized: 0.5,
                        }),
                    },
                    origin: None,
                }),
            })
            .unwrap();
        let expected = match case {
            7 => Fault::InvalidSamples,
            8 | 9 => Fault::InvalidEvents,
            _ => Fault::Identity,
        };
        assert!(
            matches!(callback.stage(&[[1.0; 2]; 8], &[], &mut output), Err(fault) if fault == expected)
        );
        assert_eq!(output, [[0.0; 2]; 8]);
        assert_eq!(callback.take_accepted_manual_parameter(), None);
    }
}

#[test]
fn duplicate_and_out_of_order_replies_are_terminal() {
    for duplicate in [false, true] {
        let (_control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
        let mut output = [[0.0; 2]; 8];
        for _ in 0..2 {
            callback.stage(&[[1.0; 2]; 8], &[], &mut output).unwrap();
            callback.finish(true).unwrap();
        }
        let first = worker.receive().unwrap().unwrap();
        worker.complete(first).unwrap();
        if duplicate {
            worker
                .replies
                .push(OutputReply {
                    block: first,
                    manual: None,
                })
                .unwrap();
            callback.stage(&[[1.0; 2]; 8], &[], &mut output).unwrap();
            callback.finish(true).unwrap();
        } else {
            callback.replies.pop().unwrap();
            let second = worker.receive().unwrap().unwrap();
            worker.complete(second).unwrap();
        }
        assert!(matches!(
            callback.stage(&[[1.0; 2]; 8], &[], &mut output),
            Err(Fault::Identity)
        ));
        assert_eq!(output, [[0.0; 2]; 8]);
    }
}

#[test]
fn worker_enforces_single_inflight_and_exact_completion_before_next_receive() {
    for case in 0..3 {
        let (control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
        callback
            .stage(&[[1.0; 2]; 8], &[], &mut [[0.0; 2]; 8])
            .unwrap();
        callback.finish(true).unwrap();
        let request = worker.receive_request().unwrap().unwrap();
        assert!(!request.manual_origin_available);
        let mut block = request.block;
        match case {
            0 => assert!(matches!(worker.receive(), Err(Fault::Protocol))),
            1 => {
                block.binding.epoch += 1;
                assert_eq!(worker.complete(block), Err(Fault::Identity));
            }
            2 => {
                worker.complete(block).unwrap();
                assert_eq!(worker.complete(block), Err(Fault::Protocol));
            }
            _ => unreachable!(),
        }
        assert!(control.is_revoked());
    }
}

#[test]
fn bounded_input_validation_and_queue_independent_stop() {
    for case in 0..5 {
        let (control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
        let mut output = [[99.0; 2]; 8];
        let fault = match case {
            0 => callback
                .stage(&[[f32::INFINITY; 2]; 8], &[], &mut output)
                .err(),
            1 => callback.stage(&[[1.0; 2]; 7], &[], &mut output).err(),
            2 => callback
                .stage(
                    &[[1.0; 2]; 8],
                    &[MidiEvent {
                        frame: 0,
                        bytes: [0x90, 200, 0],
                        len: 3,
                        note_id: None,
                    }],
                    &mut output,
                )
                .err(),
            3 => {
                for _ in 0..QUEUE_CAPACITY {
                    callback
                        .requests
                        .push(InputRequest {
                            block: Block::empty(BINDING, 8),
                            parameter_capacity: MAX_PARAMETER_POINTS,
                            authored: false,
                            manual_origin: None,
                        })
                        .unwrap();
                }
                callback.stage(&[[1.0; 2]; 8], &[], &mut output).unwrap();
                callback.finish(true).err()
            }
            4 => {
                for _ in 0..QUEUE_CAPACITY {
                    callback
                        .requests
                        .push(InputRequest {
                            block: Block::empty(BINDING, 8),
                            parameter_capacity: MAX_PARAMETER_POINTS,
                            authored: false,
                            manual_origin: None,
                        })
                        .unwrap();
                }
                control.revoke();
                assert!(matches!(worker.receive(), Err(Fault::Revoked)));
                callback.stage(&[[1.0; 2]; 8], &[], &mut output).err()
            }
            _ => unreachable!(),
        };
        assert_eq!(
            fault,
            Some(
                [
                    Fault::InvalidSamples,
                    Fault::Protocol,
                    Fault::InvalidEvents,
                    Fault::Saturated,
                    Fault::Revoked
                ][case]
            )
        );
        assert!(control.is_revoked());
        assert_eq!(output, [[0.0; 2]; 8]);
        assert_eq!(
            control.take_callback_fault(),
            fault.filter(|&reason| reason != Fault::Revoked)
        );
    }
    for q in [0, 257, u16::MAX] {
        assert!(matches!(channel(BINDING, q), Err(Fault::Configuration)));
    }
    assert!(matches!(
        channel(
            Binding {
                epoch: 0,
                ..BINDING
            },
            8
        ),
        Err(Fault::Configuration)
    ));
}

#[cfg(feature = "allocation-probe")]
#[test]
fn endpoint_exchange_has_zero_total_and_signed_net_allocation() {
    for origin in [0, 1_u64 << 32] {
        let (control, mut callback, mut worker) =
            channel_from_completed_prefix(BINDING, 8, origin).unwrap();
        let input = [[0.5; 2]; 8];
        let mut output = [[0.0; 2]; 8];
        let info = allocation_counter::measure(|| {
            for _ in 0..32 {
                assert!(callback.stage(&input, &[], &mut output).is_ok());
                assert!(callback.finish(true).is_ok());
                echo(&mut worker);
            }
            control.revoke();
            assert!(matches!(
                callback.stage(&input, &[], &mut output),
                Err(Fault::Revoked)
            ));
        });
        assert_eq!(info.count_total, 0);
        assert_eq!(info.bytes_total, 0);
        assert_eq!(info.count_current, 0);
        assert_eq!(info.bytes_current, 0);
        // Endpoint and queue destruction deliberately lies outside the measured scope.
        drop((callback, worker, control));
    }
}

#[test]
fn fresh_continuation_channels_prime_two_quanta_at_the_actual_absolute_prefix() {
    for quantum in [1_u16, 8, MAX_FRAMES as u16] {
        for origin in [0, 3, u64::from(u32::MAX)] {
            let (_control, mut callback, mut worker) =
                channel_from_completed_prefix(BINDING, quantum, origin).unwrap();
            let q = usize::from(quantum);
            let input = [[0.5; 2]; MAX_FRAMES];
            let mut output = [[9.0; 2]; MAX_FRAMES];
            for n in 0..4 {
                callback.stage(&input[..q], &[], &mut output[..q]).unwrap();
                callback.finish(true).unwrap();
                let mut block = worker.receive().unwrap().unwrap();
                assert_eq!(block.sequence, origin + n);
                assert_eq!(block.first_frame, (origin + n) * u64::from(quantum));
                block.pcm[..q].fill([0.25; 2]);
                worker.complete(block).unwrap();
                assert_eq!(
                    output[..q],
                    vec![if n < 2 { [0.0; 2] } else { [0.25; 2] }; q]
                );
            }
        }
    }
    assert!(matches!(
        channel_from_completed_prefix(BINDING, 1, u64::MAX - 1),
        Err(Fault::Exhausted)
    ));
    assert!(matches!(
        channel_from_completed_prefix(BINDING, 2, u64::MAX / 2 + 1),
        Err(Fault::Exhausted)
    ));
}

#[test]
fn graph_segments_preserve_physical_pcm_event_timeline_and_one_final_dispatch() {
    let (_control, mut whole, mut whole_worker) = channel(BINDING, 8).unwrap();
    let (_control, mut split, mut split_worker) = channel(BINDING, 8).unwrap();
    let mut position = 0;
    for (size, parts) in [
        (8, &[3, 1, 4][..]),
        (5, &[1, 4]),
        (7, &[2, 3, 2]),
        (8, &[8]),
    ]
    .into_iter()
    .cycle()
    .take(12)
    {
        let input: Vec<_> = (position..position + size)
            .map(|n| [n as f32, -0.5])
            .collect();
        let event = MidiEvent {
            frame: (size - 1) as u16,
            bytes: [0x90, 61, 127],
            len: 3,
            note_id: None,
        };
        let mut expected = vec![[9.0; 2]; size];
        let whole_events = whole.stage(&input, &[event], &mut expected).unwrap();
        split.begin_physical_callback(size).unwrap();
        let mut output = vec![[9.0; 2]; size];
        let mut actual_events = Vec::new();
        let mut offset = 0;
        for &count in parts {
            let end = offset + count;
            let mut events = Vec::new();
            if usize::from(event.frame) >= offset && usize::from(event.frame) < end {
                events.push(MidiEvent {
                    frame: event.frame - offset as u16,
                    ..event
                });
            }
            let copied = split
                .stage_segment(
                    offset,
                    &input[offset..end],
                    &events,
                    &mut output[offset..end],
                )
                .unwrap();
            actual_events.extend(copied.as_slice().iter().map(|event| MidiEvent {
                frame: event.frame + offset as u16,
                ..*event
            }));
            assert!(
                split_worker.receive().unwrap().is_none(),
                "no intermediate graph segment can dispatch"
            );
            offset = end;
        }
        assert_eq!(output, expected);
        assert_eq!(actual_events, whole_events.as_slice());
        split.finish(true).unwrap();
        whole.finish(true).unwrap();
        echo(&mut split_worker);
        echo(&mut whole_worker);
        position += size;
    }
}

#[test]
fn incomplete_overlapping_gapped_or_refused_graph_callbacks_never_dispatch() {
    for fault in 0..6 {
        let (control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
        callback.begin_physical_callback(8).unwrap();
        callback
            .stage_segment(0, &[[0.5; 2]; 3], &[], &mut [[9.0; 2]; 3])
            .unwrap();
        let result = match fault {
            0 => callback.finish(true), // incomplete
            1 => callback.finish(false),
            2 => callback
                .stage_segment(2, &[[0.5; 2]; 5], &[], &mut [[9.0; 2]; 5])
                .map(|_| ()),
            3 => callback
                .stage_segment(4, &[[0.5; 2]; 4], &[], &mut [[9.0; 2]; 4])
                .map(|_| ()),
            4 => callback
                .stage_segment(3, &[[0.5; 2]; 6], &[], &mut [[9.0; 2]; 6])
                .map(|_| ()),
            _ => callback.begin_physical_callback(8),
        };
        assert_eq!(result, Err(Fault::Protocol));
        assert!(control.is_revoked());
        assert!(worker.requests.is_empty());
        assert!(matches!(worker.receive(), Err(Fault::Revoked)));
    }
    let (control, mut callback, worker) = channel(BINDING, 8).unwrap();
    callback.begin_physical_callback(8).unwrap();
    callback
        .stage_segment(0, &[[0.5; 2]; 3], &[], &mut [[9.0; 2]; 3])
        .unwrap();
    control.revoke();
    let mut output = [[9.0; 2]; 5];
    assert!(matches!(
        callback.stage_segment(3, &[[0.5; 2]; 5], &[], &mut output),
        Err(Fault::Revoked)
    ));
    assert_eq!(output, [[0.0; 2]; 5]);
    assert!(worker.requests.is_empty());
}

#[test]
fn pause_accepts_only_the_exact_staged_prefix_and_keeps_partial_input() {
    for pause_cut in [3, 4] {
        let (control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
        callback.begin_physical_callback(8).unwrap();
        callback
            .stage_segment(0, &[[0.25; 2]; 4], &[], &mut [[0.0; 2]; 4])
            .unwrap();
        callback.end_staging_at_pause(pause_cut);
        if pause_cut != 4 {
            assert_eq!(callback.finish(true), Err(Fault::Protocol));
            assert!(control.is_revoked());
            continue;
        }
        callback.finish(true).unwrap();
        assert!(worker.receive().unwrap().is_none());
        callback
            .stage(&[[0.5; 2]; 4], &[], &mut [[0.0; 2]; 4])
            .unwrap();
        callback.finish(true).unwrap();
        let input = worker.receive().unwrap().unwrap();
        assert_eq!(input.pcm[..4], [[0.25; 2]; 4]);
        assert_eq!(input.pcm[4..8], [[0.5; 2]; 4]);
    }
}

#[test]
fn segmenting_does_not_multiply_the_physical_callback_event_budget() {
    let (control, mut callback, worker) = channel(BINDING, 8).unwrap();
    let event = MidiEvent {
        frame: 0,
        bytes: [0x90, 60, 127],
        len: 3,
        note_id: None,
    };
    callback.begin_physical_callback(8).unwrap();
    callback
        .stage_segment(0, &[[0.5; 2]; 4], &[event; MAX_EVENTS], &mut [[0.0; 2]; 4])
        .unwrap();
    assert!(matches!(
        callback.stage_segment(4, &[[0.5; 2]; 4], &[event], &mut [[0.0; 2]; 4]),
        Err(Fault::InvalidEvents)
    ));
    assert!(control.is_revoked());
    assert!(worker.requests.is_empty());

    // Two legal earlier input callbacks can supply >64 events across the
    // boundary of two replies; splitting the later callback cannot evade its cap.
    let (_control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
    callback
        .stage(
            &[[0.5; 2]; 8],
            &[MidiEvent { frame: 4, ..event }; MAX_EVENTS],
            &mut [[0.0; 2]; 8],
        )
        .unwrap();
    callback.finish(true).unwrap();
    echo(&mut worker);
    callback
        .stage(&[[0.5; 2]; 8], &[event], &mut [[0.0; 2]; 8])
        .unwrap();
    callback.finish(true).unwrap();
    echo(&mut worker);
    callback
        .stage(&[[0.5; 2]; 4], &[], &mut [[0.0; 2]; 4])
        .unwrap();
    callback.finish(true).unwrap();
    echo(&mut worker);
    callback.begin_physical_callback(8).unwrap();
    let result = callback
        .stage_segment(0, &[[0.5; 2]; 4], &[], &mut [[0.0; 2]; 4])
        .unwrap();
    assert_eq!(result.as_slice().len(), MAX_EVENTS);
    assert!(matches!(
        callback.stage_segment(4, &[[0.5; 2]; 4], &[], &mut [[0.0; 2]; 4]),
        Err(Fault::InvalidEvents)
    ));
    assert!(worker.requests.is_empty());
}

#[test]
fn stopped_partial_quantum_dispatches_play_context_before_the_coincident_note() {
    let stopped = ProcessTiming {
        arrangement_frame: 0,
        playing: false,
        arrangement_beat_raw: Some(0),
        tempo_micro_bpm: None,
        meter: Some((7, 8)),
        cycle_beats_raw: None,
    };
    let playing = ProcessTiming {
        playing: true,
        ..stopped
    };
    let note = MidiEvent {
        frame: 0,
        bytes: [0x90, 60, 100],
        len: 3,
        note_id: std::num::NonZeroU32::new(42),
    };
    // Keep the pair's control lease alive across both callback transactions.
    let (_control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
    let paused = ProcessTiming {
        arrangement_frame: 2,
        ..stopped
    };
    let mut stage = || {
        callback.begin_physical_callback(3).unwrap();
        callback
            .stage_segment_with_timing(
                0,
                &[[0.0; 2]; 3],
                &[],
                &[],
                &mut [[0.0; 2]; 3],
                SegmentTiming {
                    playing: Some(false),
                    at: |_| Some(stopped),
                },
            )
            .unwrap();
        callback.finish(true).unwrap();
        assert!(worker.requests.is_empty());
        callback.begin_physical_callback(5).unwrap();
        callback
            .stage_segment_with_timing(
                0,
                &[[0.0; 2]; 2],
                &[note],
                &[],
                &mut [[0.0; 2]; 2],
                SegmentTiming {
                    playing: Some(true),
                    at: |_| Some(playing),
                },
            )
            .unwrap();
        callback
            .stage_segment_with_timing(
                2,
                &[[0.0; 2]; 3],
                &[],
                &[],
                &mut [[0.0; 2]; 3],
                SegmentTiming {
                    playing: Some(false),
                    at: |_| Some(paused),
                },
            )
            .unwrap();
        assert!(worker.requests.is_empty());
        callback.finish(true).unwrap();
    };
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(&mut stage);
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    stage();
    let request = worker.receive().unwrap().unwrap();
    assert_eq!(request.timing, Some(stopped));
    assert_eq!(request.events.as_slice(), &[MidiEvent { frame: 3, ..note }]);
    assert_eq!(
        request.timing_changes.as_slice(),
        &[
            TimingChange {
                frame: 3,
                timing: playing
            },
            TimingChange {
                frame: 5,
                timing: paused
            }
        ]
    );
    let mut packet = Vec::new();
    wire::write(&mut packet, &request).unwrap();
    assert_eq!(&packet[4..6], &6_u16.to_le_bytes());
    assert_eq!(packet[12], 2);
    let decoded = wire::read(&mut packet.as_slice()).unwrap().unwrap();
    assert_eq!(
        decoded.timing_changes.as_slice(),
        request.timing_changes.as_slice()
    );
    let first_change = wire::HEADER_BYTES + 8 * 8 + 12 + 40;
    for (offset, value) in [
        (12, 8),
        (first_change, 0),
        (first_change + 2, 1),
        (first_change + 28, 2),
    ] {
        let mut malformed = packet.clone();
        malformed[offset] = value;
        assert!(
            matches!(wire::read(&mut malformed.as_slice()), Err(error) if error.kind() == std::io::ErrorKind::InvalidData)
        );
    }
}

#[test]
fn dense_identified_notes_cross_callback_and_wire_without_expanding_replies() {
    let (control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
    let events: [MidiEvent; 160] = std::array::from_fn(|index| MidiEvent {
        frame: 1,
        bytes: [0x92, 60, 127],
        len: 3,
        note_id: std::num::NonZeroU32::new(index as u32 + 1),
    });
    let mut stage = || {
        callback.begin_physical_callback(8).unwrap();
        for (index, events) in events.chunks_exact(events.len() / 2).enumerate() {
            callback
                .stage_segment(index * 4, &[[0.0; 2]; 4], events, &mut [[0.0; 2]; 4])
                .unwrap();
        }
        callback.finish(true).unwrap();
    };
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(&mut stage);
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    stage();
    let request = worker.receive().unwrap().unwrap();
    assert_eq!(request.events.as_slice().len(), events.len());
    for (index, event) in request.events.as_slice().iter().enumerate() {
        assert_eq!(event.frame, if index < events.len() / 2 { 1 } else { 5 });
        assert_eq!(event.note_id, events[index].note_id);
    }
    let mut packet = Vec::new();
    wire::write(&mut packet, &request).unwrap();
    let decoded = wire::read(&mut packet.as_slice()).unwrap().unwrap();
    assert_eq!(decoded.events.as_slice(), request.events.as_slice());
    let first_note_id = wire::HEADER_BYTES + 8 * 8 + 8;
    packet[first_note_id..first_note_id + 4].fill(0);
    assert!(
        matches!(wire::read(&mut packet.as_slice()), Err(error) if error.kind() == std::io::ErrorKind::InvalidData)
    );
    assert_eq!(worker.complete(decoded), Err(Fault::InvalidEvents));
    assert!(control.is_revoked());

    // Even across accepted physical callbacks, one assembled quantum has a
    // finite note budget and cannot switch to legacy events above its cap.
    for (first, extra) in [
        (&events[..], events[0]),
        (
            &events[..64],
            MidiEvent {
                note_id: None,
                ..events[0]
            },
        ),
    ] {
        let (control, mut callback, worker) = channel(BINDING, 8).unwrap();
        callback
            .stage(&[[0.0; 2]; 4], first, &mut [[0.0; 2]; 4])
            .unwrap();
        callback.finish(true).unwrap();
        assert!(matches!(
            callback.stage(&[[0.0; 2]; 4], &[extra], &mut [[0.0; 2]; 4]),
            Err(Fault::InvalidEvents)
        ));
        assert!(control.is_revoked());
        assert!(worker.requests.is_empty());
    }
}

#[cfg(feature = "allocation-probe")]
#[test]
fn graph_fragment_staging_final_acceptance_and_refusal_allocate_and_free_nothing() {
    let (_control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
    let input = [[0.5; 2]; 8];
    let mut output = [[0.0; 2]; 8];
    for _ in 0..4 {
        let measured = allocation_counter::measure(|| {
            callback.begin_physical_callback(8).unwrap();
            callback
                .stage_segment(0, &input[..3], &[], &mut output[..3])
                .unwrap();
            callback
                .stage_segment(3, &input[3..], &[], &mut output[3..])
                .unwrap();
            callback.finish(true).unwrap();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        echo(&mut worker);
    }
    let measured = allocation_counter::measure(|| {
        callback.begin_physical_callback(8).unwrap();
        callback
            .stage_segment(0, &input[..3], &[], &mut output[..3])
            .unwrap();
        assert_eq!(callback.finish(false), Err(Fault::Protocol));
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(worker.requests.is_empty());
}

#[test]
fn manual_marker_requires_the_aligned_accepted_physical_callback() {
    for action in [
        ManualParameterAction::Point(ParameterPoint {
            id: 0,
            frame: 0,
            normalized: 0.75,
        }),
        ManualParameterAction::Release { parameter: 0 },
    ] {
        let input = ManualParameterInput {
            operation: NonZeroU64::MIN,
            action,
        };
        for host_accepts in [false, true] {
            let (control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
            let mut position = 0;
            let mut delivered = false;
            for size in [3, 4, 6, 2, 4, 5, 8] {
                let mut output = [[0.0; 2]; 8];
                callback.begin_physical_callback(size).unwrap();
                // The transport's first marked output is at position 16, which
                // falls in the second render segment of a physical callback.
                let split = size.min(1);
                let mut stage = |offset, input: &[Frame], output: &mut [Frame]| {
                    let origin = |index| {
                        let frame = (position + offset + index) as u64;
                        Some(ManualParameterOrigin {
                            arrangement_frame: 127 + frame,
                            performance_frame: 301 + frame,
                            loop_pass: 2,
                        })
                    };
                    callback
                        .stage_effect_segment(
                            offset,
                            input,
                            &[],
                            &[],
                            output,
                            SegmentTiming {
                                playing: None,
                                at: |_| None,
                            },
                            EffectSegmentOrigins {
                                authored: None,
                                manual: Some(&origin),
                            },
                        )
                        .unwrap();
                };
                stage(0, &[[1.0; 2]; 1][..split], &mut output[..split]);
                if split < size {
                    stage(split, &[[1.0; 2]; 8][split..size], &mut output[split..size]);
                }
                assert_eq!(callback.take_accepted_manual_parameter(), None);
                let includes_marker = (position..position + size).contains(&16);
                if includes_marker && !host_accepts {
                    assert_eq!(callback.finish(false), Err(Fault::Protocol));
                    assert_eq!(callback.take_accepted_manual_parameter(), None);
                    assert!(control.is_revoked());
                    break;
                }
                callback.finish(true).unwrap();
                let accepted = callback.take_accepted_manual_parameter();
                if includes_marker {
                    assert_eq!(
                        accepted,
                        Some(AcceptedManualParameter {
                            binding: BINDING,
                            sequence: 0,
                            first_frame: 0,
                            callback_offset: (16 - position) as u16,
                            input,
                            origin: Some(ManualParameterOrigin {
                                arrangement_frame: 127,
                                performance_frame: 301,
                                loop_pass: 2,
                            }),
                        })
                    );
                    assert_eq!(output[16 - position], [3.0; 2]);
                    delivered = true;
                } else {
                    assert_eq!(accepted, None);
                }
                assert_eq!(callback.take_accepted_manual_parameter(), None);
                if let Some(request) = worker.receive_request().unwrap() {
                    assert!(request.manual_origin_available);
                    let mut block = request.block;
                    let marker = (block.sequence == 0).then_some(input);
                    if marker.is_some() {
                        block.pcm[..8].fill([3.0; 2]);
                    }
                    worker
                        .complete_with_manual_parameter(block, marker)
                        .unwrap();
                }
                position += size;
            }
            assert_eq!(delivered, host_accepts);
        }
    }
}

#[test]
fn malformed_or_late_manual_marker_cannot_escape_its_reply() {
    let marker = ManualParameterInput {
        operation: NonZeroU64::MIN,
        action: ManualParameterAction::Point(ParameterPoint {
            id: 7,
            frame: 0,
            normalized: 0.5,
        }),
    };
    for action in [
        ManualParameterAction::Point(ParameterPoint {
            frame: 1,
            ..marker.point().unwrap()
        }),
        ManualParameterAction::Point(ParameterPoint {
            normalized: f64::NAN,
            ..marker.point().unwrap()
        }),
        ManualParameterAction::Release {
            parameter: u32::MAX,
        },
    ] {
        let (_control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
        callback
            .stage(&[[0.0; 2]; 8], &[], &mut [[0.0; 2]; 8])
            .unwrap();
        callback.finish(true).unwrap();
        let block = worker.receive().unwrap().unwrap();
        assert_eq!(
            worker.complete_with_manual_parameter(
                block,
                Some(ManualParameterInput { action, ..marker })
            ),
            Err(Fault::InvalidParameters)
        );
        assert!(callback.is_revoked());
        assert_eq!(callback.take_accepted_manual_parameter(), None);
    }
    let (_control, mut callback, mut worker) =
        instrument_channel_from_completed_prefix(BINDING, 8, 0).unwrap();
    let stage = |callback: &mut Callback| {
        let mut output = [[9.0; 2]; 8];
        callback.stage(&[[0.0; 2]; 8], &[], &mut output).unwrap();
        callback.finish(true).unwrap();
        assert_eq!(callback.take_accepted_manual_parameter(), None);
        assert_eq!(output, [[0.0; 2]; 8]);
    };
    stage(&mut callback);
    let late = worker.receive().unwrap().unwrap();
    stage(&mut callback);
    stage(&mut callback); // Exactly one concealed missing reply.
    worker
        .complete_with_manual_parameter(late, Some(marker))
        .unwrap();
    let on_time = worker.receive().unwrap().unwrap();
    worker.complete(on_time).unwrap();
    stage(&mut callback);
}
