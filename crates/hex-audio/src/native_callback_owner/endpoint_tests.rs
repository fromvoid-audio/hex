//! Actual endpoint Boxes, original runtime and the existing lifecycle executor.
use super::*;
use crate::prepared::{self, PublicationPolicy};
use std::sync::atomic::Ordering::{Acquire, SeqCst};
use std::time::{Duration, Instant};
fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "endpoint lifecycle did not progress"
        );
        std::thread::yield_now();
    }
}
fn limits(
    owner: &mut hex_engine::AudioCandidateOwner,
) -> crate::AudioCandidatePublicationReservation {
    let ledger = hex_engine::AudioStorageLedgerReservation {
        data_rows: 4096,
        handle_rows: 2048,
        root_rows: 1,
        max_retained_bytes: 96 * 1024 * 1024,
        max_construction_peak_bytes: 96 * 1024 * 1024,
    };
    let summary = owner.seal_constructor_on_control(ledger).unwrap();
    crate::AudioCandidatePublicationReservation {
        runtime_slots: 3,
        archive_slots: 3,
        member_rows: summary.member_rows * 3,
        transfer_rows: summary.transfer_rows * 3,
        receipt_rows: 4,
        data_rows: ledger.data_rows,
        handle_rows: ledger.handle_rows,
        max_retained_data_bytes: ledger.max_retained_bytes,
        max_construction_peak_data_bytes: ledger.max_construction_peak_bytes,
    }
}
#[test]
fn actual_four_terminal_factory_cleans_graph_without_client_poll_but_keeps_lease_until_client_drop()
{
    let _serial = crate::prepared::candidate_tests::serial();
    let ui = std::thread::current().id();
    for error_first in [false, true] {
        let Some(mut current) = crate::prepared::candidate_tests::candidate() else {
            return;
        };
        let records = Arc::new(Mutex::new(Vec::new()));
        let reservation = limits(&mut current);
        let lease = crate::lifecycle::Lease::acquire().unwrap();
        let initial = crate::spectrum_worker::SpectrumStamp {
            target: crate::spectrum_worker::SpectrumTargetId(0),
            generation: 0,
            sample_rate_hz: 48000,
            applied_revision: 0,
        };
        let (capture, reader, reaper) =
            crate::spectrum_worker::spawn_native_spectrum_worker(initial).unwrap();
        let spectrum_bytes = capture.native_receipt().unwrap().storage.data_bytes;
        let (control, callback) = prepared::channel_with_spectrum(48_000, Some(capture)).unwrap();
        let root = control.native_candidate_root();
        let (mut data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, Some(reader));
        let client_address = client.owner_ref().unwrap() as *const _ as usize;
        let publication_address = publication.owner_ref().unwrap() as *const _ as usize;
        let observed = Arc::clone(&records);
        client.test_drop_probe(move || {
            observed
                .lock()
                .unwrap()
                .push(("client", client_address, std::thread::current().id()))
        });
        let observed = Arc::clone(&records);
        publication.test_drop_probe(move || {
            observed.lock().unwrap().push((
                "publication",
                publication_address,
                std::thread::current().id(),
            ))
        });
        let native = NativeRetirement::new(terminals, root.clone());
        let stream = crate::lifecycle::StreamOwner::prepared_custody(lease, Some(reaper), native);
        let control = publication.control().unwrap();
        control.enable_native_custody().unwrap();
        let native_bytes = control
            .native_candidate_root()
            .native_boundary_bytes()
            .unwrap();
        let mut too_small = reservation;
        too_small.max_retained_data_bytes = native_bytes.checked_sub(spectrum_bytes).unwrap();
        too_small.max_construction_peak_data_bytes = too_small.max_retained_data_bytes;
        assert!(
            control
                .configure_audio_candidate_publication(too_small)
                .is_err()
        );
        assert_eq!(
            current.phase(),
            hex_engine::AudioCandidateDisposalPhase::Owned
        );
        control
            .configure_audio_candidate_publication(reservation)
            .unwrap();
        client.activate();
        // Once exposed, even an unused Resident client cannot be converted.
        assert!(
            control
                .configure_audio_candidate_publication(reservation)
                .is_err()
        );
        let _ticket = control
            .stage_eligible_audio_candidate(current, vec![], PublicationPolicy::InheritRuntime, 1)
            .ok()
            .unwrap();
        assert!(client.preview_request().is_err());
        let root_identity = Arc::new(hex_engine::Project::new(48000, vec![]).unwrap());
        let rejected = client
            .forward_resident_publication(
                hex_engine::playback::Playback::from_shared(Arc::clone(&root_identity)),
                PublicationPolicy::InheritRuntime,
                1,
            )
            .unwrap_err();
        assert_eq!(rejected.reason, "resident publication unavailable");
        assert_eq!(Arc::strong_count(&root_identity), 2);
        drop(rejected);
        assert_eq!(Arc::strong_count(&root_identity), 1);
        client.command(hex_engine::PlaybackCommand::Play).unwrap();
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.begin_block();
                let mut output = [[0.0; 2]; 8];
                callback.render(&mut output).unwrap();
                assert!(output.iter().any(|frame| frame[0] != 0.0));
            })
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(client.status().unwrap().applied_revision.load(Acquire), 1);
        publication.close();
        drop(stream); // actual singleton lifecycle owns retirement and lease
        if error_first {
            drop(errors);
            drop(data);
        } else {
            drop(data);
            drop(errors);
        }
        // No UI reclaim/recover/advance calls: original graph disposal is autonomous.
        wait(|| root.test_eligible_complete());
        assert!(crate::lifecycle::Lease::acquire().is_err());
        assert!(
            !records
                .lock()
                .unwrap()
                .iter()
                .any(|(kind, address, _)| *kind == "client" && *address == client_address)
        );
        let measured = allocation_counter::measure(|| {
            client.close();
            client.close();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(client.status().is_none());
        let mut replacement = None;
        wait(|| {
            replacement = crate::lifecycle::Lease::acquire().ok();
            replacement.is_some()
        });
        let records = records.lock().unwrap();
        for (kind, address) in [
            ("client", client_address),
            ("publication", publication_address),
        ] {
            let found = records
                .iter()
                .filter(|(k, a, _)| *k == kind && *a == address)
                .collect::<Vec<_>>();
            assert_eq!(found.len(), 1);
            assert_ne!(found[0].2, ui);
        }
        drop(records);
        drop(replacement);
        // Keep the inert token alive until AFTER the replacement lease exists.
        drop(client);
        drop(publication);
    }
}

