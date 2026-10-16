use super::*;
fn reservation() -> AudioCandidatePublicationReservation {
    AudioCandidatePublicationReservation {
        runtime_slots: 2,
        archive_slots: 2,
        member_rows: 4096,
        transfer_rows: 4096,
        receipt_rows: 2,
        data_rows: 4096,
        handle_rows: 2048,
        max_retained_data_bytes: 64 * 1024 * 1024,
        max_construction_peak_data_bytes: 64 * 1024 * 1024,
    }
}
fn reservation_for(owner: &mut AudioCandidateOwner) -> AudioCandidatePublicationReservation {
    let mut reservation = reservation();
    let summary = owner
        .seal_constructor_on_control(reservation.ledger())
        .unwrap();
    // These are fixture-selected capacities from actual observations, not a
    // global graph count or a detached claim accepted by the Root.
    reservation.member_rows = summary
        .member_rows
        .checked_mul(reservation.runtime_slots)
        .unwrap();
    reservation.transfer_rows = summary
        .transfer_rows
        .checked_mul(reservation.runtime_slots)
        .unwrap();
    reservation
}
fn root(reservation: AudioCandidatePublicationReservation) -> CandidateRoot {
    let root = CandidateRoot::new(2).unwrap();
    let bytes = std::mem::size_of::<Inner>() + 2 * std::mem::size_of::<Slot>();
    root.0
        .eligible
        .configure_publications(root.root_id(), reservation, bytes)
        .unwrap();
    root
}
fn stage(
    root: &CandidateRoot,
    mut owner: AudioCandidateOwner,
    domains: Vec<AudioCandidateDomainToken>,
) -> Result<(Queued, AudioCandidateTicket), AudioCandidateEligibleStageRefusal> {
    let summary = root
        .0
        .eligible
        .seal_for_stage(&mut owner, domains.capacity())
        .unwrap();
    let _terminal_barrier = lock(&root.0.bank);
    root.0.eligible.stage_sealed(
        root,
        owner,
        domains,
        Header {
            revision: 1,
            context_generation: 19,
            policy: crate::prepared::PublicationPolicy::InheritRuntime,
        },
        summary,
    )
}
#[test]
fn reserved_actual_split_retains_anchors_until_runtime_and_metadata_drop_then_bank_report() {
    let Some(mut owner) = crate::prepared::candidate_tests::candidate() else {
        return;
    };
    let identity = owner.identity();
    let reservation = reservation_for(&mut owner);
    let root = root(reservation);
    let (queued, ticket) =
        stage(&root, owner, Vec::new()).unwrap_or_else(|r| panic!("{}", r.reason));
    assert!(root.0.eligible.recover_publication(&ticket).is_none());
    assert_eq!(queued.state.context_generation, 19);
    assert_eq!(
        queued.state.policy,
        crate::prepared::PublicationPolicy::InheritRuntime
    );
    {
        let bank = lock(&root.0.eligible.bank);
        let slot = &bank.publication.slots[ticket.stamp.slot];
        assert_eq!(slot.anchors.as_ref().unwrap().identity(), identity);
        assert!(
            slot.anchors
                .as_ref()
                .unwrap()
                .constructor_claims(queued.state.runtime.as_ref().unwrap())
                .is_some()
        );
        assert!(slot.retained > 0 && slot.summary.unwrap().member_rows > 0);
        assert!(slot.life.live.load(Ordering::Acquire));
    }
    // This owning-module boundary control never exposes a native callback.
    // The next queue checkpoint tests that production rejection producer.
    root.0
        .eligible
        .ingest_publication(&root, queued.reject_before_install())
        .ok()
        .unwrap();
    let worker_root = root.clone();
    let control_thread = std::thread::current().id();
    let disposal_thread = std::thread::spawn(move || {
        worker_root.advance_on_control();
        std::thread::current().id()
    })
    .join()
    .unwrap();
    assert_ne!(control_thread, disposal_thread);
    let stale = AudioCandidateTicket {
        stamp: Stamp {
            incarnation: ticket.stamp.incarnation + 1,
            ..ticket.stamp
        },
    };
    assert!(root.0.eligible.recover_publication(&stale).is_none());
    let recovery = root.0.eligible.recover_publication(&ticket).unwrap();
    assert_eq!(recovery.disposed.identity(), identity);
    assert!(recovery.inputs.owning_handle_count() > 0);
    assert!(root.0.eligible.recover_publication(&ticket).is_none());
    assert!(root.0.eligible.complete());
}
#[test]
fn member_pressure_and_foreign_return_refuse_original_ownership_before_release() {
    let Some(mut owner) = crate::prepared::candidate_tests::candidate() else {
        return;
    };
    let identity = owner.identity();
    let configured = reservation_for(&mut owner);
    let mut constrained = configured;
    constrained.member_rows = 0;
    let root = root(constrained);
    let domains = Vec::with_capacity(7);
    let original = (domains.as_ptr(), domains.capacity());
    let refusal = stage(&root, owner, domains).err().unwrap();
    assert_eq!(refusal.owner.identity(), identity);
    assert_eq!(
        refusal.owner.phase(),
        hex_engine::AudioCandidateDisposalPhase::Owned
    );
    assert_eq!(
        (refusal.domains.as_ptr(), refusal.domains.capacity()),
        original
    );
    assert!(root.0.eligible.complete());
    let actual_root = super::tests::root(configured);
    let (queued, ticket) = stage(&actual_root, refusal.owner, refusal.domains)
        .unwrap_or_else(|r| panic!("{}", r.reason));
    let returned = queued.reject_before_install();
    let returned = root
        .0
        .eligible
        .ingest_publication(&root, returned)
        .err()
        .unwrap();
    assert_eq!(
        returned
            .state
            .as_ref()
            .unwrap()
            .runtime
            .as_ref()
            .unwrap()
            .identity(),
        identity
    );
    actual_root
        .0
        .eligible
        .ingest_publication(&actual_root, returned)
        .ok()
        .unwrap();
    actual_root.advance_on_control();
    assert_eq!(
        actual_root
            .0
            .eligible
            .recover_publication(&ticket)
            .unwrap()
            .disposed
            .identity(),
        identity
    );
}
#[test]
fn abandoned_return_destroys_original_bank_and_consumes_charge_on_worker_without_ui_poll() {
    let Some(mut owner) = crate::prepared::candidate_tests::candidate() else {
        return;
    };
    let reservation = reservation_for(&mut owner);
    let root = root(reservation);
    let (queued, ticket) =
        stage(&root, owner, Vec::new()).unwrap_or_else(|r| panic!("{}", r.reason));
    root.0
        .eligible
        .ingest_publication(&root, queued.reject_before_install())
        .ok()
        .unwrap();
    // Isolated owning-boundary fixture: mark the same actual abandonment
    // state without enqueuing an unleased Root into the native singleton.
    // The real Control/terminal queue path has its own serialized control.
    root.0.eligible.close();
    root.0.abandoned.store(true, Ordering::Release);
    let worker_root = root.clone();
    let thread = std::thread::spawn(move || {
        worker_root.advance_on_control();
        std::thread::current().id()
    })
    .join()
    .unwrap();
    assert_ne!(thread, std::thread::current().id());
    assert!(root.0.eligible.complete());
    assert!(root.0.eligible.recover_publication(&ticket).is_none());
    let work = lock(&root.0.eligible.work);
    assert!(work.publication.anchors.iter().all(Option::is_none));
    assert!(work.publication.executions.iter().all(Option::is_none));
    assert!(work.publication.banks.iter().all(Option::is_none));
}
#[test]
fn excessive_metadata_request_refuses_before_reserving_per_cell_payloads() {
    let mut reservation = reservation();
    reservation.runtime_slots = usize::MAX / std::mem::size_of::<RuntimeSlot>();
    reservation.receipt_rows = reservation.runtime_slots;
    assert!(Bank::prepare(reservation, 128).is_err());
}
