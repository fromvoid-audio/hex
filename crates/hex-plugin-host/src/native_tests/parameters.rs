//! Actual source-bound SDK parameter processing through the production Rust
//! reader. Direct worker exchanges are not Root/callback delivery acceptance.
use super::*;
use crate::transport::{ParameterPoint, Parameters};

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
