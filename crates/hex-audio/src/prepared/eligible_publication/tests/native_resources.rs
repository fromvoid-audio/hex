use super::*;
#[test]
fn native_flag_without_actual_carriers_and_resource_pressure_refuse_before_consuming_inputs() {
    let _serial = serial();
    let Some(mut owner) = candidate() else { return };
    let identity = owner.identity();
    let mut limits = reservation(&mut owner);
    let (mut naked, callback) = channel(48000).unwrap();
    naked.enable_native_custody().unwrap();
    assert!(naked.configure_audio_candidate_publication(limits).is_err());
    let tokens = Vec::with_capacity(4);
    let ptr = tokens.as_ptr();
    let cap = tokens.capacity();
    let refused = naked
        .stage_eligible_audio_candidate(owner, tokens, PublicationPolicy::InheritRuntime, 1)
        .err()
        .unwrap();
    assert_eq!(refused.owner.identity(), identity);
    assert_eq!(
        refused.owner.phase(),
        hex_engine::AudioCandidateDisposalPhase::Owned
    );
    assert_eq!(
        (refused.domains.as_ptr(), refused.domains.capacity()),
        (ptr, cap)
    );
    drop(naked);
    drop(callback);
    let (mut control, data, errors, mut retirement) = anchored();
    limits.handle_rows = 32; // finite caller capacity: no new process/global cap
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let refused = control
        .stage_eligible_audio_candidate(
            refused.owner,
            refused.domains,
            PublicationPolicy::InheritRuntime,
            1,
        )
        .err()
        .unwrap();
    assert_eq!(refused.owner.identity(), identity);
    assert_eq!(
        refused.owner.phase(),
        hex_engine::AudioCandidateDisposalPhase::Owned
    );
    assert_eq!(
        (refused.domains.as_ptr(), refused.domains.capacity()),
        (ptr, cap)
    );
    assert_eq!(control.revision, 0);
    assert_eq!(control.audio_candidate_custody().occupied_slots, 0);
    let pair = crate::prepared::candidate_tests::pair();
    let refused_pair = control.register_audio_candidate_domain(pair).err().unwrap();
    let lease = refused_pair.domain.0.catalog_lease().unwrap();
    assert!(refused_pair.domain.1.matches_catalog(&lease));
    drop(lease);
    assert!(
        refused_pair
            .domain
            .0
            .shutdown(refused_pair.domain.1)
            .unwrap()
            .accounting_complete
    );
    assert_eq!(
        control
            .audio_candidate_domain_usage()
            .unwrap()
            .occupied_domains,
        0
    );
    drop(refused);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
#[test]
fn both_final_guard_orders_and_completed_backend_gate_actual_eligible_return() {
    let _serial = serial();
    for errors_first in [false, true] {
        let Some(mut owner) = candidate() else { return };
        let identity = owner.identity();
        let limits = reservation(&mut owner);
        let (mut control, mut data, errors, retirement) = anchored();
        control
            .configure_audio_candidate_publication(limits)
            .unwrap();
        let ticket = control
            .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 1)
            .ok()
            .unwrap();
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        assert_eq!(control.status.applied_revision.load(Acquire), 1);
        let mut retirement = std::thread::spawn(move || {
            let mut retirement = retirement;
            let mut backend = Some(());
            let mut pending = false;
            let mut disposed = false;
            assert!(
                !crate::lifecycle::test_dispose_backend(
                    &mut backend,
                    &mut pending,
                    &mut disposed,
                    &mut retirement,
                    |_| Ok(())
                )
                .unwrap()
            );
            assert!(backend.is_none() && disposed);
            retirement
        })
        .join()
        .unwrap();
        if errors_first {
            drop(errors);
            assert!(!retirement.advance().unwrap());
            drop(data);
        } else {
            drop(data);
            assert!(!retirement.advance().unwrap());
            drop(errors);
        }
        assert!(control.recover_audio_candidate(&ticket).is_none());
        let mut retirement = std::thread::spawn(move || {
            let mut r = retirement;
            let _ = r.advance().unwrap();
            r
        })
        .join()
        .unwrap();
        let ready = recover(&mut control, &ticket);
        assert_eq!(ready.disposed.identity(), identity);
        assert_eq!(
            ready.return_kind,
            crate::AudioCandidateReturnKind::FinalActive
        );
        assert_eq!(control.audio_candidate_custody().occupied_slots, 0);
        assert!(
            control
                .audio_candidate_custody()
                .runtime_and_shared_overhead_unknown
        );
        drop(ready);
        drop(control);
        finish(&mut retirement);
    }
}
#[test]
fn actual_eligible_owner_remains_pinned_after_carrier_destructor_panic_without_retry_or_ready() {
    let _serial = serial();
    let Some(mut owner) = candidate() else { return };
    let identity = owner.identity();
    let limits = reservation(&mut owner);
    let (mut control, mut data, mut errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 1)
        .ok()
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    // Control-side fault sensitivity only, after valid admission. This test-only
    // carrier probe is not a claimed production payload/cost or callback catch.
    errors.owner_mut().unwrap().test_drop_probe(move || {
        count.fetch_add(1, Relaxed);
        std::panic::panic_any("eligible carrier drop");
    });
    drop(data);
    drop(errors);
    let (mut retirement, first) = std::thread::spawn(move || {
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
        (retirement, first)
    })
    .join()
    .unwrap();
    assert_eq!(retirement.test_extracted_identity(), Some(identity));
    assert!(retirement.advance().is_err());
    assert_eq!(calls.load(Relaxed), 1);
    assert!(first.downcast_ref::<&str>().is_some());
    assert!(control.recover_audio_candidate(&ticket).is_none());
    assert_eq!(control.audio_candidate_custody().occupied_slots, 1);
    let original = candidate().unwrap();
    let original_id = original.identity();
    let refused = control
        .stage_eligible_audio_candidate(original, vec![], PublicationPolicy::InheritRuntime, 2)
        .err()
        .unwrap();
    assert_eq!(refused.owner.identity(), original_id);
    // Explicit fixture quarantine: never manufacture a successful disposal or
    // allow the singleton to take a new unleased cleanup job for this pinned Root.
    retirement.root().announce_retirement();
    drop(control);
    drop(refused);
    drop(first);
    std::mem::forget(retirement);
}

#[test]
fn closed_root_revokes_already_queued_claim_without_install_or_ack() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let (project, _) = utility_fixture::project();
    let (mut owner, bank) = utility_fixture::owner_with_bank(&project);
    let identity = owner.identity();
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 1)
        .ok()
        .unwrap();
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    // Revoke a real queued constructor claim, independently of Callback's own
    // shutdown flag. No already-committed callback decision is being revoked.
    control.native_candidate_root().close();
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            assert!(callback.active.is_none());
        });
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
    assert_eq!(
        control.status.rejected_revision(),
        Some((1, PlaybackHandoffRejection::ConstructorClaimsUnavailable))
    );
    let ready = recover(&mut control, &ticket);
    assert_eq!(ready.disposed.identity(), identity);
    assert_eq!(ready.inputs.test_bank_identity(), bank);
    assert_eq!(
        ready.return_kind,
        crate::AudioCandidateReturnKind::NeverInstalled
    );
    drop(ready);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
