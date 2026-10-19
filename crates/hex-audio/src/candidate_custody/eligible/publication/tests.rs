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
    let bytes = root_allocation_bytes().unwrap()
        + pages::signal_data_bytes().unwrap()
        + 2 * std::mem::size_of::<Slot>();
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

#[test]
fn actual_lifecycle_paired_birth_reconciles_quote_and_each_limit_before_allocation() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported pinned lifecycle layout; no allocation qualification");
        return;
    }
    use hex_engine::sampler_host_fields::SharedMetadataRef;
    let mut quoted = None;
    let quote = allocation_counter::measure(|| {
        quoted = Some(SharedMetadataRef::constructor_bytes::<Life>().unwrap());
    });
    assert_eq!(quote.count_total, 0);
    assert_eq!(quote.bytes_total, 0);
    let bytes = quoted.unwrap();
    let mut single = None;
    let birth = allocation_counter::measure(|| single = Some(RuntimeSlot::vacant()));
    assert_eq!(birth.count_total, 1);
    assert_eq!(birth.bytes_total as usize, bytes);
    let Life {
        incarnation: _,
        live: _,
        revision: _,
        cancellation: _,
        successor_dependencies: _,
    } = single.as_ref().unwrap().life.as_ref(); // audited atomic-only leaf
    let mut alias = None;
    let cloning = allocation_counter::measure(|| {
        alias = Some(single.as_ref().unwrap().life.clone());
    });
    assert_eq!(cloning.count_total, 0);
    assert_eq!(cloning.bytes_total, 0);
    let dropping = allocation_counter::measure(|| drop(alias.take()));
    assert_eq!(dropping.count_total, 0);
    assert_eq!(dropping.count_current, 0);
    drop(single.take());

    let mut pair = None;
    let full = allocation_counter::measure(|| {
        pair = Some(Bank::prepare(reservation(), 0).unwrap());
    });
    let (bank, work) = pair.take().unwrap();
    assert_eq!(
        full.bytes_total as usize, bank.metadata_bytes,
        "all seven actual Vec backings plus both atomic Life payload/header allocations"
    );
    assert_eq!(full.count_total, 7 + bank.slots.len() as u64);
    let required = bank.metadata_bytes;
    drop((bank, work));
    let mut exact = reservation();
    exact.max_retained_data_bytes = required;
    exact.max_construction_peak_data_bytes = required;
    let (bank, work) = Bank::prepare(exact, 0).unwrap();
    assert_eq!(bank.metadata_bytes, required);
    drop((bank, work));
    for retained in [true, false] {
        let mut short = exact;
        if retained {
            short.max_retained_data_bytes -= 1;
        } else {
            short.max_construction_peak_data_bytes -= 1;
        }
        let mut refusal = None;
        let failure = allocation_counter::measure(|| {
            refusal = Bank::prepare(short, 0).err();
        });
        assert_eq!(
            refusal,
            Some(if retained {
                "constructor publication metadata reservation exceeded"
            } else {
                // This metadata-only constructor has equal retained/peak demand.
                // Peak-minus1 is independently rejected by its peak>=retained law,
                // not evidence of a separate transient graph-construction peak.
                "invalid constructor publication reservation"
            })
        );
        assert_eq!(
            failure.count_total, 0,
            "both limits refuse BEFORE Vec or paired Life birth"
        );
        assert_eq!(failure.bytes_total, 0);
    }
    eprintln!(
        "SUPPORTED lifecycle fresh paired payload+header bytes={bytes}; one allocation reconciled; clone/nonfinaldrop0; all metadata{required} exact/retained-minus1 priced; peak-minus1 rejects peak<retained law; both prebirth0 allocations"
    );
}

