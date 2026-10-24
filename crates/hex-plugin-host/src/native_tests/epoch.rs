//! Actual loaded-source epoch and fresh-channel controls.
use super::*;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_same_child_quiescence_preserves_dsp_controller_capture_and_cumulative_pcm() {
    let case = Case::new();
    let selected = source();
    let saved = startup_state(&selected, true, 3);
    let setup = case.setup_restore(37);
    let events = case.root.join("module-events");
    let event_path = events.clone();
    let mut instance = Pending::prepare_with_restore(
        &build(),
        selected,
        options().preparation,
        Some(saved.clone()),
        PreparationCancellation::new_on_control().unwrap(),
        move |command| {
            setup(command);
            command.env("HEX_ORIGINAL_INSTANCE_MODULE_EVENTS", event_path);
        },
    )
    .unwrap()
    .complete_on_control(Some(options().presentation))
    .unwrap();
    let child = instance.child_id_on_control();
    let captured = instance.ready().initial_state.clone();
    let mut generated = vec![[0.0; 2]; 3];
    let mut actual = Vec::new();
    for epoch in 0..3_u64 {
        for n in epoch * 2..epoch * 2 + 2 {
            let block = request(instance.configuration(), n);
            generated.extend(block.pcm[..8].iter().map(|frame| {
                let gain = (0.5 + n as f32 * 0.125) * 0.75;
                [frame[0] * gain, frame[1] * gain]
            }));
            actual.extend_from_slice(&instance.exchange_on_worker(block).unwrap().pcm[..8]);
        }
        let stopped = instance.quiesce_on_control().unwrap();
        assert_eq!(
            fs::read(&events).unwrap(),
            b"E",
            "no module unload/reload between epochs"
        );
        assert_eq!(stopped.completed_blocks(), (epoch + 1) * 2);
        assert_eq!(stopped.child_id_on_control(), child); // diagnostic, not custody proof
        assert_eq!(stopped.facts().algorithmic_latency_frames, 3);
        assert!(stopped.facts().restored && stopped.facts().presentation_available);
        let observed = case.observation().unwrap();
        assert_eq!(&observed[1..4], &[0, 1, 1]); // no SDK lane violation/recreation/reinitialization
        assert_eq!(observed[7], 2); // same two bus activations; no reconfiguration
        assert_eq!(observed[8], epoch as i32 + 1);
        assert_eq!(observed[9], epoch as i32 + 1);
        assert_eq!(observed[10], ((epoch + 1) * 2) as i32);
        assert_eq!(&case.restore_observation()[..3], &[1, 1, 1]);
        assert_eq!(&case.restore_observation()[10..13], &[1, 1, 1]); // no recapture/recreated controller
        let previous = stopped.configuration().binding;
        let next = Binding {
            graph: previous.graph + 1,
            epoch: previous.epoch + 1,
            ..previous
        };
        instance = stopped
            .resume_on_control(next, Some(options().presentation))
            .unwrap();
        assert_eq!(instance.child_id_on_control(), child);
        assert_eq!(instance.configuration().binding, next);
        assert_eq!(instance.configuration().restore.as_ref(), Some(&saved));
        assert_eq!(
            instance.ready().initial_state,
            captured,
            "startup capture remains historical"
        );
    }
    assert_eq!(
        actual,
        generated[..48],
        "actual DSP history and absolute PCM sequence survive epochs"
    );
    let terminal = instance
        .stop_and_reap_on_control()
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (6, 48));
    let observed = case.observation().unwrap();
    assert_eq!(&observed[1..5], &[0, 1, 1, 1]);
    assert_eq!((observed[8], observed[9]), (4, 4));
    assert_eq!(&case.restore_observation()[10..15], &[1, 1, 1, 1, 1]);
    assert_eq!(fs::read(&events).unwrap(), b"EX");
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_same_instance_fresh_channels_discard_old_replies_and_keep_native_prefix() {
    use crate::transport as plugin_transport;
    let case = Case::new();
    let selected = source();
    let saved = startup_state(&selected, true, 3);
    let mut instance = Pending::prepare_with_restore(
        &build(),
        selected,
        options().preparation,
        Some(saved),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup_restore(37),
    )
    .unwrap()
    .complete_on_control(Some(options().presentation))
    .unwrap();
    let mut origin = 0_u64;
    let mut oracle = vec![[0.0; 2]; 3];
    for epoch in 0..2 {
        let (control, mut callback, mut worker) = plugin_transport::channel_from_completed_prefix(
            instance.configuration().binding,
            8,
            origin,
        )
        .unwrap();
        for n in 0..6_u64 {
            let input = request(instance.configuration(), origin + n);
            let mut output = [[9.0; 2]; 8];
            callback.stage(&input.pcm[..8], &[], &mut output).unwrap();
            callback.finish(true).unwrap();
            let block = worker.receive().unwrap().unwrap();
            assert_eq!(block.sequence, origin + n);
            let gain = (0.5 + (origin + n) as f32 * 0.125) * 0.75;
            oracle.extend(
                input.pcm[..8]
                    .iter()
                    .map(|frame| [frame[0] * gain, frame[1] * gain]),
            );
            worker
                .complete(instance.exchange_on_worker(block).unwrap())
                .unwrap();
            let expected = if n < 2 {
                [[0.0; 2]; 8]
            } else {
                oracle[((origin + n - 2) * 8) as usize..((origin + n - 1) * 8) as usize]
                    .try_into()
                    .unwrap()
            };
            assert_eq!(
                output,
                expected,
                "epoch{epoch}, cumulative prefix{}",
                origin + n
            );
        }
        control.revoke();
        // Includes two real completed replies still in the old output queue.
        // Dispose exact old carriers on control before making a fresh channel.
        drop((callback, worker, control));
        let stopped = instance.quiesce_on_control().unwrap();
        origin = stopped.completed_blocks();
        let previous = stopped.configuration().binding;
        instance = stopped
            .resume_on_control(
                Binding {
                    epoch: previous.epoch + 1,
                    ..previous
                },
                Some(options().presentation),
            )
            .unwrap();
    }
    let terminal = instance
        .stop_and_reap_on_control()
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (12, 96));
    assert_eq!(&case.restore_observation()[10..13], &[1, 1, 1]);
}

