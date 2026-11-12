use super::*;

#[test]
fn only_an_exact_armed_playing_vst3_target_uses_the_recording_gesture() {
    use hex_session::device_gesture::{GestureRequest, PluginParameterValue};

    let device = touch().audio.input.device();
    let class = [9; 16];
    let origin = PluginParameterOrigin {
        context: AudioSourceContext {
            project_generation: 1,
            editor_revision: 3,
        },
        device,
        parameter: 0,
        page_first: 0,
        metadata_revision: 4,
        initial: PluginParameterValue::new(0.25).unwrap(),
    };
    let arm = Arm {
        generation: 1,
        revision: 3,
        target: ArmTarget::Vst3 {
            device,
            class,
            parameter: 0,
        },
    };
    for (armed, selected_class, playing, desired, expected) in [
        (Some(arm), Some(class), true, true, Ok(true)),
        (Some(arm), Some(class), false, false, Ok(false)),
        (None, Some(class), true, true, Ok(false)),
        (Some(arm), Some([8; 16]), true, true, Ok(false)),
        (
            Some(Arm {
                generation: 0,
                ..arm
            }),
            Some(class),
            true,
            true,
            Ok(false),
        ),
        (
            Some(Arm { revision: 2, ..arm }),
            Some(class),
            true,
            true,
            Err("automation arm changed · arm this parameter again"),
        ),
        (
            Some(arm),
            Some(class),
            true,
            false,
            Err("wait for playback to settle before recording automation"),
        ),
    ] {
        let mut recorder = Recorder {
            arm: armed,
            ..Recorder::default()
        };
        let mut client = GestureClient::default();
        let result = recorder.begin_plugin(&mut client, origin, selected_class, playing, desired);
        match expected {
            Ok(expected) => {
                let key = result.unwrap();
                assert_eq!(
                    client.next_request(),
                    Some(GestureRequest::BeginPlugin {
                        key,
                        origin,
                        recording: expected,
                    })
                );
                assert_eq!(
                    recorder.plugin_take.map(|(owner, _)| owner),
                    expected.then_some(key)
                );
            }
            Err(reason) => {
                assert_eq!(result, Err(reason));
                assert!(
                    client.next_request().is_none(),
                    "refusal must not enqueue a scalar edit"
                );
            }
        }
    }
}

#[test]
fn delivered_batch_retries_in_order_before_the_actual_restoration() {
    let mut touch = touch();
    assert!(touch.next_submission().is_none());
    let application = NativeParameterApplication {
        identity: touch.audio.input.identity(),
        operation: NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(
            HostGain::ZERO,
        )),
        arrangement_frame: 19,
        performance_frame: 23,
        loop_pass: 0,
        remaining_latency_frames: 4,
    };
    let restored = NativeParameterApplication {
        operation: NativeTransientOperation::Clear(DeviceAutomationTarget::HostOutputGain),
        arrangement_frame: 23,
        performance_frame: 27,
        ..application
    };
    touch.phase = Phase::Delivering;
    touch.in_flight = None;
    touch.end = Some(End::Release);
    touch.issued_sequence = 2;
    assert!(!touch.release_ready());
    touch.receipt(Receipt::Applied {
        target: touch.audio,
        sequence: 1,
        application,
    });
    assert!(!touch.release_ready());
    touch.receipt(Receipt::Superseded {
        target: touch.audio,
        sequence: 2,
    });
    assert!(!touch.release_ready());
    touch.issued_sequence = 3;
    touch.receipt(Receipt::Applied {
        target: touch.audio,
        sequence: 3,
        application,
    });
    assert!(touch.release_ready());
    touch.receipt(Receipt::Finished {
        target: touch.audio,
        application: restored,
    });
    let Some((Submission::Append, Action::Append(batch))) = touch.next_submission() else {
        panic!("delivery must be sent before the restoration");
    };
    assert_eq!(&*batch, &[(1, application), (3, application)]);
    touch.in_flight = Some((2, Submission::Append));
    assert!(touch.next_submission().is_none());
    touch.in_flight = None;
    let Some((Submission::Append, Action::Append(retry))) = touch.next_submission() else {
        panic!("backpressure must retain the original batch");
    };
    assert_eq!(retry, batch);
    touch.batch.clear();
    assert!(
        matches!(touch.next_submission(), Some((Submission::Terminal, Action::Finish(value))) if value == restored)
    );
}

#[test]
fn full_refused_batch_retains_entire_endpoint_window_and_restoration() {
    let mut touch = touch();
    touch.phase = Phase::Delivering;
    touch.in_flight = None;
    let target = touch.audio;
    let applied = |sequence| NativeParameterApplication {
        identity: target.input.identity(),
        operation: NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(
            HostGain::ZERO,
        )),
        arrangement_frame: sequence + 1,
        performance_frame: sequence + 5,
        loop_pass: 2,
        remaining_latency_frames: 4,
    };
    let end = (EVENTS_PER_BATCH + MAX_PENDING_VALUE_RECEIPTS) as u64;
    let restored = NativeParameterApplication {
        operation: NativeTransientOperation::Clear(DeviceAutomationTarget::HostOutputGain),
        ..applied(end + 1)
    };
    let mut waiting = (1..=end)
        .map(|sequence| Receipt::Applied {
            target,
            sequence,
            application: applied(sequence),
        })
        .chain(std::iter::once(Receipt::Finished {
            target,
            application: restored,
        }))
        .map(ScalarReceipt::NativeParameter);

    touch.drain_receipts(|| waiting.next());
    assert_eq!(touch.batch.len(), EVENTS_PER_BATCH);
    assert!(touch.terminal.is_none());
    touch.in_flight = Some((2, Submission::Append));
    touch.retain_after_refusal();
    touch.drain_receipts(|| waiting.next());
    assert_eq!(touch.batch.len(), end as usize);
    assert_eq!(
        touch.batch,
        (1..=end).map(|seq| (seq, applied(seq))).collect::<Vec<_>>()
    );
    assert!(matches!(
        touch.terminal,
        Some(Action::Finish(application))
            if application == restored
    ));
    assert!(waiting.next().is_none());
    assert!(touch.next_submission().is_none());
}