#[test]
fn actual_lifecycle_paired_aliases_require_each_physical_handle() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported pinned lifecycle layout; no alias qualification");
        return;
    }
    let slot = RuntimeSlot::vacant();
    let alias = slot.life.clone();
    let budget = hex_engine::AudioStorageLedgerReservation {
        data_rows: 4,
        handle_rows: 4,
        root_rows: 1,
        max_retained_bytes: 65_536,
        max_construction_peak_bytes: 65_536,
    };
    let mut omitted = hex_engine::AudioRootStorageLedger::try_new_on_control(budget).unwrap();
    omitted
        .borrow_shared_metadata_body_on_control(&slot.life)
        .unwrap();
    assert_eq!(
        omitted.validate_selected_shared_handles_on_control(),
        Err(hex_engine::AudioStorageError::UnknownStorage)
    );
    drop(omitted);
    let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(budget).unwrap();
    ledger
        .borrow_shared_metadata_body_on_control(&slot.life)
        .unwrap();
    ledger
        .borrow_shared_metadata_body_on_control(&alias)
        .unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    let summary = ledger.summary().unwrap();
    assert_eq!(summary.data_allocations, 1);
    assert_eq!(summary.unresolved_handles, 2);
    assert_eq!(
        summary.data_bytes,
        hex_engine::sampler_host_fields::SharedMetadataRef::constructor_bytes::<Life>().unwrap()
    );
    ledger
        .borrow_shared_metadata_body_on_control(&alias)
        .unwrap();
    assert_eq!(ledger.summary().unwrap(), summary);
    eprintln!(
        "SUPPORTED actual lifecycle paired data1/handles2; omitted actual alias refuses, repeated identical field dedups; no whole native-root claim"
    );
}

#[test]
fn actual_native_bank_contention_refuses_cancel_without_mutating_live_owner() {
    let Some(mut owner) = crate::prepared::candidate_tests::candidate() else {
        return;
    };
    let identity = owner.identity();
    let root = root(reservation_for(&mut owner));
    let (queued, ticket) = stage(&root, owner, vec![]).ok().unwrap();
    crate::candidate_custody::inline_lock_tests::held_on_other_thread(
        &root.0.eligible.bank,
        || {
            let measured = allocation_counter::measure(|| {
                assert_eq!(
                    root.cancel_eligible(&ticket),
                    AudioCandidateCancellationResult::Unavailable
                );
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
        },
    );
    {
        let bank = lock(&root.0.eligible.bank);
        let slot = &bank.publication.slots[ticket.stamp.slot];
        assert_eq!(slot.identity, Some(identity));
        assert_eq!(slot.life.cancellation.load(Ordering::Acquire), PENDING);
        assert_eq!(slot.anchors.as_ref().unwrap().identity(), identity);
    }
    assert_eq!(
        root.cancel_eligible(&ticket),
        AudioCandidateCancellationResult::Cancelled
    );
    root.ingest_eligible(queued.reject_before_install())
        .ok()
        .unwrap();
    root.advance_on_control();
    let recovered = root
        .recover(&ticket)
        .expect("actual same owner retired after release");
    assert_eq!(recovered.disposed.identity(), identity);
    eprintln!(
        "SUPPORTED actual native Bank other-thread hold: one-try cancellation Unavailable/allocfree0 preserves SAME live anchors and Pending; release allows actual cancel+normal retirement"
    );
}

#[test]
fn changed_inline_custody_body_is_priced_once_in_existing_publication_metadata_limits() {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported pinned constructor layout; no positive metadata limit proof");
        return;
    }
    let root = CandidateRoot::new(2).unwrap();
    let capacity = lock(&root.0.bank).slots.capacity();
    // Same actual caller expression as configure_publications: Inner embeds
    // eligible and all five wrappers. Their bodies must NOT be added again.
    let body = std::mem::size_of::<Inner>();
    let base = root_allocation_bytes().unwrap()
        + pages::signal_data_bytes().unwrap()
        + capacity * std::mem::size_of::<Slot>();
    let (bank, work) = Bank::prepare(reservation(), base).unwrap();
    let required = base + bank.metadata_bytes;
    drop((bank, work));
    let mut exact = reservation();
    exact.max_retained_data_bytes = required;
    exact.max_construction_peak_data_bytes = required;
    let (bank, work) = Bank::prepare(exact, base).unwrap();
    assert_eq!(base + bank.metadata_bytes, required);
    drop((bank, work));
    for retained in [true, false] {
        let mut short = exact;
        if retained {
            short.max_retained_data_bytes -= 1;
        } else {
            short.max_construction_peak_data_bytes -= 1;
        }
        let mut refused = false;
        let measured =
            allocation_counter::measure(|| refused = Bank::prepare(short, base).is_err());
        assert!(refused);
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    eprintln!(
        "SUPPORTED changed inline Inner{body} embeds all5wrappers once, existing slot capacity{capacity}; publication metadata total{required} exact/eachminus1 pre-new-backing0. Guard{} is separate overlapping scratch for future whole-host peak, not admitted here. Peak-minus1 also rejects peak<retained; no aggregate Root factory ceiling/claim.",
        std::mem::size_of::<CustodyMutexGuard<'_, NativeBank>>()
    );
}