fn epoch_refusal<T>(result: Result<T>, expected: Option<u32>) {
    match result {
        Err(Error::Preparation {
            cause,
            exit,
            custody_error,
        }) => {
            assert!(custody_error.is_none());
            assert!(
                !exit.unwrap().success(),
                "exact failed child was directly reaped"
            );
            if let Some(expected) = expected {
                assert!(
                    matches!(*cause, Error::Worker(code) if code==expected),
                    "{cause:?}"
                );
            }
        }
        _ => panic!("expected terminal epoch refusal with direct child custody"),
    }
}
fn another_instance() -> Prepared {
    let mut other = options();
    other.preparation.binding.instance = 5;
    Prepared::prepare_on_control(
        &build(),
        source(),
        other,
        PreparationCancellation::new_on_control().unwrap(),
        TrustUserCode::Acknowledged,
    )
    .unwrap()
}
fn unchanged_other(instance: &mut Prepared, sequence: u64) {
    let input = request(instance.configuration(), sequence);
    let output = instance.exchange_on_worker(input).unwrap();
    for (actual, input) in output.pcm[..8].iter().zip(input.pcm) {
        let gain = 0.5 + sequence as f32 * 0.125;
        assert_eq!(*actual, [input[0] * gain, input[1] * gain]);
    }
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_epoch_sdk_refusal_exception_crash_and_changed_latency_never_resume_or_replay() {
    let mut other = another_instance();
    for (index, (variant, resuming, reason)) in [
        (58, false, Some(15)),
        (59, false, Some(12)),
        (60, false, None),
        (62, true, Some(16)),
        (63, true, Some(12)),
        (64, true, None),
        (68, true, Some(5)),
    ]
    .into_iter()
    .enumerate()
    {
        let case = Case::new();
        let instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(variant),
        )
        .unwrap();
        if resuming {
            let stopped = instance.quiesce_on_control().unwrap();
            let previous = stopped.configuration().binding;
            epoch_refusal(
                stopped.resume_on_control(
                    Binding {
                        epoch: previous.epoch + 1,
                        ..previous
                    },
                    Some(options().presentation),
                ),
                reason,
            );
        } else {
            epoch_refusal(instance.quiesce_on_control(), reason);
        }
        let observed = case.observation().unwrap();
        assert_eq!(
            observed[10], 0,
            "no foreign DSP before a matching resumed ACK"
        );
        assert_eq!(observed[2], 1, "no replacement native component");
        unchanged_other(&mut other, index as u64);
    }
    assert!(other.stop_and_reap_on_control().unwrap().status.success());
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_epoch_held_sdk_lanes_cancel_or_timeout_without_release_and_other_pcm_survives() {
    let mut other = another_instance();
    let mut sequence = 0;
    for (variant, stage, resuming) in [
        (61, 20, false),
        (66, 21, false),
        (67, 22, true),
        (65, 23, true),
    ] {
        for cancel in [false, true] {
            let case = Case::new();
            let context = PreparationCancellation::new_on_control().unwrap();
            let token = context.token_on_control();
            let instance =
                Prepared::prepare(&build(), source(), options(), context, case.setup(variant))
                    .unwrap();
            let result = if resuming {
                let stopped = instance.quiesce_on_control().unwrap();
                let previous = stopped.configuration().binding;
                case.after_stage(
                    stage,
                    move || {
                        stopped
                            .resume_on_control(
                                Binding {
                                    epoch: previous.epoch + 1,
                                    ..previous
                                },
                                Some(options().presentation),
                            )
                            .map(|_| ())
                    },
                    || {
                        if cancel {
                            token.cancel_on_control();
                        }
                    },
                )
            } else {
                case.after_stage(
                    stage,
                    move || instance.quiesce_on_control().map(|_| ()),
                    || {
                        if cancel {
                            token.cancel_on_control();
                        }
                    },
                )
            };
            match result {
                Err(Error::Preparation {
                    cause,
                    exit,
                    custody_error,
                }) => {
                    assert!(custody_error.is_none());
                    assert_eq!(exit.unwrap().code(), Some(9));
                    assert!(
                        matches!(*cause, Error::Cancelled) && cancel
                            || matches!(*cause, Error::Timeout) && !cancel,
                        "{cause:?}"
                    );
                }
                _ => panic!("held epoch must not resume"),
            }
            assert!(!case.release().exists());
            let observed = case.observation().unwrap();
            assert_eq!(observed[0], stage);
            assert_eq!(observed[4], 0);
            assert_eq!(observed[10], 0);
            unchanged_other(&mut other, sequence);
            sequence += 1;
        }
    }
    assert!(other.stop_and_reap_on_control().unwrap().status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_quiescent_stale_cancelled_dropped_and_retired_owners_never_restart() {
    for terminal in [
        "stale", "root", "instance", "graph", "unknown", "cancel", "drop", "retired", "faulted",
    ] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(0),
        )
        .unwrap();
        let reap = Arc::new(std::sync::atomic::AtomicI32::new(i32::MIN));
        instance.lease.reap_probe = Some(Arc::clone(&reap));
        if terminal == "retired" {
            assert!(
                instance
                    .stop_and_reap_on_control()
                    .unwrap()
                    .status
                    .success()
            );
            assert!(instance.quiesce_on_control().is_err());
            assert_eq!(reap.load(Ordering::Acquire), 0);
        } else if terminal == "faulted" {
            let wrong = request(instance.configuration(), 1);
            assert!(instance.exchange_on_worker(wrong).is_err());
            assert!(instance.quiesce_on_control().is_err());
            assert_eq!(reap.load(Ordering::Acquire), 9);
        } else {
            let stopped = instance.quiesce_on_control().unwrap();
            let previous = stopped.configuration().binding;
            let mut next = Binding {
                epoch: previous.epoch + 1,
                ..previous
            };
            if terminal == "drop" {
                drop(stopped);
            } else {
                match terminal {
                    "stale" => next = previous,
                    "root" => next.root += 1,
                    "instance" => next.instance += 1,
                    "graph" => next.graph -= 1,
                    "cancel" => stopped.cancellation_on_control().cancel_on_control(),
                    _ => {}
                }
                assert!(
                    stopped
                        .resume_on_control(
                            next,
                            if terminal == "unknown" {
                                None
                            } else {
                                Some(options().presentation)
                            }
                        )
                        .is_err()
                );
            }
            assert_eq!(
                reap.load(Ordering::Acquire),
                9,
                "actual Drop/reap, no PID lookup"
            );
        }
        let observed = case.observation().unwrap();
        if terminal == "faulted" {
            // Local PCM validation refused before further helper I/O. Its last
            // fixture snapshot is the initial latency query, before first start.
            assert_eq!((observed[0], observed[8]), (15, 0));
        } else {
            assert_eq!(observed[8], 1, "no new processing start for {terminal}");
        }
        assert_eq!(observed[10], 0, "no PCM replay for {terminal}");
    }
}
