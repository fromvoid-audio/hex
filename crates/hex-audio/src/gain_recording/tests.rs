use super::*;
fn target() -> Target {
    Target {
        track_id: 1,
        project_generation: 0,
        canonical_revision: 2,
        audio_revision: 3,
        recording_epoch: 1,
        transport_epoch: 1,
        gesture_serial: 1,
        sample_rate: 48000,
    }
}
fn clock(frame: u64, delay: u64) -> Clock {
    Clock {
        performance_frame: frame,
        content_frame: frame + 100,
        delivery_delay: delay,
        loop_pass: 0,
    }
}
#[test]
fn only_written_compensated_samples_ack_values_and_release() {
    let (mut control, mut callback) = channel();
    control.begin(target(), 0.25).unwrap();
    assert_eq!(callback.prepare(|_| Ok(clock(0, 7))), Some((1, 0.25)));
    assert!(control.try_receipt().is_none());
    callback.finish_quantum(7, true);
    assert!(control.try_receipt().is_none());
    callback.finish_quantum(8, true);
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Applied {
            gain: 0.25,
            sequence: 1,
            performance_frame: 0,
            content_frame: 100,
            ..
        })
    ));
    control.release().unwrap();
    assert_eq!(callback.prepare(|_| Ok(clock(8, 7))), None);
    callback.finish_quantum(15, true);
    assert_eq!(control.target(), Some(target()));
    callback.finish_quantum(16, true);
    assert!(control.try_receipt().is_none());
    callback.finish_quantum(254, true);
    assert!(control.try_receipt().is_none());
    callback.finish_quantum(255, true);
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Finished {
            performance_frame: 8,
            content_frame: 108,
            ..
        })
    ));
    assert!(control.target().is_none());
}
#[test]
fn latest_boundary_value_wins_and_unwritten_or_cancelled_takes_never_commit() {
    let (mut control, mut callback) = channel();
    control.begin(target(), 0.1).unwrap();
    control.change(0.8).unwrap();
    assert_eq!(callback.prepare(|_| Ok(clock(10, 0))), Some((1, 0.8)));
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Superseded { sequence: 1, .. })
    ));
    assert!(control.try_receipt().is_none());
    callback.finish_quantum(20, false);
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Superseded { sequence: 2, .. })
    ));
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Aborted {
            reason: AbortReason::OutputFailed,
            ..
        })
    ));
    control.begin(target(), 0.5).unwrap();
    callback.abort(AbortReason::TransportChanged); // Includes a not-yet-consumed Begin.
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Superseded { .. })
    ));
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Aborted { .. })
    ));
    assert!(control.target().is_none());
}
#[test]
fn terminal_reservation_survives_full_undrained_normal_prefix() {
    let (mut control, mut callback) = channel();
    control.begin(target(), 0.5).unwrap();
    for _ in 1..IN_FLIGHT {
        control.change(1.0).unwrap();
    }
    assert!(control.change(1.5).is_err());
    control.release().unwrap();
    assert_eq!(callback.prepare(|_| Ok(clock(0, 0))), None);
    callback.finish_quantum(16, true);
    callback.finish_quantum(240, true);
    let mut superseded = 0;
    let mut finished = 0;
    while let Some(receipt) = control.try_receipt() {
        match receipt {
            Receipt::Superseded { .. } => superseded += 1,
            Receipt::Finished { .. } => finished += 1,
            _ => panic!("no sample carried a value in this empty gesture"),
        }
    }
    assert_eq!((superseded, finished), (IN_FLIGHT, 1));
    assert!(!callback.receipt_fault);
    assert!(control.target().is_none());
}
#[test]
fn reset_discards_in_flight_delivery_markers_without_acknowledging_them() {
    let (mut control, mut callback) = channel();
    control.begin(target(), 0.4).unwrap();
    callback.prepare(|_| Ok(clock(0, 100)));
    callback.finish_quantum(16, true);
    assert!(control.try_receipt().is_none());
    callback.prepare(|_| Err(AbortReason::SourceChanged));
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Superseded { .. })
    ));
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Aborted {
            reason: AbortReason::SourceChanged,
            ..
        })
    ));
    callback.finish_quantum(200, true);
    assert!(control.try_receipt().is_none());
    for gain in [f32::NAN, f32::INFINITY, -0.1, 2.1] {
        assert!(control.begin(target(), gain).is_err());
    }
}

