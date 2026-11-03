use super::*;
fn register(control: &mut Control) -> crate::AudioCandidateDomainToken {
    control
        .register_audio_candidate_domain_with_readiness(
            crate::prepared::candidate_tests::pair(),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .ok()
        .unwrap()
}
fn pool(
    control: &mut Control,
    token: crate::AudioCandidateDomainToken,
) -> hex_stream::PagePoolReport {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(token) {
            return report;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}
#[test]
fn one_actual_page_loan_uses_real_rings_and_retires_without_callback_allocation_or_join() {
    let _serial = serial();
    let Some(mut owner) = candidate() else { return };
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let token = register(&mut control);
    let original = crate::prepared::candidate_tests::pair();
    let refused = control
        .register_audio_candidate_domain_with_readiness(
            original,
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .err()
        .unwrap();
    assert_eq!(refused.reason, "candidate publication queue is full");
    assert!(
        refused
            .domain
            .0
            .shutdown(refused.domain.1)
            .unwrap()
            .accounting_complete
    );
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            assert_eq!(
                callback.domain_pages.iter().filter(|p| p.is_some()).count(),
                1
            );
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
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert!(control.retire_audio_candidate_domain(token));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let mut requested = false;
        data.owner_mut().unwrap().run(|callback, _| {
            requested = callback
                .domain_pages
                .iter()
                .flatten()
                .any(|pages| pages.return_requested());
        });
        if requested {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let measured = allocation_counter::measure(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
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
    let report = pool(&mut control, token);
    assert!(report.worker_joined && report.accounting_complete);
    assert_eq!(report.page_buffers, 4);
    while control
        .audio_candidate_domain_usage()
        .map(|usage| usage.occupied_domains)
        != Some(0)
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(owner);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
#[test]
fn final_page_loans_wait_for_both_guard_orders_completed_handle_and_normal_carrier_drop() {
    let _serial = serial();
    for errors_first in [false, true] {
        let Some(mut owner) = candidate() else { return };
        let limits = reservation(&mut owner);
        let (mut control, mut data, errors, mut retirement) = anchored();
        control
            .configure_audio_candidate_publication(limits)
            .unwrap();
        let token = register(&mut control);
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        if errors_first {
            drop(errors);
            assert!(!retirement.advance().unwrap());
            drop(data);
        } else {
            drop(data);
            assert!(!retirement.advance().unwrap());
            drop(errors);
        }
        assert!(!retirement.advance().unwrap()); // no completed-handle frontier
        assert_eq!(retirement.test_terminal_domain_count(), 0);
        assert!(control.recover_audio_candidate_domain(token).is_none());
        let mut backend = Some(());
        let mut pending = false;
        let mut disposed = false;
        let _ = crate::lifecycle::test_dispose_backend(
            &mut backend,
            &mut pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(()),
        )
        .unwrap();
        assert!(disposed && backend.is_none());
        assert_eq!(retirement.test_terminal_domain_count(), 0); // restored to exact control owner
        // Terminal drain already closed and retired this actual domain.
        let report = pool(&mut control, token);
        assert!(report.accounting_complete && report.worker_joined);
        drop(owner);
        drop(control);
        finish(&mut retirement);
    }
}

#[test]
fn extracted_page_loan_survives_carrier_destructor_panic_without_join_or_retry() {
    let _serial = serial();
    let Some(mut owner) = candidate() else { return };
    let limits = reservation(&mut owner);
    let (mut control, mut data, mut errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let token = register(&mut control);
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    errors.owner_mut().unwrap().test_drop_probe(move || {
        count.fetch_add(1, Relaxed);
        std::panic::panic_any("page carrier drop");
    });
    drop(data);
    drop(errors);
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::lifecycle::test_dispose_backend(
            &mut backend,
            &mut pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(()),
        )
    }))
    .err()
    .unwrap();
    assert!(first.downcast_ref::<&str>().is_some());
    assert_eq!(retirement.test_terminal_domain_count(), 1);
    assert!(retirement.advance().is_err());
    assert_eq!(calls.load(Relaxed), 1);
    assert!(control.recover_audio_candidate_domain(token).is_none());
    assert_eq!(observed_usage(&control).occupied_domains, 1);
    // Preserve the failed native execution and exact page endpoint, not a fake
    // report or a fixture Drop that finally joins the worker on this thread.
    retirement.root().announce_retirement();
    drop(owner);
    drop(control);
    drop(first);
    std::mem::forget(retirement);
}
