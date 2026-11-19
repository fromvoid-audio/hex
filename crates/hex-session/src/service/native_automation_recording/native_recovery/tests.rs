use super::*;
use crate::service::automation_draft::tests::injected_failed_writer_with_terminal;
use hex_engine::{NativeTransientOperation, NativeTransientValue};
use hex_lattice_contract::{DeviceAutomationTarget, HostGain};
use hex_storage::automation_draft::{DraftClock, DraftReader, DraftWriter};

const LIMITS: DraftReadLimits = DraftReadLimits {
    max_source_bytes: 64 * 1024,
};

fn fixture() -> (
    super::super::tests::Files,
    Session,
    DeviceAutomationTakeScope,
    DeviceAutomationSource,
) {
    let (files, session, scope, source) = super::super::tests::fixture();
    let target: DeviceCaptureTarget = hex_engine::native_parameter_input_target(
        session.project(),
        session.project().main_devices()[0].id(),
        DeviceAutomationTarget::HostOutputGain,
    )
    .unwrap()
    .into();
    (
        files,
        session,
        DeviceAutomationTakeScope { target, ..scope },
        DeviceAutomationSource { target, ..source },
    )
}

fn gain(sequence: u64) -> HostGain {
    HostGain::from_db(if sequence.is_multiple_of(2) {
        -6.0
    } else {
        -12.0
    })
    .unwrap()
}

fn application(scope: DeviceAutomationTakeScope, sequence: u64) -> NativeParameterApplication {
    NativeParameterApplication {
        identity: scope.target.native().unwrap().identity(),
        operation: NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(gain(
            sequence,
        ))),
        arrangement_frame: sequence * 8,
        performance_frame: sequence * 8 + 4,
        loop_pass: 0,
        remaining_latency_frames: 4,
    }
}

fn expected(sequence: u64) -> AppliedEvent {
    AppliedEvent {
        sequence,
        performance_frame: sequence * 8 + 4,
        content_frame: sequence * 8,
        loop_pass: 0,
        value: hex_project::AutomationValue::new(gain(sequence).normalized()).unwrap(),
    }
}

fn failed(
    path: &Path,
    session: &Session,
    scope: DeviceAutomationTakeScope,
    source: DeviceAutomationSource,
    terminal: DraftTerminal,
) -> DraftWorkerOutcome {
    let target = scope.target.native().unwrap();
    let writer = DraftWriter::create_with_clock(
        path,
        session.project(),
        DraftTarget::Device {
            device: target.device(),
            target: target.target(),
        },
        scope.stamp,
        source.capture_clock(session.project()),
    )
    .unwrap();
    injected_failed_writer_with_terminal(writer, expected(1), terminal)
}

#[test]
fn copies_failed_writer_and_native_continuation_in_pages_with_actual_terminal() {
    let (files, session, scope, source) = fixture();
    let original_path = files.0.join("original");
    let outcome = failed(
        &original_path,
        &session,
        scope,
        source,
        DraftTerminal::Interrupted,
    );
    let original = std::fs::read(&original_path).unwrap();
    assert_eq!(outcome.retained[0].events(), &[expected(1)]);
    let last = EVENTS_PER_BATCH as u64 + 2;
    let applications: Arc<[_]> = (2..=last)
        .map(|seq| (seq, application(scope, seq)))
        .collect();
    let clear = NativeParameterApplication {
        operation: NativeTransientOperation::Clear(DeviceAutomationTarget::HostOutputGain),
        ..application(scope, last + 1)
    };
    let finished = DraftTerminal::Finished(DraftClock {
        performance_frame: (last + 1) * 8 + 4,
        content_frame: (last + 1) * 8,
        loop_pass: 0,
    });
    for (name, observed, terminal) in [
        (
            "finished",
            NativeRecoveryTerminal::Restored(clear),
            finished,
        ),
        (
            "aborted",
            NativeRecoveryTerminal::Aborted(DraftAbortReason::Cancelled),
            DraftTerminal::Aborted(DraftAbortReason::Cancelled),
        ),
        (
            "interrupted",
            NativeRecoveryTerminal::Interrupted,
            DraftTerminal::Interrupted,
        ),
    ] {
        let path = files.0.join(name);
        let continuation = NativeRecoveryContinuation {
            applications: Arc::clone(&applications),
            terminal: observed,
        };
        let receipt = copy_native_recovery_on_control(
            &outcome,
            &path,
            LIMITS,
            last,
            scope,
            source,
            &continuation,
        )
        .unwrap();
        assert_eq!(receipt.events(), last);
        assert_eq!(receipt.terminal(), terminal);
        receipt.verify_on_control(&path, LIMITS, last).unwrap();
        let mut reader = DraftReader::open(&path, LIMITS).unwrap();
        assert_eq!(
            reader.capture_clock(),
            Some(source.capture_clock(session.project()))
        );
        assert!(reader.source_matches(session.project()).unwrap());
        let mut events = vec![None; last as usize];
        assert_eq!(reader.read_events(0, &mut events).unwrap(), last as usize);
        assert!(
            events
                .into_iter()
                .eq((1..=last).map(|seq| Some(expected(seq))))
        );
        assert_eq!(reader.progress().durable_terminal, Some(terminal));
        assert_eq!(std::fs::read(&original_path).unwrap(), original);
        assert_eq!(outcome.retained[0].events(), &[expected(1)]);
    }
}

