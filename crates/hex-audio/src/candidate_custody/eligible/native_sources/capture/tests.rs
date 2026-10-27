//! Actual Root admission and late close around original worker result delivery.
use super::super::tests::{dispose, root, source_reservation, start};
use super::*;
use std::time::{Duration, Instant};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn admitted_parameter_token_survives_source_close() {
    use crate::plugin_worker::NativeWorkerParameterProgress;
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let request = loop {
        match root.parameter_admission(binding, 7, 0.75, || root.close()) {
            Ok(request) => break request,
            Err("native preparation is busy" | "native parameter mailbox is busy") => {}
            Err(reason) => panic!("admission lost its token: {reason}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    assert!(
        root.poll_native_parameter_change(binding, &request)
            .is_err()
    );
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired) if retired.status.success())
    );
    assert_eq!(
        request.poll_retirement_on_control().unwrap(),
        NativeWorkerParameterProgress::Withdrawn
    );
    assert!(request.poll_retirement_on_control().is_err());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_parameter_terminal_survives_a_close_after_consumption() {
    use crate::plugin_worker::{NativeParameterPoll, NativeWorkerParameterProgress};
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let request = loop {
        match root.begin_native_parameter_change(binding, 7, 0.75) {
            Ok(request) => break request,
            Err("native preparation is busy" | "native parameter mailbox is busy") => {}
            Err(reason) => panic!("parameter admission: {reason}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    loop {
        match root.withdraw_native_parameter_change(binding, &request) {
            Ok(true) => break,
            Err("native preparation is busy" | "native parameter mailbox is busy") => {}
            other => panic!("withdrawal before processing: {other:?}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    let result = loop {
        match root.parameter_terminal_receipt(
            binding,
            false,
            || root.close(),
            |owner| {
                let Owner::Effect(owner) = owner else {
                    panic!("original effect")
                };
                owner.poll_parameter_change(&request)
            },
        ) {
            Ok(NativeParameterPoll::Pending) | Err("native preparation is busy") => {}
            Ok(result) => break result,
            Err(reason) => panic!("terminal receipt: {reason}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    let NativeParameterPoll::Terminal(receipt) = result else {
        panic!("actual terminal missing")
    };
    assert_eq!(receipt.progress(), NativeWorkerParameterProgress::Withdrawn);
    assert!(
        !receipt.source_current(),
        "a closing source cannot authorize an edit"
    );
    assert!(
        root.poll_native_parameter_change(binding, &request)
            .is_err()
    );
    drop(request);
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired) if retired.status.success())
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_processed_parameter_receipt_survives_close_without_edit_authority() {
    use super::super::endpoints::NativeEndpointBank;
    use crate::plugin_worker::{NativeParameterPoll, NativeWorkerParameterProgress};
    use hex_lattice::{ForeignProcessorNode, ForeignProcessors};
    if std::env::var_os("HEX_RECEIPT_PROCESSED_CHILD").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env("HEX_RECEIPT_PROCESSED_CHILD", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "37")
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(String::from_utf8_lossy(&child.stdout).contains("1 passed"));
        return;
    }
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let (identity, prepared, configuration) = {
        let work = root.work.try_lock().unwrap();
        let Some(Owner::Effect(owner)) = work.sources.cells[0].owner.as_ref() else {
            panic!("original effect")
        };
        (
            owner.identity(),
            owner.prepared(),
            owner.configuration().clone(),
        )
    };
    let loan = root.loan_native_endpoint(binding).unwrap();
    let mut bank = NativeEndpointBank::new().unwrap();
    assert!(bank.install(binding.root, loan).is_ok());
    let node = ForeignProcessorNode {
        state_approval: None,
        owner: identity.owner(),
        device_id: identity.device_id(),
        device_type: configuration.type_id(),
        sample_rate: 48_000,
        configuration: &configuration,
        prepared,
        bypassed: false,
        input_gain: 1.0,
    };
    let until = Instant::now() + Duration::from_secs(3);
    let request = loop {
        match root.begin_native_parameter_change(binding, 7, 0.75) {
            Ok(request) => break request,
            Err("native preparation is busy" | "native parameter mailbox is busy") => {}
            Err(reason) => panic!("parameter admission: {reason}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    let mut submitted_blocks = 0;
    let result = loop {
        // Drive the actual pipelined endpoint until the worker consumes the
        // input. One callback can precede the worker observing admission.
        let mut frames = [[0.5, -0.25]; 8];
        bank.begin_rendering(8).unwrap();
        bank.process_segment_with_parameters(node, 0, &mut frames, &[], None)
            .unwrap();
        bank.finish_rendering(true).unwrap();
        submitted_blocks += 1;
        while root
            .native_parameter_values(binding)
            .map(|values| values.completed_blocks())
            .unwrap_or(0)
            < submitted_blocks
        {
            assert!(
                Instant::now() < until,
                "original SDK worker block completion"
            );
            std::thread::yield_now();
        }
        match root.parameter_terminal_receipt(
            binding,
            false,
            || root.close(),
            |owner| {
                let Owner::Loaned(owner) = owner else {
                    panic!("original loaned worker")
                };
                owner.poll_parameter_change(&request)
            },
        ) {
            Ok(NativeParameterPoll::Pending) | Err("native preparation is busy") => {}
            Ok(result) => break result,
            Err(reason) => panic!("terminal receipt: {reason}"),
        }
        assert!(Instant::now() < until, "original SDK input completion");
        std::thread::yield_now();
    };
    let NativeParameterPoll::Terminal(receipt) = result else {
        panic!("actual terminal missing")
    };
    let NativeWorkerParameterProgress::Processed(processed) = receipt.progress() else {
        panic!("processed input required")
    };
    assert_eq!((processed.point.id, processed.point.normalized), (7, 0.75));
    assert!(!receipt.source_current());
    assert!(
        root.poll_native_parameter_change(binding, &request)
            .is_err()
    );
    bank.request_epoch_return();
    let endpoint = bank.take_requested(0).unwrap();
    assert!(root.ingest_native_endpoint(endpoint).is_ok());
    drop(request);
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired) if retired.status.success())
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn input_receipt_survives_original_source_retirement() {
    use super::super::endpoints::NativeEndpointBank;
    use crate::plugin_worker::{NativeParameterPoll, NativeWorkerParameterProgress};
    use hex_lattice::{ForeignProcessorNode, ForeignProcessors};
    if std::env::var_os("HEX_RECEIPT_RETIRE_PROCESSED_CHILD").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        for variant in ["37", "10"] {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &name, "--ignored", "--test-threads=1"])
                .env("HEX_RECEIPT_RETIRE_PROCESSED_CHILD", variant)
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", variant)
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "fixture {variant}: {}\n{}",
                String::from_utf8_lossy(&child.stdout),
                String::from_utf8_lossy(&child.stderr)
            );
            assert!(String::from_utf8_lossy(&child.stdout).contains("1 passed"));
        }
        return;
    }
    let failed = std::env::var("HEX_RECEIPT_RETIRE_PROCESSED_CHILD").unwrap() == "10";
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let (identity, prepared, configuration) = {
        let work = root.work.try_lock().unwrap();
        let Some(Owner::Effect(owner)) = work.sources.cells[0].owner.as_ref() else {
            panic!("original effect")
        };
        (
            owner.identity(),
            owner.prepared(),
            owner.configuration().clone(),
        )
    };
    let loan = root.loan_native_endpoint(binding).unwrap();
    let mut bank = NativeEndpointBank::new().unwrap();
    assert!(bank.install(binding.root, loan).is_ok());
    let node = ForeignProcessorNode {
        state_approval: None,
        owner: identity.owner(),
        device_id: identity.device_id(),
        device_type: configuration.type_id(),
        sample_rate: 48_000,
        configuration: &configuration,
        prepared,
        bypassed: false,
        input_gain: 1.0,
    };
    let until = Instant::now() + Duration::from_secs(3);
    let request = loop {
        match root.begin_native_parameter_change(binding, 7, 0.75) {
            Ok(request) => break request,
            Err("native preparation is busy" | "native parameter mailbox is busy") => {}
            Err(reason) => panic!("parameter admission: {reason}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    let mut frames = [[0.5, -0.25]; 8];
    bank.begin_rendering(8).unwrap();
    bank.process_segment_with_parameters(node, 0, &mut frames, &[], None)
        .unwrap();
    bank.finish_rendering(true).unwrap();
    loop {
        let finished = if failed {
            matches!(
                root.poll_native_parameter_change(binding, &request),
                Ok(NativeParameterPoll::TransportRevoked)
            )
        } else {
            root.native_parameter_values(binding)
                .is_ok_and(|values| values.completed_blocks() >= 1)
        };
        if finished {
            break;
        }
        assert!(
            Instant::now() < until,
            "original SDK worker block completion"
        );
        std::thread::yield_now();
    }
    root.close();
    bank.request_epoch_return();
    let endpoint = bank.take_requested(0).unwrap();
    assert!(root.ingest_native_endpoint(endpoint).is_ok());
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.cleanup_error.is_none());
    let terminal = request.poll_retirement_on_control().unwrap();
    if failed {
        assert!(
            returned.error.is_some(),
            "actual SDK processing failure must remain visible"
        );
        assert_eq!(terminal, NativeWorkerParameterProgress::Failed);
    } else {
        assert!(returned.error.is_none());
        let NativeWorkerParameterProgress::Processed(processed) = terminal else {
            panic!("actual processed SDK input terminal required")
        };
        assert_eq!((processed.point.id, processed.point.normalized), (7, 0.75));
    }
    assert!(request.poll_retirement_on_control().is_err());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn finalized_state_snapshot_reports_completed_on_source_close() {
    use crate::plugin_worker::{NativeSnapshotAction, NativeWorkerStateSnapshotProgress};
    for accepted in [false, true] {
        let root = root();
        root.configure_native_sources(1, source_reservation())
            .unwrap();
        let binding = start(&root);
        root.complete_native_source(
            binding,
            Some(hex_plugin_host::protocol::PresentationLatency {
                input_frames: 77,
                output_frames: 91,
            }),
        )
        .unwrap();
        root.start_native_effect(binding).unwrap();
        let request = root.begin_live_native_capture(binding).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        let snapshot = loop {
            match root.poll_live_native_snapshot(binding, &request) {
                Ok(NativeWorkerStateSnapshotProgress::Captured(snapshot)) => break snapshot,
                Ok(NativeWorkerStateSnapshotProgress::Pending)
                | Err("native preparation is busy") => {}
                other => panic!("original snapshot capture: {other:?}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        };
        assert_eq!(
            root.stage_live_native_snapshot(binding, &snapshot),
            Ok(NativeSnapshotAction::Current)
        );
        assert_eq!(
            root.finalize_snapshot_receipt(binding, &snapshot, accepted, || root.close()),
            Ok(NativeSnapshotAction::SourceClosed),
            "finalization completed before close for accepted={accepted}"
        );
        assert!(
            root.finalize_live_native_snapshot(binding, &snapshot, accepted)
                .is_err()
        );
        dispose(&root, binding);
        let returned = root.recover_native_source(binding).unwrap();
        assert!(returned.cleanup_error.is_none());
        assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn staged_state_snapshot_reports_completed_on_source_close() {
    use crate::plugin_worker::{NativeSnapshotAction, NativeWorkerStateSnapshotProgress};
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let request = root.begin_live_native_capture(binding).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let snapshot = loop {
        match root.poll_live_native_snapshot(binding, &request) {
            Ok(NativeWorkerStateSnapshotProgress::Captured(snapshot)) => break snapshot,
            Ok(NativeWorkerStateSnapshotProgress::Pending) | Err("native preparation is busy") => {}
            other => panic!("original snapshot capture: {other:?}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    assert_eq!(
        root.stage_snapshot_receipt(binding, &snapshot, || root.close()),
        Ok(NativeSnapshotAction::SourceClosed),
        "staging completed before close and must retain its exact disposition"
    );
    assert!(
        root.matches_staged_native_snapshot(binding, &snapshot, snapshot.state())
            .is_err()
    );
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.cleanup_error.is_none());
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn consumed_state_snapshot_reports_discarded_on_source_close() {
    use crate::plugin_worker::NativeWorkerStateSnapshotProgress as Progress;
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let request = root.begin_live_native_capture(binding).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match root.snapshot_receipt(binding, &request, || root.close()) {
            Ok(Progress::Discarded) => break,
            Ok(Progress::Pending) | Err("native preparation is busy") => {}
            other => panic!("consumed close must discard the actual snapshot: {other:?}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    assert!(root.poll_live_native_snapshot(binding, &request).is_err());
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.cleanup_error.is_none());
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn admitted_state_capture_token_survives_source_close() {
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let request = loop {
        match root.capture_admission(binding, || root.close()) {
            Ok(request) => break request,
            Err("native preparation is busy" | "native capture mailbox is busy") => {}
            Err(reason) => panic!("capture admission lost its token: {reason}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    assert!(root.poll_live_native_capture(binding, &request).is_err());
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.cleanup_error.is_none());
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn admitted_controller_query_token_survives_source_close() {
    use hex_plugin_host::protocol::ParameterPageQuery;
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let request = loop {
        match root
            .controller_query_admission(binding, ParameterPageQuery::default(), || root.close())
        {
            Ok(request) => break request,
            Err("native preparation is busy" | "native controller mailbox is busy") => {}
            Err(reason) => panic!("controller admission lost its token: {reason}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    assert!(!request.original_worker_released_on_control());
    assert!(
        root.poll_native_controller_query(binding, &request)
            .is_err()
    );
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.cleanup_error.is_none());
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
    assert!(request.original_worker_released_on_control());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_controller_query_requires_original_worker_and_suppresses_late_close() {
    use crate::plugin_worker::NativeWorkerControllerProgress;
    use hex_plugin_host::protocol::ParameterPageQuery;
    if std::env::var_os("HEX_ROOT_CONTROLLER_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env("HEX_ROOT_CONTROLLER_DRIVER", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "91")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    assert!(
        root.begin_native_controller_query(binding, ParameterPageQuery::default())
            .is_err()
    );
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    assert!(
        root.begin_native_controller_query(binding, ParameterPageQuery::default())
            .is_err()
    );
    root.start_native_effect(binding).unwrap();
    assert!(
        root.begin_native_controller_query(
            Binding {
                epoch: binding.epoch + 1,
                ..binding
            },
            ParameterPageQuery::default()
        )
        .is_err()
    );
    {
        let _work = root.work.try_lock().unwrap();
        assert!(matches!(
            root.begin_native_controller_query(binding, ParameterPageQuery::default()),
            Err("native preparation is busy")
        ));
    }
    let request = root
        .begin_native_controller_query(binding, ParameterPageQuery::default())
        .unwrap();
    assert!(
        root.begin_native_controller_query(binding, ParameterPageQuery::default())
            .is_err()
    );
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let result = root.controller_query_receipt(binding, &request, || root.close());
        match result {
            Ok(NativeWorkerControllerProgress::Discarded) => break,
            Ok(NativeWorkerControllerProgress::Pending) => {}
            _ => panic!("close must discard the consumed controller data"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    assert!(
        root.poll_native_controller_query(binding, &request)
            .is_err()
    );
    drop(request);
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired) if retired.status.success())
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_live_capture_refuses_pending_stale_and_close_after_result() {
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    assert!(root.begin_live_native_capture(binding).is_err());
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    assert!(
        root.begin_live_native_capture(binding).is_err(),
        "Prepared has no joining worker"
    );
    root.start_native_effect(binding).unwrap();
    assert!(
        root.begin_live_native_capture(Binding {
            epoch: binding.epoch + 1,
            ..binding
        })
        .is_err()
    );
    {
        let _work = root.work.try_lock().unwrap();
        assert!(matches!(
            root.begin_live_native_capture(binding),
            Err("native preparation is busy")
        ));
    }
    let request = root.begin_live_native_capture(binding).unwrap();
    assert!(root.begin_live_native_capture(binding).is_err());
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let result = root.with_live_capture(binding, |owner| {
            let Owner::Effect(owner) = owner else {
                panic!("original unsplit effect");
            };
            let result = owner.poll_state_capture(&request)?;
            if matches!(result, NativeWorkerCaptureProgress::Captured(_)) {
                root.close();
            }
            Ok(result)
        });
        match result {
            Err("native capture source is closing") => break,
            Ok(NativeWorkerCaptureProgress::Pending) => {
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
            _ => panic!("close must suppress the actual completed result"),
        }
    }
    assert!(root.poll_live_native_capture(binding, &request).is_err());
    drop(request);
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired) if retired.status.success())
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_live_capture_sdk_refusal_retains_confirmed_state_and_disposal() {
    if std::env::var_os("HEX_ROOT_CAPTURE_REFUSAL_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env("HEX_ROOT_CAPTURE_REFUSAL_DRIVER", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "69")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let confirmed = root.native_source_prepared_state(binding).unwrap();
    let request = root.begin_live_native_capture(binding).unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match root.poll_live_native_capture(binding, &request) {
            Ok(NativeWorkerCaptureProgress::Failed) => break,
            Ok(NativeWorkerCaptureProgress::Pending) | Err("native preparation is busy") => {
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
            _ => panic!("SDK refusal must not provide captured state"),
        }
    }
    assert_eq!(root.native_source_prepared_state(binding), Some(confirmed));
    drop(request);
    assert!(root.retire_native_source(binding));
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(matches!(
        returned.error,
        Some(hex_plugin_host::Error::Worker(6))
    ));
    assert!(
        matches!(returned.disposal, PreparationDisposal::Reaped(retired) if !retired.status.success() && retired.terminal.is_none())
    );
}