#[test]
fn worker_and_client_endpoints_return_in_either_order_and_unexposed_open_abandonment_is_inert() {
    let _serial = crate::prepared::candidate_tests::serial();
    for client_first in [false, true] {
        let (control, callback) = prepared::channel(48_000).unwrap();
        let root = control.native_candidate_root();
        let (data, errors, terminals, client, publication) =
            reserve_native_worker(control, callback, None);
        let mut client = Some(client);
        let mut publication = Some(publication);
        let mut retirement = NativeRetirement::new(terminals, root.clone());
        publication
            .as_mut()
            .unwrap()
            .control()
            .unwrap()
            .enable_native_custody()
            .unwrap();
        drop(data);
        drop(errors);
        if client_first {
            drop(client.take());
        } else {
            drop(publication.take());
        }
        assert!(!retirement.advance().unwrap());
        if client_first {
            drop(publication.take());
        } else {
            drop(client.take());
        }
        crate::prepared::candidate_tests::finish(&mut retirement);
    }
}
#[test]
fn publication_destructor_failure_keeps_other_actual_endpoint_and_first_payload_without_retry() {
    let _serial = crate::prepared::candidate_tests::serial();
    let (control, callback) = prepared::channel(48_000).unwrap();
    let root = control.native_candidate_root();
    let (data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    publication
        .control()
        .unwrap()
        .enable_native_custody()
        .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    publication.test_drop_probe(move || {
        count.fetch_add(1, SeqCst);
        std::panic::panic_any(1234_u64);
    });
    publication.close();
    client.close();
    drop(data);
    drop(errors);
    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| retirement.advance()))
        .err()
        .unwrap();
    assert_eq!(first.downcast_ref::<u64>(), Some(&1234));
    assert_eq!(calls.load(SeqCst), 1);
    // All returned Boxes are secured before any destructor can unwind.
    assert!(retirement.client.is_some());
    assert!(retirement.advance().is_err());
    assert_eq!(calls.load(SeqCst), 1);
    assert!(retirement.data.is_some()); // exact callback Boxes retained in persistent custody
    // Fixture teardown is explicit and never asserted as successful disposal.
    wait(|| !retirement.root.cleanup_scheduled());
    drop(first);
}

