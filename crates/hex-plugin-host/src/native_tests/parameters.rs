//! Actual source-bound SDK parameter processing through the production Rust
//! reader. Direct worker exchanges are not Root/callback delivery acceptance.
use super::*;
use crate::protocol::{ParameterPageProgress, ParameterPageQuery};
use crate::transport::{ParameterPoint, Parameters};

fn change_fixture_latency(instance: &mut Prepared, value: f64) -> Result<Block> {
    let mut block = request(instance.configuration(), 0);
    block
        .parameters
        .push(ParameterPoint {
            frame: 0,
            id: 7,
            normalized: value,
        })
        .unwrap();
    instance.exchange_on_worker(block)
}

fn wait_for_latency_restart(instance: &mut Prepared) -> std::num::NonZeroU64 {
    let until = Instant::now() + Duration::from_millis(500);
    loop {
        if let Some(pending) = instance
            .poll_recovery_edits_on_worker()
            .unwrap()
            .pending_latency_restart
        {
            return pending;
        }
        assert!(Instant::now() < until, "latency restart was not observed");
        thread::yield_now();
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_latency_restart_survives_receipt_reads_and_newer_activation_request() {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(104),
    )
    .unwrap();
    let child = instance.child_id_on_control();
    assert!(
        instance
            .poll_recovery_edits_on_worker()
            .unwrap()
            .pending_latency_restart
            .is_none()
    );
    assert_eq!(instance.ready().algorithmic_latency_frames, 0);
    change_fixture_latency(&mut instance, 0.75).unwrap();
    let pending = wait_for_latency_restart(&mut instance);
    assert_eq!(
        instance
            .poll_recovery_edits_on_worker()
            .unwrap()
            .pending_latency_restart,
        Some(pending)
    );
    let stopped = instance.quiesce_retained_on_control().unwrap();
    assert_eq!(stopped.facts().algorithmic_latency_frames, 0);
    let mut stopped = stopped.reactivate_retained_on_control().unwrap();
    assert_eq!(stopped.facts().algorithmic_latency_frames, 7);
    assert!(
        stopped
            .poll_recovery_edits_on_control()
            .unwrap()
            .pending_latency_restart
            .unwrap()
            > pending,
        "covering the old ordinal must retain a restart raised during activation"
    );
    let mut stopped = stopped.reactivate_retained_on_control().unwrap();
    assert!(
        stopped
            .poll_recovery_edits_on_control()
            .unwrap()
            .pending_latency_restart
            .is_none()
    );
    let old = stopped.configuration().binding;
    let mut instance = stopped
        .resume_retained_on_control(
            Binding {
                epoch: old.epoch + 1,
                ..old
            },
            Some(options().presentation),
        )
        .unwrap();
    assert_eq!(instance.child_id_on_control(), child);
    assert_eq!(instance.ready().algorithmic_latency_frames, 7);
    let block = request(instance.configuration(), 1);
    let reply = instance.exchange_on_worker(block).unwrap();
    assert_eq!(&reply.pcm[..7], &[[0.0; 2]; 7]);
    assert_ne!(reply.pcm[7], [0.0; 2]);
    let terminal = instance
        .stop_and_reap_on_control()
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (2, 16));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_latency_restart_requires_opt_in_and_rejects_mixed_io() {
    for opted_in in [false, true] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup(104),
        )
        .unwrap();
        if opted_in {
            instance.poll_recovery_edits_on_worker().unwrap();
        }
        let mut result =
            change_fixture_latency(&mut instance, if opted_in { 0.25 } else { 0.75 }).map(|_| ());
        let until = Instant::now() + Duration::from_millis(500);
        while result.is_ok() {
            assert!(
                Instant::now() < until,
                "unsupported restart was not refused"
            );
            result = instance.poll_parameter_edits_on_worker().map(|_| ());
        }
        let error = result.unwrap_err();
        let expected = if opted_in {
            "restart requires graph changes"
        } else {
            "recursive controller callback"
        };
        assert!(error.to_string().contains(expected), "{error:?}");
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_uncovered_latency_restart_cannot_resume_stale_facts() {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup(104),
    )
    .unwrap();
    instance.poll_recovery_edits_on_worker().unwrap();
    change_fixture_latency(&mut instance, 0.75).unwrap();
    wait_for_latency_restart(&mut instance);
    let stopped = instance.quiesce_retained_on_control().unwrap();
    let old = stopped.configuration().binding;
    let Err(failure) = stopped.resume_retained_on_control(
        Binding {
            epoch: old.epoch + 1,
            ..old
        },
        Some(options().presentation),
    ) else {
        panic!("stale latency must never acknowledge a resumed epoch");
    };
    assert!(matches!(failure.cause(), Error::Worker(15)), "{failure:?}");
    let _ = failure.finish_on_control();
    assert_eq!(
        case.observation().unwrap()[8],
        1,
        "processing started only at startup"
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_parameter_pcm_feedback_and_capture_keep_original_source_with_no_caller_allocations() {
    for quantum in [1, 7, 64, 256] {
        let case = Case::new();
        let mut configured = options();
        configured.preparation.quantum = quantum;
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            configured,
            PreparationCancellation::new_on_control().unwrap(),
            case.setup_restore(37),
        )
        .unwrap();
        let child = instance.child_id_on_control();
        let initial = instance.ready().initial_state.clone();
        for n in 0..3 {
            let mut input = request(instance.configuration(), n);
            if n < 2 {
                input
                    .parameters
                    .push(ParameterPoint {
                        frame: if n == 0 { 0 } else { quantum - 1 },
                        id: 7,
                        normalized: if n == 0 { 0.25 } else { 0.75 },
                    })
                    .unwrap();
            }
            #[cfg(feature = "allocation-probe")]
            let result = {
                let mut result = None;
                let measured = allocation_counter::measure(|| {
                    result = Some(instance.exchange_on_worker(input));
                });
                assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
                assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
                result.unwrap()
            };
            #[cfg(not(feature = "allocation-probe"))]
            let result = instance.exchange_on_worker(input);
            let output = result.unwrap();
            assert_eq!(
                (output.binding, output.sequence, output.first_frame),
                (input.binding, n, n * u64::from(quantum))
            );
            for frame in 0..usize::from(quantum) {
                let normalized = match n {
                    0 => 0.25,
                    1 => 0.25 + 0.5 * (frame + 1) as f64 / f64::from(quantum),
                    _ => 0.75,
                };
                for channel in 0..2 {
                    let expected = f64::from(input.pcm[frame][channel])
                        * (0.5 + n as f64 * 0.125)
                        * normalized
                        * 4.0;
                    assert!(
                        (f64::from(output.pcm[frame][channel]) - expected).abs() < 1e-6,
                        "q{quantum}, block{n}, frame{frame}, channel{channel}"
                    );
                }
            }
            let mut expected = Parameters::default();
            if n < 2 {
                expected
                    .push(ParameterPoint {
                        frame: quantum - 1,
                        id: 7,
                        normalized: if n == 0 { 0.25 } else { 0.75 },
                    })
                    .unwrap();
            }
            assert_eq!(output.parameters, expected);
            assert_eq!(instance.child_id_on_control(), child);
            assert_eq!(instance.ready().initial_state, initial);
        }
        instance.begin_state_capture_on_worker().unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        let captured = loop {
            if let Some(state) = instance.poll_state_capture_on_worker().unwrap() {
                break state;
            }
            assert!(Instant::now() < until, "actual parameter-state capture");
            thread::yield_now();
        };
        assert_eq!(
            &decode(&captured).unwrap().component.unwrap()[8..12],
            &3.0_f32.to_le_bytes()
        );
        assert_eq!(
            &decode(&initial).unwrap().component.unwrap()[8..12],
            &1.0_f32.to_le_bytes()
        );
        assert_eq!(instance.child_id_on_control(), child);
        let terminal = instance
            .stop_and_reap_on_control()
            .unwrap()
            .terminal
            .unwrap();
        assert_eq!(
            (terminal.blocks, terminal.frames),
            (3, u64::from(quantum) * 3)
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_stopped_parameter_flush_synchronizes_original_controller_without_pcm_and_resumes_same_source()
 {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup_restore(37),
    )
    .unwrap();
    let child = instance.child_id_on_control();
    let initial = instance.ready().initial_state.clone();
    instance
        .exchange_on_worker(request(instance.configuration(), 0))
        .unwrap();
    let stopped = instance.quiesce_retained_on_control().unwrap();
    let (stopped, empty) = stopped
        .flush_parameters_retained_on_control(&Parameters::default())
        .unwrap();
    assert_eq!(empty.disposition, protocol::FlushDisposition::Empty);
    assert!(empty.as_slice().is_empty());
    assert_eq!(
        case.observation().unwrap()[11],
        0,
        "empty flush makes no SDK processing call"
    );
    let mut points = Parameters::default();
    points
        .push(ParameterPoint {
            frame: 0,
            id: 7,
            normalized: 0.75,
        })
        .unwrap();
    let (stopped, applied) = stopped
        .flush_parameters_retained_on_control(&points)
        .unwrap();
    assert_eq!(applied.disposition, protocol::FlushDisposition::Applied);
    assert_eq!(
        applied.as_slice(),
        &[protocol::FlushedParameter {
            id: 7,
            normalized: 0.75
        }]
    );
    assert_eq!(stopped.child_id_on_control(), child);
    assert_eq!(stopped.completed_blocks(), 1);
    let (stopped, captured) = stopped.capture_state_retained_on_control().unwrap();
    assert_eq!(
        &decode(&captured).unwrap().component.unwrap()[8..12],
        &3.0_f32.to_le_bytes()
    );
    let observed = case.observation().unwrap();
    assert_eq!(
        (observed[8], observed[9], observed[10], observed[11]),
        (1, 1, 1, 1)
    );
    assert_eq!(
        case.restore_observation()[6],
        i32::from_le_bytes(3.0_f32.to_le_bytes())
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
    assert_eq!(instance.ready().initial_state, initial);
    let input = request(instance.configuration(), 1);
    let output = instance.exchange_on_worker(input).unwrap();
    for n in 0..8 {
        for channel in 0..2 {
            assert_eq!(output.pcm[n][channel], input.pcm[n][channel] * 0.625 * 3.0);
        }
    }
    let terminal = instance
        .stop_and_reap_on_control()
        .unwrap()
        .terminal
        .unwrap();
    assert_eq!((terminal.blocks, terminal.frames), (2, 16));
    assert_eq!(case.observation().unwrap()[1], 0);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_host_input_updates_controller_without_fabricating_optional_processor_feedback() {
    for variant in [78, 79] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup_restore(variant),
        )
        .unwrap();
        let child = instance.child_id_on_control();
        let initial = instance.ready().initial_state.clone();
        let mut input = request(instance.configuration(), 0);
        input
            .parameters
            .push(ParameterPoint {
                frame: if variant == 78 { 0 } else { 7 },
                id: 7,
                normalized: 0.75,
            })
            .unwrap();
        let output = instance.exchange_on_worker(input).unwrap();
        if variant == 78 {
            assert!(
                output.parameters.as_slice().is_empty(),
                "SDK output changes are optional, not a fabricated echo"
            );
        } else {
            assert_eq!(
                output.parameters.as_slice(),
                &[ParameterPoint {
                    frame: 0,
                    id: 7,
                    normalized: 0.25
                }],
                "preserve actual early processor feedback"
            );
        }
        for (n, actual) in output.pcm[..8].iter().enumerate() {
            let gain = if variant == 78 {
                3.0
            } else if n == 0 {
                1.0
            } else {
                1.0 + 2.0 * (n + 1) as f32 / 8.0
            };
            for (actual, source) in actual.iter().zip(input.pcm[n]) {
                assert!((*actual - source * 0.5 * gain).abs() < 1e-6);
            }
        }
        let stopped = instance.quiesce_retained_on_control().unwrap();
        let (stopped, captured) = stopped.capture_state_retained_on_control().unwrap();
        assert_eq!(
            &decode(&captured).unwrap().component.unwrap()[8..12],
            &3.0_f32.to_le_bytes()
        );
        assert_eq!(
            case.restore_observation()[6],
            i32::from_le_bytes(3.0_f32.to_le_bytes()),
            "main controller must receive latest host input, with no echo or earlier output (variant{variant})"
        );
        assert_eq!(stopped.child_id_on_control(), child);
        assert_eq!(stopped.completed_blocks(), 1);
        assert_eq!(
            &decode(&initial).unwrap().component.unwrap()[8..12],
            &1.0_f32.to_le_bytes()
        );
        assert!(stopped.retire_on_control().unwrap().status.success());
        assert_eq!(case.observation().unwrap()[1], 0);
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_sdk_zero_id_stepped_and_bypass_apply_exact_offsets_and_stopped_readback() {
    for variant in [80, 81, 82] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup_restore(variant),
        )
        .unwrap();
        let child = instance.child_id_on_control();
        let initial = instance.ready().initial_state.clone();
        let id = if variant == 80 { 0 } else { 7 };
        let (first, last) = match variant {
            80 => (0.25, 0.75),
            81 => (0.49, 0.84),
            _ => (0.0, 1.0),
        };
        let mut input = request(instance.configuration(), 0);
        for (frame, normalized) in [(0, first), (3, first), (4, last), (7, last)] {
            input
                .parameters
                .push(ParameterPoint {
                    frame,
                    id,
                    normalized,
                })
                .unwrap();
        }
        let output = instance.exchange_on_worker(input).unwrap();
        let end = if variant == 80 { 0.75 } else { 1.0 };
        assert_eq!(
            output.parameters.as_slice(),
            &[ParameterPoint {
                frame: 7,
                id,
                normalized: end
            }]
        );
        for (n, actual) in output.pcm[..8].iter().enumerate() {
            let factor = match (variant, n < 4) {
                (80, true) => 0.5,
                (80, false) => 1.5,
                (81, true) => 2.0 / 3.0,
                (81, false) => 2.0,
                (82, true) => 0.5,
                _ => 1.0,
            };
            for (actual, input) in actual.iter().zip(input.pcm[n]) {
                assert!(
                    (f64::from(*actual) - f64::from(input) * factor).abs() < 1e-6,
                    "variant{variant}/frame{n}"
                );
            }
        }
        let stopped = instance.quiesce_retained_on_control().unwrap();
        let mut points = Parameters::default();
        points
            .push(ParameterPoint {
                frame: 0,
                id,
                normalized: 0.6,
            })
            .unwrap();
        let (stopped, values) = stopped
            .flush_parameters_retained_on_control(&points)
            .unwrap();
        let expected = match variant {
            80 => 0.6,
            81 => 2.0 / 3.0,
            _ => 1.0,
        };
        assert_eq!(values.as_slice().len(), 1);
        assert_eq!(values.as_slice()[0].id, id);
        assert!((values.as_slice()[0].normalized - expected).abs() < 1e-6);
        let (stopped, captured) = stopped.capture_state_retained_on_control().unwrap();
        let state = decode(&captured).unwrap();
        let gain = f32::from_le_bytes(state.component.unwrap()[8..12].try_into().unwrap());
        assert!((f64::from(gain) - expected * 4.0).abs() < 1e-6);
        assert_eq!(
            case.restore_observation()[6],
            i32::from_le_bytes(gain.to_le_bytes())
        );
        assert_eq!(stopped.child_id_on_control(), child);
        assert_eq!(stopped.completed_blocks(), 1);
        assert_ne!(
            initial, captured,
            "capture is fresh data, startup history is retained separately"
        );
        assert!(stopped.retire_on_control().unwrap().status.success());
        assert_eq!(case.observation().unwrap()[1], 0);
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_readonly_and_hidden_accept_processor_updates_but_refuse_live_and_stopped_host_input() {
    for variant in [83, 84] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup_restore(variant),
        )
        .unwrap();
        let input = request(instance.configuration(), 0);
        let output = instance.exchange_on_worker(input).unwrap();
        assert_eq!(&output.pcm[..8], &input.pcm[..8]);
        assert_eq!(
            output.parameters.as_slice(),
            &[ParameterPoint {
                frame: 0,
                id: 7,
                normalized: 0.5
            }]
        );
        let stopped = instance.quiesce_retained_on_control().unwrap();
        let (stopped, captured) = stopped.capture_state_retained_on_control().unwrap();
        assert_eq!(
            &decode(&captured).unwrap().component.unwrap()[8..12],
            &2.0_f32.to_le_bytes()
        );
        assert_eq!(
            case.restore_observation()[6],
            i32::from_le_bytes(2.0_f32.to_le_bytes())
        );
        assert!(stopped.retire_on_control().unwrap().status.success());
        for stopped in [false, true] {
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
            let mut points = Parameters::default();
            points
                .push(ParameterPoint {
                    frame: 0,
                    id: 7,
                    normalized: 0.75,
                })
                .unwrap();
            if stopped {
                let source = instance.quiesce_retained_on_control().unwrap();
                let mut failure = match source.flush_parameters_retained_on_control(&points) {
                    Err(failure) => failure,
                    Ok(_) => panic!("read-only stopped input must refuse"),
                };
                assert!(matches!(failure.cause(), Error::Worker(8)));
                let until = Instant::now() + Duration::from_secs(3);
                loop {
                    if let Some(PreparationDisposal::Reaped(retired)) =
                        failure.poll_disposal_on_control().unwrap()
                    {
                        assert!(!retired.status.success());
                        break;
                    }
                    assert!(Instant::now() < until);
                    thread::yield_now();
                }
            } else {
                let mut input = request(instance.configuration(), 0);
                input.parameters = points;
                assert!(matches!(
                    instance.exchange_on_worker(input),
                    Err(Error::Worker(8))
                ));
                assert!(
                    !instance
                        .cancel_and_reap_on_control()
                        .unwrap()
                        .status
                        .success()
                );
            }
            assert_eq!(
                &decode(&initial).unwrap().component.unwrap()[8..12],
                &1.0_f32.to_le_bytes()
            );
            assert_eq!(
                case.observation().unwrap()[10],
                0,
                "read-only/hidden input refuses before actual DSP entry"
            );
        }
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_sdk_ui_gestures_apply_on_original_worker_live_and_stopped_with_ordered_receipts() {
    use crate::protocol::ParameterEditKind;
    for variant in [85, 89] {
        for stopped in [false, true] {
            let case = Case::new();
            let instance = Prepared::prepare(
                &build(),
                source(),
                options(),
                PreparationCancellation::new_on_control().unwrap(),
                case.setup_restore(variant),
            )
            .unwrap();
            let child = instance.child_id_on_control();
            let initial = instance.ready().initial_state.clone();
            let mut live = Some(instance);
            let mut paused = if stopped {
                Some(live.take().unwrap().quiesce_retained_on_control().unwrap())
            } else {
                None
            };
            std::fs::write(case.release(), b"emit actual SDK callbacks").unwrap();
            let mut records = Vec::new();
            let until = Instant::now() + Duration::from_secs(3);
            loop {
                let batch = if let Some(instance) = live.as_mut() {
                    instance.poll_parameter_edits_on_worker().unwrap()
                } else {
                    paused
                        .as_mut()
                        .unwrap()
                        .poll_parameter_edits_on_control()
                        .unwrap()
                };
                records.extend_from_slice(batch.as_slice());
                if batch.ready > 0 && records.len() == 1 {
                    break;
                }
                assert!(Instant::now() < until, "actual main callback handoff");
                thread::yield_now();
            }
            if let Some(instance) = live.as_mut() {
                let input = request(instance.configuration(), 0);
                let output = instance.exchange_on_worker(input).unwrap();
                for (actual, input) in output.pcm[..8].iter().zip(input.pcm) {
                    assert_eq!(*actual, input.map(|value| value * 1.5));
                }
            } else {
                let (owner, values) = paused
                    .take()
                    .unwrap()
                    .flush_parameters_retained_on_control(&Parameters::default())
                    .unwrap();
                assert_eq!(values.disposition, protocol::FlushDisposition::Applied);
                assert_eq!(values.as_slice()[0].normalized, 0.75);
                paused = Some(owner);
            }
            let until = Instant::now() + Duration::from_secs(3);
            while records.len() < 3 {
                let batch = if let Some(instance) = live.as_mut() {
                    instance.poll_parameter_edits_on_worker().unwrap()
                } else {
                    paused
                        .as_mut()
                        .unwrap()
                        .poll_parameter_edits_on_control()
                        .unwrap()
                };
                records.extend_from_slice(batch.as_slice());
                assert!(Instant::now() < until, "exact gesture terminal");
                thread::yield_now();
            }
            assert_eq!(
                records.iter().map(|edit| edit.kind).collect::<Vec<_>>(),
                [
                    ParameterEditKind::Begin,
                    ParameterEditKind::Perform,
                    ParameterEditKind::End
                ]
            );
            let id = if variant == 89 { 0 } else { 7 };
            for (index, event) in records.iter().enumerate() {
                assert_eq!(
                    (event.sequence, event.gesture, event.id),
                    (index as u64 + 1, 1, id)
                );
            }
            assert_eq!(records[1].normalized, 0.75);
            assert_eq!(records[1].first_frame, 0);
            let paused = if let Some(instance) = live.take() {
                instance.quiesce_retained_on_control().unwrap()
            } else {
                paused.take().unwrap()
            };
            let (paused, captured) = paused.capture_state_retained_on_control().unwrap();
            assert_eq!(paused.child_id_on_control(), child);
            assert_eq!(paused.completed_blocks(), u64::from(!stopped));
            assert_eq!(
                &decode(&captured).unwrap().component.unwrap()[8..12],
                &3.0_f32.to_le_bytes()
            );
            assert_eq!(
                &decode(&initial).unwrap().component.unwrap()[8..12],
                &1.0_f32.to_le_bytes()
            );
            let observed = case.observation().unwrap();
            assert_eq!(
                (observed[10], observed[11]),
                if stopped { (0, 1) } else { (1, 0) }
            );
            assert!(paused.retire_on_control().unwrap().status.success());
            assert_eq!(case.observation().unwrap()[1], 0);
        }
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_sdk_editor_burst_drains_in_order_on_the_original_worker() {
    for idle in [false, true] {
        editor_burst_drains(idle);
    }
}

fn editor_burst_drains(idle: bool) {
    use crate::protocol::ParameterEditKind;

    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup_restore(116),
    )
    .unwrap();
    let child = instance.child_id_on_control();
    std::fs::write(case.release(), b"emit one native editor burst").unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    let mut edits = Vec::new();
    let mut next_block = 0;
    while edits.len() < 14 {
        let batch = if idle {
            instance
                .service_idle_editor_edits_on_worker()
                .unwrap()
                .edits
        } else {
            instance.poll_parameter_edits_on_worker().unwrap()
        };
        edits.extend_from_slice(batch.as_slice());
        if !idle && batch.ready > 0 {
            let input = request(instance.configuration(), next_block);
            instance.exchange_on_worker(input).unwrap();
            next_block += 1;
        }
        assert!(Instant::now() < until, "native editor burst must drain");
        thread::yield_now();
    }
    assert_eq!(edits.len(), 14);
    for (index, edit) in edits.iter().enumerate() {
        let expected = match index {
            0 => (ParameterEditKind::Begin, 0.0),
            13 => (ParameterEditKind::End, 0.0),
            _ => (ParameterEditKind::Perform, 0.75),
        };
        assert_eq!(
            (edit.sequence, edit.gesture, edit.id),
            (index as u64 + 1, 1, 7)
        );
        assert_eq!((edit.kind, edit.normalized), expected);
    }
    assert_eq!(instance.child_id_on_control(), child);
    if idle {
        assert!(edits.iter().all(|edit| edit.first_frame == 0));
        assert_eq!(instance.completed_blocks_on_control(), 0);
        let input = request(instance.configuration(), 0);
        let output = instance.exchange_on_worker(input).unwrap();
        for (actual, input) in output.pcm[..8].iter().zip(input.pcm) {
            assert_eq!(*actual, input.map(|value| value * 1.5));
        }
    }
    let paused = instance.quiesce_retained_on_control().unwrap();
    let (paused, captured) = paused.capture_state_retained_on_control().unwrap();
    assert_eq!(
        &decode(&captured).unwrap().component.unwrap()[8..12],
        &3.0_f32.to_le_bytes()
    );
    assert!(paused.retire_on_control().unwrap().status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_sdk_playback_does_not_replay_stale_editor_values() {
    for authored in [false, true] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup_restore(135),
        )
        .unwrap();
        fs::write(case.release(), b"finish the editor drag").unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while instance.poll_parameter_edits_on_worker().unwrap().ready == 0 {
            assert!(Instant::now() < until, "the drag must reach the worker");
            thread::yield_now();
        }
        let mut input = request(instance.configuration(), 0);
        if authored {
            input
                .parameters
                .push(ParameterPoint {
                    frame: 4,
                    id: 7,
                    normalized: 0.5,
                })
                .unwrap();
        }
        let output = instance.exchange_on_worker(input).unwrap();
        assert_eq!(
            output.parameters.as_slice(),
            &[ParameterPoint {
                frame: input.frames - 1,
                id: 7,
                normalized: if authored { 0.5 } else { 0.25 },
            }]
        );
        instance
            .begin_parameter_page_on_worker(ParameterPageQuery::default())
            .unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            match instance.poll_parameter_page_on_worker().unwrap() {
                ParameterPageProgress::Page(page) => {
                    assert_eq!(
                        page.as_slice()[0].normalized,
                        if authored { 0.5 } else { 0.875 }
                    );
                    break;
                }
                ParameterPageProgress::Pending => {
                    assert!(Instant::now() < until, "controller feedback must settle");
                    thread::yield_now();
                }
                other => panic!("expected a controller page, got {other:?}"),
            }
        }
        assert!(
            instance
                .stop_and_reap_on_control()
                .unwrap()
                .status
                .success()
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_sdk_saturated_drag_keeps_final_values_and_gesture_boundaries() {
    assert_saturated_editor_drags(87, 1);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_sdk_admits_five_drags_before_the_previous_gestures_drain() {
    assert_saturated_editor_drags(131, 5);
}

fn assert_saturated_editor_drags(variant: u32, drags: usize) {
    use crate::protocol::ParameterEditKind::{Begin, End, Perform};

    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup_restore(variant),
    )
    .unwrap();
    let child = instance.child_id_on_control();
    std::fs::write(case.release(), b"emit a two-axis drag").unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    let mut edits: Vec<protocol::ParameterEdit> = Vec::new();
    while edits.iter().filter(|edit| edit.kind == End).count() < 2 * drags {
        let batch = instance.service_idle_editor_edits_on_worker().unwrap();
        edits.extend_from_slice(batch.edits.as_slice());
        assert!(Instant::now() < until, "the entire drag must drain");
        thread::yield_now();
    }

    assert_eq!(edits.len(), 64);
    assert!(
        edits
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert!(edits.iter().all(|edit| edit.first_frame == 0));
    for (id, final_value, step) in [(7, 0.875, 0.09375), (11, 0.625, 0.0625)] {
        let mut axis = edits.iter().filter(|edit| edit.id == id);
        for drag in 0..drags {
            let begin = axis.next().unwrap();
            assert_eq!(begin.kind, Begin);
            let mut final_perform = None;
            loop {
                let edit = axis.next().unwrap();
                assert_eq!(edit.gesture, begin.sequence);
                if edit.kind == End {
                    break;
                }
                assert_eq!(edit.kind, Perform);
                final_perform = Some(edit.normalized);
            }
            assert_eq!(
                final_perform,
                Some(final_value - (drags - 1 - drag) as f64 * step)
            );
        }
        assert!(axis.next().is_none());
    }
    assert_eq!(instance.completed_blocks_on_control(), 0);
    assert_eq!(instance.child_id_on_control(), child);
    let input = request(instance.configuration(), 0);
    let output = instance.exchange_on_worker(input).unwrap();
    for (actual, input) in output.pcm[..8].iter().zip(input.pcm) {
        assert_eq!(*actual, input.map(|value| value * 1.75));
    }
    assert!(
        instance
            .quiesce_retained_on_control()
            .unwrap()
            .retire_on_control()
            .unwrap()
            .status
            .success()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_sdk_drains_deferred_main_lane_objects_before_unload() {
    let case = Case::new();
    let events = case.root.join("module-events");
    let event_path = events.clone();
    let setup = case.setup_restore(134);
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
    fs::write(case.release(), b"trigger a controller timer").unwrap();
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        if fs::read(&events).unwrap() == b"ER" {
            break;
        }
        assert!(
            Instant::now() < until,
            "the timer's deferred release must run while the worker is active"
        );
        thread::sleep(Duration::from_millis(2));
    }
    instance
        .exchange_on_worker(request(instance.configuration(), 0))
        .unwrap();
    let retired = instance.stop_and_reap_on_control().unwrap();
    assert!(retired.status.success());
    let terminal = retired.terminal.unwrap();
    assert!(terminal.closed && terminal.unloaded);
    assert_eq!(fs::read(&events).unwrap(), b"ERCXT");
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_sdk_standalone_edits_keep_the_last_value_at_queue_capacity() {
    use crate::protocol::ParameterEditKind::{Begin, End, Perform};

    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup_restore(86),
    )
    .unwrap();
    let child = instance.child_id_on_control();
    std::fs::write(case.release(), b"emit standalone editor values").unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    let mut edits = Vec::new();
    loop {
        let batch = instance.service_idle_editor_edits_on_worker().unwrap();
        edits.extend_from_slice(batch.edits.as_slice());
        if edits.windows(2).any(|pair| {
            pair[0].kind == Perform && pair[0].normalized == 0.875 && pair[1].kind == End
        }) {
            break;
        }
        assert!(
            Instant::now() < until,
            "the final standalone value must drain"
        );
        thread::yield_now();
    }

    assert_eq!(edits.len(), 63);
    assert!(
        edits
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    for gesture in edits.chunks_exact(3) {
        assert_eq!(
            [gesture[0].kind, gesture[1].kind, gesture[2].kind],
            [Begin, Perform, End]
        );
        assert!(gesture.iter().all(|edit| {
            edit.id == 7 && edit.gesture == gesture[0].sequence && edit.first_frame == 0
        }));
    }
    assert_eq!(edits[1].normalized, 0.5, "keep the explicit gesture");
    assert_eq!(instance.completed_blocks_on_control(), 0);
    assert_eq!(instance.child_id_on_control(), child);
    let input = request(instance.configuration(), 0);
    let output = instance.exchange_on_worker(input).unwrap();
    for (actual, input) in output.pcm[..8].iter().zip(input.pcm) {
        assert_eq!(*actual, input.map(|value| value * 1.75));
    }
    assert!(
        instance
            .quiesce_retained_on_control()
            .unwrap()
            .retire_on_control()
            .unwrap()
            .status
            .success()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_sdk_invalid_ui_gestures_refuse_before_dsp() {
    for (variant, expected) in [
        (88, "read-only edit parameter"),
        (133, "invalid edit value"),
    ] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup_restore(variant),
        )
        .unwrap();
        let child = instance.child_id_on_control();
        let initial = instance.ready().initial_state.clone();
        std::fs::write(case.release(), b"emit invalid actual SDK callbacks").unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match instance.poll_parameter_edits_on_worker() {
                Ok(batch) => {
                    // Accepted Begin or queued values may be observed before
                    // main publishes refusal. No rejected Perform is applied.
                    assert!(
                        batch
                            .as_slice()
                            .iter()
                            .all(|event| event.kind != protocol::ParameterEditKind::Perform)
                    );
                }
                Err(error @ Error::Worker(code)) if crate::runtime_host_refusal(code) => {
                    assert!(error.to_string().contains(expected), "{error:?}");
                    break;
                }
                Err(error) => {
                    panic!("variant {variant}: expected actual HostControl refusal, got {error:?}")
                }
            }
            assert!(
                Instant::now() < until,
                "variant {variant}: callback refusal must terminate the source"
            );
            thread::yield_now();
        }
        assert_eq!(instance.child_id_on_control(), child);
        assert_eq!(instance.completed_blocks_on_control(), 0);
        assert_eq!(instance.ready().initial_state, initial);
        assert!(instance.poll_parameter_edits_on_worker().is_err());
        let retired = instance.cancel_and_reap_on_control().unwrap();
        assert!(!retired.status.success());
        assert_eq!(
            case.observation().unwrap()[10],
            0,
            "invalid callbacks must not enter DSP"
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_sdk_idle_feedback_refusal_identifies_controller_setter() {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup_restore(132),
    )
    .unwrap();
    std::fs::write(case.release(), b"emit editor value that the setter rejects").unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    let error = loop {
        if let Err(error) = instance.service_idle_editor_edits_on_worker() {
            break error;
        }
        assert!(Instant::now() < until, "controller refusal did not arrive");
        thread::yield_now();
    };
    let message = error.to_string();
    assert!(
        message.contains("host refused controller feedback"),
        "{message}"
    );
    assert!(
        message.contains("controller rejected setParamNormalized"),
        "{message}"
    );
    assert_eq!(instance.completed_blocks_on_control(), 0);
    assert!(
        !instance
            .cancel_and_reap_on_control()
            .unwrap()
            .status
            .success()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_sdk_edit_during_capture_defers_snapshot_until_real_processing() {
    let case = Case::new();
    let mut instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup_restore(90),
    )
    .unwrap();
    let child = instance.child_id_on_control();
    let initial = instance.ready().initial_state.clone();
    instance.begin_state_capture_on_worker().unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let mut began = false;
    loop {
        let batch = instance.poll_parameter_edits_on_worker().unwrap();
        began |= batch
            .as_slice()
            .iter()
            .any(|edit| edit.kind == protocol::ParameterEditKind::Begin);
        if began && batch.ready > 0 {
            break;
        }
        assert!(
            Instant::now() < until,
            "SDK capture must admit the actual gesture"
        );
        thread::yield_now();
    }
    assert!(
        instance.poll_state_capture_on_worker().unwrap().is_none(),
        "pre-edit capture must not escape while Perform is pending"
    );
    assert_eq!(instance.completed_blocks_on_control(), 0);
    let input = request(instance.configuration(), 0);
    let output = instance.exchange_on_worker(input).unwrap();
    for (actual, input) in output.pcm[..8].iter().zip(input.pcm) {
        assert_eq!(*actual, input.map(|value| value * 1.5));
    }
    let until = Instant::now() + Duration::from_secs(3);
    let captured = loop {
        if let Some(state) = instance.poll_state_capture_on_worker().unwrap() {
            break state;
        }
        assert!(
            Instant::now() < until,
            "capture must finish after actual SDK application"
        );
        thread::yield_now();
    };
    assert_eq!(
        &decode(&captured).unwrap().component.unwrap()[8..12],
        &3.0_f32.to_le_bytes()
    );
    assert_eq!(instance.child_id_on_control(), child);
    assert_eq!(instance.ready().initial_state, initial);
    assert_eq!(instance.completed_blocks_on_control(), 1);
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
fn actual_sdk_edit_during_stopped_capture_refuses_incoherent_snapshot() {
    let case = Case::new();
    let instance = Prepared::prepare(
        &build(),
        source(),
        options(),
        PreparationCancellation::new_on_control().unwrap(),
        case.setup_restore(90),
    )
    .unwrap();
    let initial = instance.ready().initial_state.clone();
    let child = instance.child_id_on_control();
    let stopped = instance.quiesce_retained_on_control().unwrap();
    let (stopped, result) = stopped
        .capture_state_or_refusal_retained_while_on_control(|| false)
        .unwrap();
    assert!(matches!(
        result,
        Err(protocol::StateCaptureRefusal::StillChanging)
    ));
    assert_eq!(stopped.child_id_on_control(), child);
    assert_eq!(
        &decode(&initial).unwrap().component.unwrap()[8..12],
        &1.0_f32.to_le_bytes()
    );
    let observed = case.observation().unwrap();
    assert_eq!(
        (observed[10], observed[11]),
        (0, 0),
        "capture must not invent PCM or flush work"
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
    instance
        .exchange_on_worker(request(instance.configuration(), 0))
        .unwrap();
    assert_eq!(instance.child_id_on_control(), child);
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
fn exact_pcm_editor_performs_exclude_overwritten_values_without_consuming_receipts() {
    use crate::protocol::ParameterEditKind::{Begin, End, Perform};

    for (variant, raw_id, min_ready) in [(89, 0, 1), (135, 7, 4), (147, 7, 1)] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup_restore(variant),
        )
        .unwrap();
        if variant == 147 {
            let class = instance.configuration().identity.class;
            let metadata = instance
                .read_authored_parameter_metadata_on_control(class, &[7])
                .unwrap();
            instance
                .arm_authored_parameter_metadata_on_control(metadata)
                .unwrap();
        }
        let initial = request(instance.configuration(), 0);
        let reply = instance.exchange_editor_on_worker(initial).unwrap();
        assert!(reply.performs.as_slice().is_empty());
        fs::write(case.release(), b"emit original editor performs").unwrap();
        let mut ordinary = Vec::new();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let batch = instance.poll_parameter_edits_on_worker().unwrap();
            ordinary.extend_from_slice(batch.as_slice());
            if batch.ready >= min_ready {
                break;
            }
            assert!(Instant::now() < until, "editor input was not queued");
            thread::yield_now();
        }
        let mut effective = Vec::new();
        let mut sequence = 1;
        loop {
            let mut input = request(instance.configuration(), sequence);
            let authored = variant == 147 && sequence == 1;
            if authored {
                input
                    .parameters
                    .push(ParameterPoint {
                        frame: 4,
                        id: raw_id,
                        normalized: 0.5,
                    })
                    .unwrap();
            }
            let reply = if authored {
                instance
                    .exchange_armed_authored_with_editor_on_worker(input, None)
                    .unwrap()
            } else {
                instance.exchange_editor_on_worker(input).unwrap()
            };
            assert_eq!(
                (
                    reply.block.binding,
                    reply.block.sequence,
                    reply.block.first_frame
                ),
                (input.binding, sequence, sequence * 8),
            );
            let points = reply.performs.as_slice();
            assert!(
                points.len() <= 1,
                "offset-zero duplicate was reported twice"
            );
            if let Some(point) = points.first() {
                assert_eq!(point.id, raw_id);
                let scale = 0.5 + sequence as f64 * 0.125;
                for (offset, (sample, original)) in
                    reply.block.pcm[..8].iter().zip(input.pcm).enumerate()
                {
                    // The fixture linearly interpolates from the actual
                    // offset-zero editor point to the authored point at 4.
                    let normalized = if authored {
                        point.normalized + (0.5 - point.normalized) * offset.min(4) as f64 / 4.0
                    } else {
                        point.normalized
                    };
                    assert_eq!(
                        *sample,
                        original
                            .map(|sample| (f64::from(sample) * scale * 4.0 * normalized) as f32),
                        "variant {variant}, quantum {sequence}, offset {offset}"
                    );
                }
                effective.push((*point, reply.block.first_frame));
            }
            let batch = instance.poll_parameter_edits_on_worker().unwrap();
            ordinary.extend_from_slice(batch.as_slice());
            if ordinary.last().is_some_and(|edit| edit.kind == End) {
                break;
            }
            assert!(Instant::now() < until, "editor receipts did not finish");
            sequence += 1;
        }
        assert_eq!(ordinary.first().unwrap().kind, Begin);
        assert_eq!(ordinary.last().unwrap().kind, End);
        assert_eq!(
            ordinary.iter().filter(|edit| edit.kind == Perform).count(),
            if variant == 135 { 13 } else { 1 }
        );
        assert_eq!(
            effective.last().unwrap().0.normalized,
            if variant == 135 { 0.875 } else { 0.75 }
        );
        if variant == 135 {
            assert!(effective.len() < 13, "the fixture must exercise coalescing");
        }
        for (point, frame) in effective {
            let last = ordinary
                .iter()
                .rev()
                .find(|edit| edit.kind == Perform && edit.first_frame == frame && edit.id == raw_id)
                .unwrap();
            assert_eq!(
                (last.sequence, last.gesture, last.normalized),
                (point.sequence, point.gesture, point.normalized)
            );
        }
        let input = request(instance.configuration(), sequence + 1);
        assert!(
            instance
                .exchange_editor_on_worker(input)
                .unwrap()
                .performs
                .as_slice()
                .is_empty()
        );
        assert!(
            instance
                .stop_and_reap_on_control()
                .unwrap()
                .status
                .success()
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn idle_and_ordinary_editor_input_is_not_replayed_as_opted_in_pcm() {
    use crate::protocol::ParameterEditKind::End;
    for idle in [false, true] {
        let case = Case::new();
        let mut instance = Prepared::prepare(
            &build(),
            source(),
            options(),
            PreparationCancellation::new_on_control().unwrap(),
            case.setup_restore(89),
        )
        .unwrap();
        fs::write(case.release(), b"emit one editor value").unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        let mut sequence = 0;
        loop {
            let finished = if idle {
                instance
                    .service_idle_editor_edits_on_worker()
                    .unwrap()
                    .edits
                    .as_slice()
                    .iter()
                    .any(|edit| edit.kind == End)
            } else {
                let input = request(instance.configuration(), sequence);
                instance.exchange_on_worker(input).unwrap();
                sequence += 1;
                instance
                    .poll_parameter_edits_on_worker()
                    .unwrap()
                    .as_slice()
                    .iter()
                    .any(|edit| edit.kind == End)
            };
            if finished {
                break;
            }
            assert!(Instant::now() < until, "ordinary edit did not finish");
            thread::yield_now();
        }
        let input = request(instance.configuration(), sequence);
        let reply = instance.exchange_editor_on_worker(input).unwrap();
        assert!(reply.performs.as_slice().is_empty());
        let multiplier = 3.0 * (0.5 + sequence as f32 * 0.125);
        for (sample, original) in reply.block.pcm[..8].iter().zip(input.pcm) {
            assert_eq!(*sample, original.map(|sample| sample * multiplier));
        }
        assert!(
            instance
                .stop_and_reap_on_control()
                .unwrap()
                .status
                .success()
        );
    }
}
