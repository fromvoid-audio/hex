use super::super::endpoints::NativeEndpointBank;
use super::super::tests::{dispose, identity, input, root, source_reservation, start};
use super::*;
use hex_plugin_host::protocol::ParameterEditKind;
use std::time::{Duration, Instant};

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn refused_unpublished_source_can_release_its_unclaimed_reservation() {
    use crate::candidate_custody::{AudioCandidateDomainReservation, CandidateRoot};
    crate::lifecycle::prepare_candidate_cleanup().unwrap();
    let outer = CandidateRoot::new(2).unwrap();
    outer
        .configure_domains(AudioCandidateDomainReservation {
            max_retained_data_bytes: 64 * 1024 * 1024,
            max_construction_peak_data_bytes: 64 * 1024 * 1024,
        })
        .unwrap();
    outer
        .configure_native_source_preparation(1, source_reservation())
        .unwrap();
    let root = &outer.0.eligible;
    let binding = start(root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let mut lease = RetiringEditLease::reserve(&outer, binding).unwrap();
    assert_eq!(
        outer.quiesce_native_source_preparation(binding, binding.graph, None),
        Err("native epoch requires its original editor receipt reservation")
    );
    let stale = Binding {
        epoch: binding.epoch + 1,
        ..binding
    };
    assert_eq!(
        outer.quiesce_native_source_preparation(stale, stale.graph, Some(&mut lease)),
        Err("native epoch editor receipt reservation differs")
    );
    assert_eq!(
        lease.release_unaccepted(),
        Err("native unaccepted source is not retiring")
    );
    // Cancellation can reach Root before the desktop owner disposes its
    // never-published candidate and its unclaimed reservation.
    assert!(root.retire_native_source(binding));
    let work = lock(&root.work);
    assert_eq!(
        lease.release_unaccepted(),
        Err("native preparation is busy")
    );
    drop(work);
    lease.release_unaccepted().unwrap();
    assert_eq!(
        lease.release_unaccepted(),
        Err("native retiring edit reservation was already claimed")
    );
    drop(lease);
    root.advance_on_control(false);
    assert!(
        !lock(&root.bank).closing,
        "unpublished refusal closed the audio root"
    );
    dispose(root, binding);
    let deadline = Instant::now() + Duration::from_millis(500);
    let returned = loop {
        if let Some(returned) = root.recover_native_source(binding) {
            break returned;
        }
        assert!(Instant::now() < deadline, "unpublished return stayed busy");
        std::thread::yield_now();
    };
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());

    // Output relinquishment has no PublicationControl to request the
    // candidate's retirement; its retained lease must initiate it.
    let binding = start(root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let mut closed = RetiringEditLease::reserve(&outer, binding).unwrap();
    let renewed = outer
        .quiesce_native_source_preparation(binding, binding.graph, Some(&mut closed))
        .unwrap();
    assert!(renewed.epoch > binding.epoch);
    assert!(root.native_source_progress(binding).is_none());
    assert_eq!(closed.binding, renewed);
    let binding = renewed;
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match closed.cancel_unaccepted() {
            Ok(()) => break,
            Err("native preparation is busy" | "native unaccepted source is not retiring") => {}
            Err(reason) => panic!("closed output cancellation failed: {reason}"),
        }
        assert!(
            Instant::now() < deadline,
            "closed output cancellation stayed busy"
        );
        std::thread::yield_now();
    }
    drop(closed);
    dispose(root, binding);
    let deadline = Instant::now() + Duration::from_millis(500);
    let returned = loop {
        if let Some(returned) = root.recover_native_source(binding) {
            break returned;
        }
        assert!(
            Instant::now() < deadline,
            "closed output return stayed busy"
        );
        std::thread::yield_now();
    };
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());

    let binding = start(root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let mut claimed = RetiringEditLease::reserve(&outer, binding).unwrap();
    claimed.claim(None).unwrap();
    assert_eq!(
        claimed.release_unaccepted(),
        Err("native retiring edit reservation was already claimed")
    );
    assert_eq!(
        claimed.cancel_unaccepted(),
        Err("native retiring edit reservation was already claimed")
    );
    assert!(root.retire_native_source(binding));
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match claimed.poll() {
            Ok(Progress::Pending) | Err("native preparation is busy") => {}
            Ok(Progress::Batch(receipt, _)) => claimed.acknowledge(receipt).unwrap(),
            Ok(Progress::Complete) => break,
            Ok(Progress::Disposition(_)) => panic!("unexpected retained edit sequence"),
            Err(reason) => panic!("claimed source did not drain: {reason}"),
        }
        assert!(Instant::now() < deadline, "claimed source did not complete");
        std::thread::yield_now();
    }
    drop(claimed);
    dispose(root, binding);
    let deadline = Instant::now() + Duration::from_millis(500);
    let returned = loop {
        if let Some(returned) = root.recover_native_source(binding) {
            break returned;
        }
        assert!(Instant::now() < deadline, "claimed return stayed busy");
        std::thread::yield_now();
    };
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn shutdown_retains_every_pending_page_and_honors_only_exact_disposition() {
    const CHILD: &str = "HEX_SHUTDOWN_EDITOR_RECEIPTS";
    let mode = match std::env::var(CHILD) {
        Ok(mode) => mode,
        Err(_) => {
            for mode in ["empty", "backlog", "unclaimed", "acknowledged", "stale_ack"] {
                let path = std::env::temp_dir()
                    .join(format!("hex-editor-shutdown-{}-{mode}", std::process::id()));
                std::fs::create_dir(&path).unwrap();
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        std::thread::current().name().unwrap(),
                        "--ignored",
                        "--nocapture",
                    ])
                    .env(CHILD, mode)
                    .env("HEX_ORIGINAL_INSTANCE_VARIANT", "116")
                    .env("HEX_ORIGINAL_INSTANCE_RELEASE", path.join("release"))
                    .env("HEX_ORIGINAL_INSTANCE_TRACE", path.join("trace"))
                    .output()
                    .unwrap();
                std::fs::remove_dir_all(path).unwrap();
                assert!(
                    output.status.success()
                        && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                    "{mode}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            return;
        }
    };

    use crate::candidate_custody::{AudioCandidateDomainReservation, CandidateRoot};
    let outer = CandidateRoot::new(2).unwrap();
    outer
        .configure_domains(AudioCandidateDomainReservation {
            max_retained_data_bytes: 64 * 1024 * 1024,
            max_construction_peak_data_bytes: 64 * 1024 * 1024,
        })
        .unwrap();
    outer
        .configure_native_source_preparation(1, source_reservation())
        .unwrap();
    let root = &outer.0.eligible;
    let binding = start(root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let endpoint = root.loan_native_endpoint(binding).unwrap();
    let mut bank = NativeEndpointBank::new().unwrap();
    assert!(bank.install(binding.root, endpoint).is_ok());
    let mut lease = RetiringEditLease::reserve(&outer, binding).unwrap();
    if mode != "unclaimed" {
        lease.claim(None).unwrap();
    }
    let mut lease = Some(lease);
    if mode != "empty" {
        std::fs::write(
            std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
            b"emit",
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while std::fs::read(std::env::var_os("HEX_ORIGINAL_INSTANCE_TRACE").unwrap())
            .ok()
            .is_none_or(|trace| trace.get(..4) != Some(&25_i32.to_le_bytes()))
        {
            assert!(
                Instant::now() < deadline,
                "native fixture did not accept the edits"
            );
            std::thread::yield_now();
        }
    }

    root.close();
    let has_project_disposition = matches!(mode.as_str(), "acknowledged" | "stale_ack");
    if !has_project_disposition {
        lease.take().unwrap().handoff_on_shutdown(true, None, None);
    }
    root.advance_on_control(false);
    assert_eq!(
        root.native_source_progress(binding),
        Some(NativeSourcePreparationProgress::Retiring)
    );
    assert!(root.recover_native_source(binding).is_none());
    assert!(!root.cleanup_failed());

    #[cfg(feature = "allocation-probe")]
    let mut returned = None;
    #[cfg(feature = "allocation-probe")]
    let allocation = allocation_counter::measure(|| {
        bank.request_epoch_return();
        returned = bank.take_requested(0);
    });
    #[cfg(feature = "allocation-probe")]
    {
        assert_eq!(allocation.count_total, 0);
        assert_eq!(allocation.bytes_total, 0);
        assert_eq!(allocation.count_current, 0);
        assert_eq!(allocation.bytes_current, 0);
    }
    #[cfg(not(feature = "allocation-probe"))]
    let returned = {
        bank.request_epoch_return();
        bank.take_requested(0)
    };
    assert!(root.ingest_native_endpoint(returned.unwrap()).is_ok());

    if has_project_disposition {
        let mut lease = lease.take().unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let accepted = loop {
            match lease.poll() {
                Ok(Progress::Batch(receipt, _)) => lease.retain_page(receipt).unwrap(),
                Ok(Progress::Disposition(receipt)) => break receipt,
                Ok(Progress::Pending)
                | Err(
                    "native preparation is busy"
                    | "native controller mailbox is busy"
                    | "native edit observation is busy",
                ) => {}
                _ => panic!("original receipt did not reach project disposition"),
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_ordered_shutdown_receipts(&lease.retained_sequence(accepted).unwrap());
        let accepted = if mode == "stale_ack" {
            Receipt {
                ordinal: accepted.ordinal - 1,
                ..accepted
            }
        } else {
            accepted
        };
        lease.handoff_on_shutdown(true, None, Some(accepted));
    }

    if matches!(mode.as_str(), "empty" | "acknowledged") {
        dispose(root, binding);
        let returned = root.recover_native_source(binding).unwrap();
        assert_eq!(returned.binding, binding);
        assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    } else {
        let deadline = Instant::now() + Duration::from_secs(1);
        while root.native_source_progress(binding) != Some(NativeSourcePreparationProgress::Failed)
        {
            root.advance_on_control(false);
            assert!(
                Instant::now() < deadline,
                "original nonempty receipt was not retained"
            );
            std::thread::yield_now();
        }
        assert!(root.recover_native_source(binding).is_none());
        let status = crate::lifecycle::LifecycleStatus::new();
        assert!(!crate::lifecycle::finish_cleanup_observed(&outer, &status));
        assert_eq!(
            status.failure(),
            Some(crate::AudioLifecycleFailure::RetiringEditor)
        );
        let work = lock(&root.work);
        let cell = &work.sources.cells[0];
        let drain = cell.retiring_edits.as_ref().unwrap();
        assert_ordered_shutdown_receipts(&drain.retained);
        assert!(matches!(drain.stage, Stage::Disposition));
        assert!(drain.ordinal > 2 && drain.delivery.is_none());
        let expected = if mode == "stale_ack" {
            "native retiring edit disposition differs"
        } else {
            "vst3 editor receipts need project disposition after shutdown"
        };
        assert!(matches!(
            cell.cleanup_error, Some(hex_plugin_host::Error::Protocol(reason)) if reason == expected
        ));
        assert!(root.cleanup_failed());
    }
}

fn assert_ordered_shutdown_receipts(edits: &[ParameterEdit]) {
    assert_eq!(edits.len(), 14);
    for (index, edit) in edits.iter().enumerate() {
        assert_eq!(
            (edit.sequence, edit.gesture, edit.id),
            (index as u64 + 1, 1, 7)
        );
        match index {
            0 => assert_eq!(edit.kind, ParameterEditKind::Begin),
            13 => assert_eq!(edit.kind, ParameterEditKind::End),
            _ => {
                assert_eq!(edit.kind, ParameterEditKind::Perform);
                assert_eq!(edit.normalized, 0.75);
            }
        }
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn sealed_retirement_retains_every_batch_until_acknowledged() {
    retirement_case(false, false, false, false, false, false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn admitted_read_precedes_the_sealed_retirement_backlog() {
    retirement_case(true, false, false, false, false, false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn active_gesture_refuses_seal_until_its_real_end() {
    retirement_case(false, true, false, false, false, false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn retiring_source_delivers_edits_before_its_endpoint_returns() {
    retirement_case(false, false, true, false, false, false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn live_reader_shares_only_guarded_detach_before_retirement() {
    retirement_case(false, false, true, true, false, false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn active_gesture_drains_while_guarded_detach_waits() {
    retirement_case(false, true, true, true, false, false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn grouped_editor_receipts_survive_live_detach_and_multiple_batches() {
    retirement_case(false, true, true, true, true, false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn acknowledged_live_latency_remains_pending_without_blocking_the_reader() {
    const CHILD: &str = "HEX_RETIRING_LATENCY_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let path =
            std::env::temp_dir().join(format!("hex-retiring-latency-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "105")
            .env("HEX_ORIGINAL_INSTANCE_RELEASE", path.join("release"))
            .env("HEX_ORIGINAL_INSTANCE_TRACE", path.join("trace"))
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(path);
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
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
    let endpoint = root.loan_native_endpoint(binding).unwrap();
    let mut bank = NativeEndpointBank::new().unwrap();
    assert!(bank.install(binding.root, endpoint).is_ok());
    root.reserve_retiring_edits(binding).unwrap();
    root.claim_retiring_edits(binding, None).unwrap();
    let until = Instant::now() + Duration::from_millis(850);
    assert!(matches!(
        root.poll_retiring_edits(binding),
        Ok(Progress::Pending)
    ));
    // Finish the first recovery read before requesting the real SDK
    // restart; the helper has then explicitly enabled latency observation.
    loop {
        match root.poll_retiring_edits(binding) {
            Ok(Progress::Pending)
            | Err("native preparation is busy" | "native edit observation is busy") => {}
            _ => panic!("unexpected startup receipt"),
        }
        let idle = root.work.try_lock().ok().is_some_and(|work| {
            matches!(
                work.sources.cells[0]
                    .retiring_edits
                    .as_ref()
                    .map(|drain| &drain.stage),
                Some(Stage::Live(None))
            )
        });
        if idle {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    std::fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"request latency",
    )
    .unwrap();
    let ordinal = loop {
        match root.poll_retiring_edits(binding) {
            Ok(Progress::Batch(receipt, batch)) => {
                let ordinal = batch.pending_latency_restart.expect("SDK latency ordinal");
                assert!(
                    matches!(root.poll_retiring_edits(binding), Ok(Progress::Batch(repeated, _)) if repeated == receipt)
                );
                root.acknowledge_retiring_edits(binding, receipt).unwrap();
                break ordinal;
            }
            Ok(Progress::Pending)
            | Err("native preparation is busy" | "native edit observation is busy") => {}
            _ => panic!("latency observation failed"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    let mut cycles = 0;
    while cycles < 2 {
        match root.poll_retiring_edits(binding) {
            Ok(Progress::Pending)
            | Err("native preparation is busy" | "native edit observation is busy") => {}
            Ok(Progress::Batch(_, _)) => panic!("the same latency ordinal was redelivered"),
            _ => panic!("live latency reader failed"),
        }
        let state = root.work.try_lock().ok().and_then(|work| {
            let drain = work.sources.cells[0].retiring_edits.as_ref()?;
            Some((
                matches!(drain.stage, Stage::Live(None)),
                drain.accepted_latency,
            ))
        });
        if state == Some((true, Some(ordinal))) {
            cycles += 1;
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    root.close();
    bank.request_epoch_return();
    assert!(
        root.ingest_native_endpoint(bank.take_requested(0).unwrap())
            .is_ok()
    );
    loop {
        match root.poll_retiring_edits(binding) {
            Ok(Progress::Complete) => break,
            Ok(Progress::Pending)
            | Err(
                "native preparation is busy"
                | "native edit observation is busy"
                | "native controller mailbox is busy",
            ) => {}
            _ => panic!("latency source did not retire"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    dispose(&root, binding);
    assert!(
        root.recover_native_source(binding)
            .unwrap()
            .cleanup_error
            .is_none()
    );
}

#[test]
#[ignore = "explicit signed SDK fixture required"]
fn source_keeps_all_pages_until_sealed_project_disposition() {
    retirement_case(true, false, true, false, false, true);
}

fn retirement_case(
    prior: bool,
    active_gesture: bool,
    live: bool,
    preclaim: bool,
    grouped: bool,
    retain: bool,
) {
    const CHILD: &str = "HEX_RETIRING_EDITOR_DRAIN_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let path = std::env::temp_dir().join(format!("hex-retiring-editor-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env(
                "HEX_ORIGINAL_INSTANCE_VARIANT",
                if grouped {
                    "148"
                } else if active_gesture {
                    "147"
                } else {
                    "116"
                },
            )
            .env("HEX_ORIGINAL_GROUPED_EDITOR_PATTERN", "repeat")
            .env("HEX_ORIGINAL_INSTANCE_RELEASE", path.join("release"))
            .env("HEX_ORIGINAL_INSTANCE_TRACE", path.join("trace"))
            .env("HEX_ORIGINAL_EDITOR_END_RELEASE", path.join("end"))
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(path);
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = if grouped {
        let (build, source, preparation) = input();
        root.begin_native_source(
            identity(),
            12,
            &build,
            source.with_grouped_editor_callbacks(),
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
            None,
        )
        .unwrap()
    } else {
        start(&root)
    };
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let endpoint = root.loan_native_endpoint(binding).unwrap();
    let mut bank = NativeEndpointBank::new().unwrap();
    assert!(bank.install(binding.root, endpoint).is_ok());
    root.reserve_retiring_edits(binding).unwrap();
    std::fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"emit",
    )
    .unwrap();
    let until = Instant::now() + Duration::from_millis(800);
    while std::fs::read(std::env::var_os("HEX_ORIGINAL_INSTANCE_TRACE").unwrap())
        .ok()
        .is_none_or(|trace| trace.get(..4) != Some(&25_i32.to_le_bytes()))
    {
        assert!(Instant::now() < until, "SDK editor burst did not complete");
        std::thread::yield_now();
    }
    if prior {
        assert!(root.native_recovery_edits(binding, true).unwrap().is_none());
        loop {
            let ready = root.with_live_capture(binding, |owner| {
                Ok(match owner {
                    Owner::Effect(owner) => owner.pending_edit_result_nonempty_for_test(),
                    Owner::Loaned(owner) => owner.pending_edit_result_nonempty_for_test(),
                    _ => None,
                })
            });
            match ready {
                Ok(Some(true)) => break,
                Ok(Some(false)) => {
                    assert!(
                        root.native_recovery_edits(binding, true)
                            .unwrap()
                            .unwrap()
                            .edits
                            .as_slice()
                            .is_empty()
                    );
                    assert!(root.native_recovery_edits(binding, true).unwrap().is_none());
                }
                Ok(None) | Err("native preparation is busy") => {}
                Err(reason) => panic!("receipt observation failed: {reason}"),
            }
            assert!(Instant::now() < until, "admitted SDK read did not complete");
            std::thread::yield_now();
        }
    }
    let previous_query = if preclaim {
        Some(
            root.begin_native_controller_query(
                binding,
                hex_plugin_host::protocol::ParameterPageQuery::default(),
            )
            .unwrap(),
        )
    } else {
        None
    };
    if !preclaim {
        root.close();
    }
    root.claim_retiring_edits(binding, prior.then_some(true))
        .unwrap();
    if preclaim {
        assert!(root.native_parameter_edits(binding).is_err());
        assert!(
            root.begin_native_controller_query(
                binding,
                hex_plugin_host::protocol::ParameterPageQuery::default()
            )
            .is_err()
        );
        let previous_query = previous_query.as_ref().unwrap();
        loop {
            match root.poll_native_controller_query(binding, previous_query) {
                Ok(NativeWorkerControllerProgress::Page(_)) => break,
                Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy") => {
                }
                _ => panic!("admitted controller reply was lost"),
            }
            assert!(
                Instant::now() < until,
                "admitted controller query timed out"
            );
            std::thread::yield_now();
        }
        let query = root
            .begin_native_controller_query(binding, ControllerRequest::DetachEditorWhenIdle)
            .unwrap();
        loop {
            match root.poll_native_controller_query(binding, &query) {
                Ok(NativeWorkerControllerProgress::Editor(PluginEditorState::Closed))
                    if !active_gesture =>
                {
                    break;
                }
                Ok(NativeWorkerControllerProgress::Editor(PluginEditorState::GestureActive))
                    if active_gesture =>
                {
                    break;
                }
                Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy") => {
                }
                _ => panic!("guarded controller query did not finish"),
            }
            assert!(Instant::now() < until, "guarded controller query timed out");
            std::thread::yield_now();
        }
    }
    let mut edits = Vec::new();
    let mut batches = 0;
    if live {
        loop {
            match root.poll_retiring_edits(binding) {
                Ok(Progress::Batch(receipt, batch)) => {
                    assert!(!batch.edits.as_slice().is_empty());
                    assert!(
                        matches!(root.poll_retiring_edits(binding), Ok(Progress::Batch(repeated, _)) if repeated == receipt)
                    );
                    edits.extend_from_slice(batch.edits.as_slice());
                    if retain {
                        root.retain_retiring_edit_page(binding, receipt).unwrap();
                    } else {
                        root.acknowledge_retiring_edits(binding, receipt).unwrap();
                    }
                    batches += 1;
                    break;
                }
                Ok(Progress::Pending)
                | Err("native preparation is busy" | "native edit observation is busy") => {}
                _ => panic!("live retiring source did not retain its batch"),
            }
            assert!(
                Instant::now() < until,
                "live retiring source did not deliver"
            );
            std::thread::yield_now();
        }
        assert!(root.recover_native_source(binding).is_none());
        if preclaim && active_gesture {
            std::fs::write(
                std::env::var_os("HEX_ORIGINAL_EDITOR_END_RELEASE").unwrap(),
                b"end",
            )
            .unwrap();
            loop {
                let query = root
                    .begin_native_controller_query(binding, ControllerRequest::DetachEditorWhenIdle)
                    .unwrap();
                let state = loop {
                    match root.poll_native_controller_query(binding, &query) {
                        Ok(NativeWorkerControllerProgress::Editor(state)) => break state,
                        Ok(NativeWorkerControllerProgress::Pending)
                        | Err("native preparation is busy") => {}
                        _ => panic!("guarded detach failed after gesture end"),
                    }
                    assert!(Instant::now() < until);
                    std::thread::yield_now();
                };
                if state == PluginEditorState::Closed {
                    break;
                }
                assert_eq!(state, PluginEditorState::GestureActive);
                assert!(Instant::now() < until, "gesture did not end");
            }
        }
        loop {
            match root.poll_retiring_edits(binding) {
                Ok(Progress::Pending)
                | Err("native preparation is busy" | "native edit observation is busy") => {}
                Ok(Progress::Batch(receipt, batch)) => {
                    edits.extend_from_slice(batch.edits.as_slice());
                    if retain {
                        root.retain_retiring_edit_page(binding, receipt).unwrap();
                    } else {
                        root.acknowledge_retiring_edits(binding, receipt).unwrap();
                    }
                    batches += 1;
                }
                _ => panic!("live source lost custody before endpoint return"),
            }
            let pending = root.work.try_lock().ok().and_then(|work| {
                match work.sources.cells[0].owner.as_ref()? {
                    Owner::Loaned(owner) => owner.edit_read_mode().ok().flatten(),
                    _ => None,
                }
            });
            if pending == Some(true) {
                break;
            }
            assert!(Instant::now() < until, "live read was not admitted");
            std::thread::yield_now();
        }
    }
    if preclaim {
        root.close();
    }
    bank.request_epoch_return();
    assert!(
        root.ingest_native_endpoint(bank.take_requested(0).unwrap())
            .is_ok()
    );
    root.advance_on_control(false);
    assert!(root.recover_native_source(binding).is_none());
    let mut refused_active_gesture = active_gesture && preclaim;
    loop {
        match root.poll_retiring_edits(binding) {
            Ok(Progress::Pending)
            | Err(
                "native preparation is busy"
                | "native edit observation is busy"
                | "native controller mailbox is busy",
            ) => {}
            Ok(Progress::Batch(receipt, batch)) => {
                let again = root.poll_retiring_edits(binding).unwrap();
                let Progress::Batch(repeated, replay) = again else {
                    panic!("unacknowledged batch was not retained");
                };
                assert_eq!(repeated, receipt);
                assert_eq!(replay.edits.as_slice(), batch.edits.as_slice());
                assert_eq!(replay.edits.pending, batch.edits.pending);
                assert_eq!(
                    replay.pending_latency_restart,
                    batch.pending_latency_restart
                );
                let wrong = Receipt {
                    ordinal: receipt.ordinal + 1,
                    ..receipt
                };
                assert!(root.acknowledge_retiring_edits(binding, wrong).is_err());
                edits.extend_from_slice(batch.edits.as_slice());
                if retain {
                    root.retain_retiring_edit_page(binding, receipt).unwrap();
                } else {
                    root.acknowledge_retiring_edits(binding, receipt).unwrap();
                }
                assert!(root.acknowledge_retiring_edits(binding, receipt).is_err());
                batches += 1;
            }
            Ok(Progress::Disposition(receipt)) => {
                assert!(retain);
                assert!(batches > 1);
                assert_eq!(
                    root.retained_retiring_edit_sequence(binding, receipt)
                        .unwrap(),
                    edits
                );
                assert!(matches!(
                    root.poll_retiring_edits(binding),
                    Ok(Progress::Disposition(repeated)) if repeated == receipt
                ));
                assert!(root.recover_native_source(binding).is_none());
                let previous = Receipt {
                    ordinal: receipt.ordinal - 1,
                    ..receipt
                };
                assert!(root.acknowledge_retiring_edits(binding, previous).is_err());
                root.acknowledge_retiring_edits(binding, receipt).unwrap();
            }
            Ok(Progress::Complete) => break,
            Err("native editor gesture remains active") if active_gesture => {
                refused_active_gesture = true;
                std::fs::write(
                    std::env::var_os("HEX_ORIGINAL_EDITOR_END_RELEASE").unwrap(),
                    b"end",
                )
                .unwrap();
            }
            Err(reason) => panic!("retiring drain: {reason}"),
        }
        assert!(Instant::now() < until, "retiring drain did not finish");
        std::thread::yield_now();
    }
    if grouped {
        use ParameterEditKind::{Begin, End, GroupBegin, GroupEnd, Perform};
        assert!(
            refused_active_gesture,
            "a grouped gesture must refuse sealing"
        );
        assert!(
            batches > 1,
            "the grouped sequence must cross a batch boundary"
        );
        let kinds: Vec<_> = edits.iter().map(|edit| edit.kind).collect();
        assert_eq!(
            kinds,
            [
                GroupBegin, Begin, Begin, GroupEnd, GroupBegin, Perform, Perform, GroupEnd,
                GroupBegin, Perform, Perform, GroupEnd, GroupBegin, End, End, GroupEnd
            ]
        );
        assert!(
            edits
                .iter()
                .enumerate()
                .all(|(index, edit)| edit.sequence == index as u64 + 1)
        );
    } else if active_gesture {
        assert!(
            refused_active_gesture,
            "an active gesture must refuse sealing"
        );
        assert_eq!(edits.len(), 3);
    } else {
        assert!(batches > 1, "the SDK burst must cross a batch boundary");
        assert_eq!(edits.len(), 14);
    }
    if !grouped {
        for (index, edit) in edits.iter().enumerate() {
            assert_eq!(
                (edit.sequence, edit.gesture, edit.id),
                (index as u64 + 1, 1, 7)
            );
        }
        assert_eq!(edits.first().unwrap().kind, ParameterEditKind::Begin);
        assert_eq!(edits.last().unwrap().kind, ParameterEditKind::End);
    }
    dispose(&root, binding);
    assert!(
        root.recover_native_source(binding)
            .unwrap()
            .cleanup_error
            .is_none()
    );
}