#[test]
fn exposed_resident_client_permanently_refuses_later_constructor_conversion() {
    let _serial = crate::prepared::candidate_tests::serial();
    let Some(mut owner) = crate::prepared::candidate_tests::candidate() else {
        return;
    };
    let limits = limits(&mut owner);
    let (control, callback) = prepared::channel(48_000).unwrap();
    let root = control.native_candidate_root();
    let (data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    let control = publication.control().unwrap();
    control.enable_native_custody().unwrap();
    client.activate(); // still Resident, no graph and no preview request
    assert!(
        control
            .configure_audio_candidate_publication(limits)
            .is_err()
    );
    assert!(
        control
            .configure_audio_candidate_domains(crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 96 * 1024 * 1024,
                max_construction_peak_data_bytes: 96 * 1024 * 1024,
            })
            .is_err()
    );
    assert!(control.configure_audio_candidate_streaming(8).is_err());
    assert!(client.preview_request().is_ok()); // failed conversion preserves Resident
    client.close();
    publication.close();
    drop(data);
    drop(errors);
    crate::prepared::candidate_tests::finish(&mut retirement);
}

#[test]
fn unrelated_spectrum_reader_or_reaper_cannot_cover_the_actual_capture_constructor() {
    let _serial = crate::prepared::candidate_tests::serial();
    let Some(mut owner) = crate::prepared::candidate_tests::candidate() else {
        return;
    };
    let reservation = limits(&mut owner);
    for wrong_reader in [false, true] {
        let initial = crate::spectrum_worker::SpectrumStamp {
            target: crate::spectrum_worker::SpectrumTargetId(0),
            generation: 0,
            sample_rate_hz: 48000,
            applied_revision: 0,
        };
        let (capture, reader, reaper) =
            crate::spectrum_worker::spawn_native_spectrum_worker(initial).unwrap();
        let (other_capture, other_reader, other_reaper) =
            crate::spectrum_worker::spawn_native_spectrum_worker(initial).unwrap();
        let (reader, unused_reader) = if wrong_reader {
            (other_reader, reader)
        } else {
            (reader, other_reader)
        };
        let (control, callback) = prepared::channel_with_spectrum(48000, Some(capture)).unwrap();
        let root = control.native_candidate_root();
        let (data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, Some(reader));
        let mut retirement = NativeRetirement::new(terminals, root.clone());
        // Wrong Reader is refused at the four-terminal constructor; wrong
        // Reaper is refused when persistent lifecycle custody is paired.
        retirement.bind_spectrum_reaper(Some(if wrong_reader { &reaper } else { &other_reaper }));
        let control = publication.control().unwrap();
        control.enable_native_custody().unwrap();
        assert!(root.native_boundary_bytes().is_none());
        assert!(
            control
                .configure_audio_candidate_publication(reservation)
                .is_err()
        );
        assert_eq!(
            owner.phase(),
            hex_engine::AudioCandidateDisposalPhase::Owned
        );
        assert!(client.preview_request().is_ok());
        client.close();
        publication.close();
        drop(data);
        drop(errors);
        drop(other_capture);
        drop(unused_reader);
        reaper.shutdown_and_join().unwrap();
        other_reaper.shutdown_and_join().unwrap();
        crate::prepared::candidate_tests::finish(&mut retirement);
    }
}
