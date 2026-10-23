use super::*;

const BINDING: Binding = Binding {
    root: 1,
    instance: 2,
    graph: 3,
    epoch: 4,
};

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
}

#[test]
fn consumer_rejects_each_identity_field_and_nonfinite_or_invalid_events() {
    for case in 0..10 {
        let (_, mut callback, mut worker) = channel(BINDING, 8).unwrap();
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
                })
                .unwrap(),
            9 => reply
                .events
                .push(MidiEvent {
                    frame: 0,
                    bytes: [0xf0, 0, 0],
                    len: 3,
                })
                .unwrap(),
            _ => unreachable!(),
        }
        // Deliberately bypass the original producer to test consumer validation.
        worker.replies.push(reply).unwrap();
        let expected = match case {
            7 => Fault::InvalidSamples,
            8 | 9 => Fault::InvalidEvents,
            _ => Fault::Identity,
        };
        assert!(
            matches!(callback.stage(&[[1.0; 2]; 8], &[], &mut output), Err(fault) if fault == expected)
        );
        assert_eq!(output, [[0.0; 2]; 8]);
    }
}

#[test]
fn duplicate_and_out_of_order_replies_are_terminal() {
    for duplicate in [false, true] {
        let (_, mut callback, mut worker) = channel(BINDING, 8).unwrap();
        let mut output = [[0.0; 2]; 8];
        for _ in 0..2 {
            callback.stage(&[[1.0; 2]; 8], &[], &mut output).unwrap();
            callback.finish(true).unwrap();
        }
        let first = worker.receive().unwrap().unwrap();
        worker.complete(first).unwrap();
        if duplicate {
            worker.replies.push(first).unwrap();
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
        let mut block = worker.receive().unwrap().unwrap();
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
                    }],
                    &mut output,
                )
                .err(),
            3 => {
                for _ in 0..QUEUE_CAPACITY {
                    callback.requests.push(Block::empty(BINDING, 8)).unwrap();
                }
                callback.stage(&[[1.0; 2]; 8], &[], &mut output).unwrap();
                callback.finish(true).err()
            }
            4 => {
                for _ in 0..QUEUE_CAPACITY {
                    callback.requests.push(Block::empty(BINDING, 8)).unwrap();
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
    let (control, mut callback, mut worker) = channel(BINDING, 8).unwrap();
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
