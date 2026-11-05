//! Actual loaded-source epoch and fresh-channel controls.
use super::*;

// The fixture confirms the SDK is held before checking cancellation or timeout.
const HELD_OPERATION_TIMEOUT: Duration = Duration::from_millis(20);

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn explicit_quiescent_reactivation_refreshes_latency_without_recreating_or_restoring() {
    let case = Case::new();
    let selected = source();
    let saved = startup_state(&selected, true, 3);
    let mut instance = Pending::prepare_with_restore(
        &build(),
        selected,
        options().preparation,
        Some(saved.clone()),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup_restore(99),
    )
    .unwrap()
    .complete_on_control(Some(options().presentation))
    .unwrap();
    let child = instance.child_id_on_control();
    let historical = instance.ready().initial_state.clone();
    for n in 0..2 {
        let block = request(instance.configuration(), n);
        instance.exchange_on_worker(block).unwrap();
    }
    let stopped = instance.quiesce_retained_on_control().unwrap();
    assert_eq!(stopped.facts().algorithmic_latency_frames, 3);
    let before = case.observation().unwrap();
    let stopped = stopped.reactivate_retained_on_control().unwrap();
    assert_eq!(stopped.facts().algorithmic_latency_frames, 7);
    let stopped = stopped.reactivate_retained_on_control().unwrap();
    assert_eq!(stopped.facts().algorithmic_latency_frames, 7);
    assert_eq!(stopped.child_id_on_control(), child);
    assert_eq!(stopped.completed_blocks(), 2);
    let observed = case.observation().unwrap();
    assert_eq!(
        &observed[1..11],
        &before[1..11],
        "no construction, setup, buses or processing on reactivation"
    );
    let previous = stopped.configuration().binding;
    let mut instance = stopped
        .resume_retained_on_control(
            Binding {
                epoch: previous.epoch + 1,
                ..previous
            },
            Some(options().presentation),
        )
        .unwrap();
    assert_eq!(instance.child_id_on_control(), child);
    assert_eq!(instance.ready().algorithmic_latency_frames, 7);
    assert_eq!(instance.ready().initial_state, historical);
    assert_eq!(instance.configuration().restore.as_ref(), Some(&saved));
    let block = request(instance.configuration(), 2);
    let output = instance.exchange_on_worker(block).unwrap();
    assert_eq!(&output.pcm[..7], &[[0.0; 2]; 7]);
    let gain = (0.5 + 2.0 * 0.125) * 0.75;
    assert_eq!(
        output.pcm[7],
        [block.pcm[0][0] * gain, block.pcm[0][1] * gain]
    );
    let terminal = instance
        .stop_and_reap_on_control()
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (3, 24));
    assert_eq!(&case.restore_observation()[..3], &[1, 1, 1]);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn quiescent_reactivation_refusal_retains_the_original_failed_lease() {
    for variant in [100, 101, 102, 103] {
        let case = Case::new();
        let events = case.root.join("module-events");
        let event_path = events.clone();
        let setup = case.setup(variant);
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            move |command| {
                setup(command);
                command.env("HEX_ORIGINAL_INSTANCE_MODULE_EVENTS", event_path);
            },
        )
        .unwrap();
        let probe = epoch_probe(&mut instance);
        let stopped = instance.quiesce_retained_on_control().unwrap();
        epoch_refusal(
            stopped.reactivate_retained_on_control(),
            Some(if variant < 102 { 15 } else { 12 }),
            &probe,
        );
        let observed = case.observation().unwrap();
        assert_eq!((observed[2], observed[8], observed[9]), (1, 1, 1));
        assert_eq!(
            observed[4], 0,
            "no SDK termination after unknown activation effects"
        );
        assert_eq!(
            observed[0],
            if variant < 102 { 24 } else { 25 },
            "no later teardown/retry"
        );
        assert_eq!(
            fs::read(&events).unwrap(),
            if variant % 2 == 0 {
                b"ED".as_slice()
            } else {
                b"EDA".as_slice()
            },
            "no activation retry, release or module unload after refusal/exception"
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn reactivation_rejects_live_stale_prefix_and_skipped_operation_requests() {
    for mode in 0..3 {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(0),
        )
        .unwrap();
        let probe = epoch_probe(&mut instance);
        if mode != 0 {
            instance = instance.quiesce_retained_on_control().unwrap().instance;
        }
        let mut request =
            protocol::reactivate(instance.configuration(), 0, if mode == 2 { 2 } else { 1 })
                .unwrap();
        if mode == 1 {
            request[56..64].copy_from_slice(&1_u64.to_le_bytes());
        }
        let until = end_after(options().preparation.limits.startup).unwrap();
        instance
            .lease
            .channel
            .send(&request, until, &mut || Ok(()))
            .unwrap();
        instance
            .lease
            .channel
            .read(&mut instance.packet, until, &mut || Ok(()))
            .unwrap();
        assert_eq!(instance.packet.magic(), b"HXVIER01");
        assert_eq!(
            protocol::error(instance.packet.body().unwrap(), instance.config.binding).unwrap(),
            8
        );
        let failure = PreparationFailure::native(
            Error::Worker(8),
            instance.lease,
            instance.limits.retirement,
        );
        epoch_refusal::<()>(Err(failure), Some(8), &probe);
        if mode != 0 {
            assert_eq!(case.observation().unwrap()[8], 1, "no resumed processor");
        }
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_current_capture_uses_same_quiescent_instance_and_keeps_startup_historical() {
    let case = Case::new();
    let mut instance = Pending::prepare(
        &build(),
        source(),
        options().preparation,
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(0),
    )
    .unwrap()
    .complete_on_control(Some(options().presentation))
    .unwrap();
    let child = instance.child_id_on_control();
    let initial = instance.ready().initial_state.clone();
    for n in 0..2 {
        let block = request(instance.configuration(), n);
        instance.exchange_on_worker(block).unwrap();
    }
    let stopped = instance.quiesce_retained_on_control().unwrap();
    let (stopped, captured) = stopped.capture_state_retained_on_control().unwrap();
    assert_eq!(stopped.child_id_on_control(), child);
    assert_eq!(stopped.completed_blocks(), 2);
    let old = hex_plugin_state::decode(&initial).unwrap();
    let current = hex_plugin_state::decode(&captured).unwrap();
    assert_eq!(old.identity, current.identity);
    assert_eq!(&old.component.unwrap()[4..8], &0_u32.to_le_bytes());
    assert_eq!(&current.component.unwrap()[4..8], &2_u32.to_le_bytes());
    let (stopped, repeated) = stopped.capture_state_retained_on_control().unwrap();
    assert_eq!(repeated, captured);
    let previous = stopped.configuration().binding;
    let mut instance = stopped
        .resume_retained_on_control(
            Binding {
                epoch: previous.epoch + 1,
                ..previous
            },
            Some(options().presentation),
        )
        .unwrap();
    assert_eq!(instance.ready().initial_state, initial);
    assert_eq!(instance.child_id_on_control(), child);
    let block = request(instance.configuration(), 2);
    instance.exchange_on_worker(block).unwrap();
    let terminal = instance
        .stop_and_reap_on_control()
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (3, 24));
    let observed = case.observation().unwrap();
    assert_eq!(&observed[1..4], &[0, 1, 1]);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_current_capture_preserves_independent_controller_and_failed_capture_custody() {
    let case = Case::new();
    let selected = source();
    let saved = startup_state(&selected, true, 3);
    let mut instance = Pending::prepare_with_restore(
        &build(),
        selected,
        options().preparation,
        Some(saved.clone()),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup_restore(37),
    )
    .unwrap()
    .complete_on_control(Some(options().presentation))
    .unwrap();
    let initial = instance.ready().initial_state.clone();
    let block = request(instance.configuration(), 0);
    instance.exchange_on_worker(block).unwrap();
    let stopped = instance.quiesce_retained_on_control().unwrap();
    let (stopped, captured) = stopped.capture_state_retained_on_control().unwrap();
    let state = decode(&captured).unwrap();
    assert_eq!(&state.component.unwrap()[8..12], &0.75_f32.to_le_bytes());
    assert_eq!(&state.controller.unwrap()[4..8], &91_u32.to_le_bytes());
    let trace = case.restore_observation();
    assert_eq!(
        &trace[..3],
        &[1, 1, 1],
        "capture must not restore or synchronize the controller again"
    );
    assert_eq!(&trace[10..13], &[2, 2, 1]);
    let independent = Case::new();
    let mut restored = Pending::prepare_with_restore(
        &build(),
        source(),
        options().preparation,
        Some(captured.clone()),
        PreparationCancellation::new_on_control().unwrap(),
        independent.setup_restore(37),
    )
    .unwrap()
    .complete_on_control(Some(options().presentation))
    .unwrap();
    let input = request(restored.configuration(), 0);
    let actual = restored.exchange_on_worker(input).unwrap();
    assert_eq!(&actual.pcm[..3], &[[0.0; 2]; 3]);
    for (index, frame) in actual.pcm[..8].iter().enumerate().skip(3) {
        assert_eq!(*frame, input.pcm[index - 3].map(|value| value * 0.75 * 0.5));
    }
    assert_eq!(independent.restore_observation()[7], 91);
    assert!(
        restored
            .stop_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );
    let previous = stopped.configuration().binding;
    let mut instance = stopped
        .resume_on_control(
            Binding {
                epoch: previous.epoch + 1,
                ..previous
            },
            Some(options().presentation),
        )
        .unwrap();
    assert_eq!(instance.ready().initial_state, initial);
    assert_eq!(instance.configuration().restore, Some(saved));
    assert!(
        instance
            .stop_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );

    for (variant, reason) in [(69, 6), (70, 12)] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(variant),
        )
        .unwrap();
        let confirmed = instance.ready().initial_state.clone();
        let probe = epoch_probe(&mut instance);
        let stopped = instance.quiesce_retained_on_control().unwrap();
        let result = stopped.capture_state_retained_on_control();
        epoch_refusal(result, Some(reason), &probe);
        assert!(
            decode(&confirmed).unwrap().component.is_some(),
            "retain last confirmed bytes on refusal"
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_current_capture_held_sdk_cancel_and_timeout_retain_original_lease() {
    // Preparation still revalidates the source and helper for every child.
    let fixture_build = build();
    let fixture_source = source();
    for cancel in [true, false] {
        let case = Case::new();
        let context = PreparationCancellation::new_on_control().unwrap();
        let token = context.token_on_control();
        let mut instance = Prepared::prepare(
            &fixture_build,
            fixture_source.clone(),
            options(),
            context,
            case.setup(71),
        )
        .unwrap();
        // Keep normal preparation time, then bound the deliberately held SDK call.
        instance.limits.startup = HELD_OPERATION_TIMEOUT;
        let probe = epoch_probe(&mut instance);
        let stopped = instance.quiesce_retained_on_control().unwrap();
        let result = case.after_stage(
            22,
            move || stopped.capture_state_retained_on_control().map(|_| ()),
            || {
                if cancel {
                    token.cancel_on_control();
                }
            },
        );
        let failure = retained_epoch_failure(result, &probe);
        assert!(
            matches!(failure.cause(), Error::Cancelled) && cancel
                || matches!(failure.cause(), Error::Timeout) && !cancel
        );
        assert_eq!(
            dispose_epoch_failure(failure, &probe).status.code(),
            Some(9)
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_current_capture_held_sdk_observes_caller_intent_cancellation() {
    use std::sync::atomic::AtomicBool;

    let case = Case::new();
    let context = PreparationCancellation::new_on_control().unwrap();
    let mut instance =
        Prepared::prepare(&build(), source(), options(), context, case.setup(71)).unwrap();
    let initial = instance.ready().initial_state.clone();
    let probe = epoch_probe(&mut instance);
    let stopped = instance.quiesce_retained_on_control().unwrap();
    let cancelled = std::sync::Arc::new(AtomicBool::new(false));
    let capture_cancelled = std::sync::Arc::clone(&cancelled);
    let result = case.after_stage(
        22,
        move || {
            stopped
                .capture_state_retained_while_on_control(|| {
                    capture_cancelled.load(AtomicOrdering::Acquire)
                })
                .map(|_| ())
        },
        || cancelled.store(true, AtomicOrdering::Release),
    );
    let failure = retained_epoch_failure(result, &probe);
    assert!(matches!(failure.cause(), Error::Cancelled));
    assert_eq!(
        dispose_epoch_failure(failure, &probe).status.code(),
        Some(9)
    );
    assert!(hex_plugin_state::decode(&initial).is_ok());
}

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
        let stopped = instance.quiesce_retained_on_control().unwrap();
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
            .resume_retained_on_control(next, Some(options().presentation))
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

fn epoch_probe(instance: &mut Prepared) -> Arc<std::sync::atomic::AtomicI32> {
    let probe = Arc::new(std::sync::atomic::AtomicI32::new(i32::MIN));
    instance.lease.reap_probe = Some(Arc::clone(&probe));
    probe
}
fn retained_epoch_failure<T>(
    result: std::result::Result<T, PreparationFailure>,
    probe: &Arc<std::sync::atomic::AtomicI32>,
) -> PreparationFailure {
    let failure = match result {
        Err(failure) => failure,
        Ok(_) => panic!("epoch refusal cannot yield a reusable instance"),
    };
    assert_eq!(
        probe.load(Ordering::Acquire),
        i32::MIN,
        "error handoff must not destroy the native lease"
    );
    let preparation_failure::Custody::Native(lease) = &failure.custody else {
        panic!("epoch refusal must keep its actual native lease")
    };
    assert!(
        lease
            .reap_probe
            .as_ref()
            .is_some_and(|original| Arc::ptr_eq(original, probe))
    );
    failure
}
fn dispose_epoch_failure(
    mut failure: PreparationFailure,
    probe: &Arc<std::sync::atomic::AtomicI32>,
) -> Retirement {
    let deadline = Instant::now() + options().preparation.limits.retirement;
    let retired = loop {
        match failure.poll_disposal_on_control().unwrap() {
            Some(PreparationDisposal::Reaped(retired)) => break retired,
            Some(PreparationDisposal::NoInstance) => panic!("cannot erase the original child"),
            None => {}
        }
        assert!(
            Instant::now() < deadline,
            "exact native disposal must be observed"
        );
        thread::yield_now();
    };
    assert!(
        retired.terminal.is_none(),
        "no normal terminal was acknowledged"
    );
    assert_eq!(
        probe.load(Ordering::Acquire),
        i32::MIN,
        "polling is not owner destruction"
    );
    drop(failure);
    assert_eq!(
        probe.load(Ordering::Acquire),
        retired.status.code().unwrap_or(-1)
    );
    retired
}
fn epoch_refusal<T>(
    result: std::result::Result<T, PreparationFailure>,
    expected: Option<u32>,
    probe: &Arc<std::sync::atomic::AtomicI32>,
) {
    let failure = retained_epoch_failure(result, probe);
    if let Some(expected) = expected {
        assert!(
            matches!(failure.cause(), Error::Worker(code) if *code == expected),
            "{:?}",
            failure.cause()
        );
    }
    assert!(!dispose_epoch_failure(failure, probe).status.success());
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
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(variant),
        )
        .unwrap();
        let probe = epoch_probe(&mut instance);
        if resuming {
            let stopped = instance.quiesce_on_control().unwrap();
            let previous = stopped.configuration().binding;
            epoch_refusal(
                stopped.resume_retained_on_control(
                    Binding {
                        epoch: previous.epoch + 1,
                        ..previous
                    },
                    Some(options().presentation),
                ),
                reason,
                &probe,
            );
        } else {
            epoch_refusal(instance.quiesce_retained_on_control(), reason, &probe);
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
    let fixture_build = build();
    let fixture_source = source();
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
            let mut instance = Prepared::prepare(
                &fixture_build,
                fixture_source.clone(),
                options(),
                context,
                case.setup(variant),
            )
            .unwrap();
            // Apply the shorter deadline only after the real child is prepared.
            instance.limits.startup = HELD_OPERATION_TIMEOUT;
            let probe = epoch_probe(&mut instance);
            let result = if resuming {
                let stopped = instance.quiesce_on_control().unwrap();
                let previous = stopped.configuration().binding;
                case.after_stage(
                    stage,
                    move || {
                        stopped
                            .resume_retained_on_control(
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
                    move || instance.quiesce_retained_on_control().map(|_| ()),
                    || {
                        if cancel {
                            token.cancel_on_control();
                        }
                    },
                )
            };
            let failure = retained_epoch_failure(result, &probe);
            assert!(
                matches!(failure.cause(), Error::Cancelled) && cancel
                    || matches!(failure.cause(), Error::Timeout) && !cancel,
                "{:?}",
                failure.cause()
            );
            assert_eq!(
                dispose_epoch_failure(failure, &probe).status.code(),
                Some(9)
            );
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
                let failure = retained_epoch_failure(
                    stopped.resume_retained_on_control(
                        next,
                        if terminal == "unknown" {
                            None
                        } else {
                            Some(options().presentation)
                        },
                    ),
                    &reap,
                );
                assert_eq!(dispose_epoch_failure(failure, &reap).status.code(), Some(9));
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

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_stopped_flush_refusal_exception_shape_controller_and_retained_borrow_keep_failed_source_custody()
 {
    use crate::transport::{ParameterPoint, Parameters};
    for (variant, reason) in [(72, 7), (73, 12), (75, 7), (76, 11), (77, 7)] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup_restore(variant),
        )
        .unwrap();
        let initial = instance.ready().initial_state.clone();
        let probe = epoch_probe(&mut instance);
        let stopped = instance.quiesce_retained_on_control().unwrap();
        let mut points = Parameters::default();
        points
            .push(ParameterPoint {
                frame: 0,
                id: 7,
                normalized: 0.75,
            })
            .unwrap();
        epoch_refusal(
            stopped.flush_parameters_retained_on_control(&points),
            Some(reason),
            &probe,
        );
        assert_eq!(
            &decode(&initial).unwrap().component.unwrap()[8..12],
            &1.0_f32.to_le_bytes(),
            "failure never rewrites confirmed startup state"
        );
    }
    for cancel in [true, false] {
        let case = Case::new();
        let context = PreparationCancellation::new_on_control().unwrap();
        let token = context.token_on_control();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            context,
            case.setup_restore(74),
        )
        .unwrap();
        let probe = epoch_probe(&mut instance);
        let stopped = instance.quiesce_retained_on_control().unwrap();
        let result = case.after_stage(
            24,
            move || {
                let mut points = Parameters::default();
                points
                    .push(ParameterPoint {
                        frame: 0,
                        id: 7,
                        normalized: 0.75,
                    })
                    .unwrap();
                stopped
                    .flush_parameters_retained_on_control(&points)
                    .map(|_| ())
            },
            || {
                if cancel {
                    token.cancel_on_control();
                }
            },
        );
        let failure = retained_epoch_failure(result, &probe);
        assert!(
            matches!(failure.cause(), Error::Cancelled) && cancel
                || matches!(failure.cause(), Error::Timeout) && !cancel
        );
        assert_eq!(
            dispose_epoch_failure(failure, &probe).status.code(),
            Some(9)
        );
    }
}

fn overflow_capture_recovers(variant: u32) {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(variant),
    )
    .unwrap();
    let child = instance.child_id_on_control();
    instance
        .exchange_on_worker(request(instance.configuration(), 0))
        .unwrap();
    let stopped = instance.quiesce_retained_on_control().unwrap();
    let (stopped, result) = stopped
        .capture_state_or_refusal_retained_while_on_control(|| false)
        .unwrap();
    assert!(
        matches!(result, Err(crate::protocol::StateCaptureRefusal::ExceedsLimit { observed_bytes }) if observed_bytes == if variant == 95 { 5000 } else { 5032 })
    );
    assert_eq!(stopped.child_id_on_control(), child);
    let (stopped, result) = stopped
        .capture_state_or_refusal_retained_while_on_control(|| false)
        .unwrap();
    assert!(decode(&result.unwrap()).unwrap().component.is_some());
    let previous = stopped.configuration().binding;
    let mut instance = stopped
        .resume_retained_on_control(
            Binding {
                epoch: previous.epoch + 1,
                ..previous
            },
            Some(options().presentation),
        )
        .unwrap();
    assert_eq!(instance.child_id_on_control(), child);
    instance
        .exchange_on_worker(request(instance.configuration(), 1))
        .unwrap();
    assert!(
        instance
            .stop_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_component_overflow_stopped_capture_keeps_instance() {
    overflow_capture_recovers(95);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_controller_overflow_stopped_capture_keeps_instance() {
    overflow_capture_recovers(96);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_overflow_with_invalid_stream_use_is_still_terminal_when_stopped() {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(97),
    )
    .unwrap();
    let probe = epoch_probe(&mut instance);
    let stopped = instance.quiesce_retained_on_control().unwrap();
    let result = stopped.capture_state_retained_on_control();
    epoch_refusal(result, Some(6), &probe);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_overflow_with_unrelated_sdk_error_is_still_terminal_when_stopped() {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(98),
    )
    .unwrap();
    let probe = epoch_probe(&mut instance);
    let stopped = instance.quiesce_retained_on_control().unwrap();
    let result = stopped.capture_state_retained_on_control();
    epoch_refusal(result, Some(6), &probe);
}