#[test]
fn abort_drains_late_old_commands_before_admitting_a_new_take() {
    let (mut control, mut callback) = channel();
    control.begin(target(), 0.5).unwrap();
    callback.prepare(|_| Ok(clock(0, 0)));
    callback.finish_quantum(16, true);
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Applied { .. })
    ));
    callback.abort(AbortReason::TransportChanged);
    // Producer has not observed the terminal yet and may still submit one value.
    control.change(0.75).unwrap();
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Aborted { .. })
    ));
    assert!(control.begin(target(), 1.0).is_err());
    callback.prepare(|_| Err(AbortReason::TransportChanged));
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Superseded { .. })
    ));
    assert!(control.try_receipt().is_none());
    control.begin(target(), 1.0).unwrap();
    callback.prepare(|_| Ok(clock(16, 0)));
    callback.finish_quantum(32, true);
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Applied { gain: 1.0, .. })
    ));
    assert!(!callback.receipt_fault);
}

#[test]
fn delivery_clock_overflow_releases_the_in_hand_value_credit_before_abort() {
    let (mut control, mut callback) = channel();
    control.begin(target(), 0.5).unwrap();
    callback.prepare(|_| {
        Ok(Clock {
            performance_frame: u64::MAX - 5,
            content_frame: 100,
            delivery_delay: 7,
            loop_pass: 0,
        })
    });
    callback.finish_quantum(u64::MAX, true);
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Superseded { sequence: 1, .. })
    ));
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Aborted {
            reason: AbortReason::ClockExhausted,
            ..
        })
    ));
    assert!(control.target().is_none());
    control
        .begin(
            Target {
                gesture_serial: 2,
                ..target()
            },
            1.0,
        )
        .unwrap();
    callback.prepare(|_| Ok(clock(0, 0)));
    callback.finish_quantum(16, true);
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Applied { gain: 1.0, .. })
    ));
    assert!(!callback.receipt_fault);
}

#[test]
fn split_admission_rolls_back_enqueue_failure_and_stale_terminal_cannot_release_new_take() {
    let status = std::sync::Arc::new(crate::prepared::Status::default());
    // The factory's split marker is set by the split boundary; this fixture
    // exercises the same measured Status without cloning an endpoint.
    status.set_split_for_test();
    let (mut control, mut callback) = channel();
    control.bind_status(status.clone());
    for _ in 0..CAPACITY {
        control
            .commands
            .push(Command {
                target: target(),
                sequence: 0,
                kind: Kind::Cancel,
            })
            .ok()
            .unwrap();
    }
    assert!(control.begin(target(), 0.5).is_err());
    assert!(control.target().is_none());
    assert!(status.publication_reserve_for_test());
    status.publication_clear_for_test();
    while callback.commands.pop().is_ok() {}
    control.begin(target(), 0.5).unwrap();
    let old = target();
    callback
        .receipts
        .push(Receipt::Aborted {
            target: old,
            reason: AbortReason::Cancelled,
        })
        .unwrap();
    assert!(matches!(
        control.try_receipt(),
        Some(Receipt::Aborted { .. })
    ));
    let mut next = old;
    next.recording_epoch += 1;
    // A real previous receipt must drain the outstanding Begin reservation.
    callback
        .receipts
        .push(Receipt::Superseded {
            target: old,
            sequence: 1,
        })
        .unwrap();
    control.try_receipt().unwrap();
    control.begin(next, 0.5).unwrap();
    callback
        .receipts
        .push(Receipt::Aborted {
            target: old,
            reason: AbortReason::Cancelled,
        })
        .unwrap();
    control.try_receipt().unwrap();
    assert_eq!(control.target(), Some(next));
    assert!(!status.publication_reserve_for_test());
    callback
        .receipts
        .push(Receipt::Aborted {
            target: next,
            reason: AbortReason::Cancelled,
        })
        .unwrap();
    control.try_receipt().unwrap();
    assert!(status.publication_reserve_for_test());
}