#[test]
fn appends_original_native_tail_after_a_clean_interrupted_writer_prefix() {
    let (files, session, scope, source) = fixture();
    let original = files.0.join("original");
    let target = scope.target.native().unwrap();
    let (mut input, worker) = DeviceDraftInput::start_on_control(
        &session,
        None,
        original.clone(),
        target,
        scope.stamp,
        source.capture_clock(session.project()),
    )
    .unwrap();
    assert!(
        input
            .append_delivered(target, scope.stamp, &[(1, application(scope, 1))])
            .unwrap()
            .is_some()
    );
    let outcome = worker.join_on_control().unwrap();
    assert_eq!(outcome.progress.durable_events, 1);
    assert_eq!(
        outcome.progress.durable_terminal,
        Some(DraftTerminal::Interrupted)
    );
    assert!(outcome.failure.is_none());
    let before = std::fs::read(&original).unwrap();
    let path = files.0.join("copy");
    let continuation = NativeRecoveryContinuation {
        applications: Arc::from([(2, application(scope, 2))]),
        terminal: NativeRecoveryTerminal::Interrupted,
    };
    let receipt =
        copy_native_recovery_on_control(&outcome, &path, LIMITS, 2, scope, source, &continuation)
            .unwrap();
    assert_eq!(receipt.events(), 2);
    let mut reader = DraftReader::open(&path, LIMITS).unwrap();
    let mut events = [None; 2];
    assert_eq!(reader.read_events(0, &mut events).unwrap(), 2);
    assert_eq!(events, [Some(expected(1)), Some(expected(2))]);
    assert_eq!(std::fs::read(original).unwrap(), before);
}

#[test]
fn native_recovery_refuses_wrong_custody_order_terminal_or_damaged_input() {
    let (files, session, scope, source) = fixture();
    let original_path = files.0.join("original");
    let admitted = DraftTerminal::Finished(DraftClock {
        performance_frame: 36,
        content_frame: 32,
        loop_pass: 0,
    });
    let outcome = failed(&original_path, &session, scope, source, admitted);
    let original = std::fs::read(&original_path).unwrap();
    let valid = NativeRecoveryContinuation {
        applications: Arc::from([(2, application(scope, 2))]),
        terminal: NativeRecoveryTerminal::Interrupted,
    };
    let copy = |name, budget, scope, source, tail: &NativeRecoveryContinuation| {
        copy_native_recovery_on_control(
            &outcome,
            &files.0.join(name),
            LIMITS,
            budget,
            scope,
            source,
            tail,
        )
    };
    let mut wrong = scope;
    wrong.stamp.editor_revision += 1;
    assert!(copy("stamp", 3, wrong, source, &valid).is_err());
    let mut wrong_source = source;
    wrong_source.transport_epoch += 1;
    assert!(copy("source", 3, scope, wrong_source, &valid).is_err());
    wrong_source = source;
    wrong_source.section_loop = Some(hex_project::LoopRegion::new(0, 48_000, true).unwrap());
    assert!(copy("clock", 3, scope, wrong_source, &valid).is_err());
    assert!(copy("budget", 1, scope, source, &valid).is_err());
    assert!(!files.0.join("budget").exists());
    let mut bad = valid.clone();
    bad.applications = Arc::from(vec![
        (2, application(scope, 2));
        MAX_NATIVE_RECOVERY_CONTINUATION_EVENTS + 1
    ]);
    assert!(copy("tail-size", 1024, scope, source, &bad).is_err());
    bad.applications = Arc::from([(1, application(scope, 1))]);
    assert!(copy("overlap", 3, scope, source, &bad).is_err());
    bad.applications = Arc::from([(3, application(scope, 3)), (2, application(scope, 2))]);
    assert!(copy("order", 3, scope, source, &bad).is_err());
    bad.applications = Arc::from([(2, application(scope, 0))]);
    assert!(copy("regressed-clock", 3, scope, source, &bad).is_err());
    bad.applications = Arc::from([(
        2,
        NativeParameterApplication {
            operation: NativeTransientOperation::Set(NativeTransientValue::HostInputGain(gain(2))),
            ..application(scope, 2)
        },
    )]);
    assert!(copy("target", 3, scope, source, &bad).is_err());
    bad = valid.clone();
    bad.terminal = NativeRecoveryTerminal::Restored(application(scope, 3));
    assert!(copy("not-restored", 3, scope, source, &bad).is_err());
    bad.terminal = NativeRecoveryTerminal::Aborted(DraftAbortReason::Cancelled);
    assert!(copy("different-terminal", 3, scope, source, &bad).is_err());
    assert_eq!(std::fs::read(&original_path).unwrap(), original);
    std::fs::remove_file(&original_path).unwrap();
    assert!(copy("missing", 3, scope, source, &valid).is_err());
    std::fs::write(&original_path, b"damaged header").unwrap();
    assert!(copy("damaged", 3, scope, source, &valid).is_err());
    std::fs::write(&original_path, &original).unwrap();
    let receipt = copy("verified", 3, scope, source, &valid).unwrap();
    assert_eq!(receipt.terminal(), admitted);
    assert_eq!(receipt.events(), 2);
    assert_eq!(std::fs::read(&original_path).unwrap(), original);
    assert_eq!(outcome.retained[0].events(), &[expected(1)]);
}
