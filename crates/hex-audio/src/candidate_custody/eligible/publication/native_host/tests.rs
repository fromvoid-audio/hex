use super::*;
use hex_engine::{AudioRootStorageLedger, AudioStorageLedgerReservation};
fn limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 4096,
        handle_rows: 2048,
        root_rows: 16,
        max_retained_bytes: 64 * 1024 * 1024,
        max_construction_peak_bytes: 64 * 1024 * 1024,
    }
}
#[cfg(feature = "allocation-probe")]
#[test]
fn actual_registered_native_active_pcm_ack_and_second_preparation_keep_all_root_children() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    let _serial = crate::prepared::candidate_tests::serial();
    let mut owner = crate::prepared::candidate_tests::candidate().unwrap();
    let measured = owner.seal_constructor_on_control(limits()).unwrap();
    let reservation = AudioCandidatePublicationReservation {
        runtime_slots: 3,
        archive_slots: 3,
        member_rows: measured.member_rows.checked_mul(3).unwrap(),
        transfer_rows: measured.transfer_rows.checked_mul(3).unwrap(),
        receipt_rows: 3,
        data_rows: 4096,
        handle_rows: 2048,
        max_retained_data_bytes: 64 * 1024 * 1024,
        max_construction_peak_data_bytes: 64 * 1024 * 1024,
    };
    let (control, callback) =
        crate::prepared::channel_with_factory_reservation(48000, None, Some(reservation)).unwrap();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut worker) =
        reserve_native_worker(control, callback, None).unwrap();
    let mut retirement = NativeRetirement::new(terminals, root.clone());
    worker.control().unwrap().enable_native_custody().unwrap();
    worker
        .control()
        .unwrap()
        .configure_audio_candidate_publication(reservation)
        .unwrap();
    let ticket = worker
        .control()
        .unwrap()
        .stage_eligible_audio_candidate(
            owner,
            Vec::new(),
            crate::prepared::PublicationPolicy::InheritRuntime,
            19,
        )
        .ok()
        .unwrap();
    client.activate();
    client.command(hex_engine::PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 4];
    let callback = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
    });
    assert_eq!(output, [[0.25; 2]; 4]);
    assert_eq!((callback.count_total, callback.count_current), (0, 0));
    assert_eq!(
        client
            .status()
            .unwrap()
            .applied_revision
            .load(Ordering::Acquire),
        1
    );
    let token = worker
        .control()
        .unwrap()
        .reserve_audio_candidate_preparation(
            AudioCandidatePreparationReservation {
                source_capacity: 1,
                member_rows: 1024,
                transfer_rows: 1024,
                handle_rows: 512,
                storage: hex_engine::AudioStorageReservation {
                    allocation_items: 256,
                    handle_obligations: 512,
                    max_prepared_data_bytes: 16 * 1024 * 1024,
                    max_construction_data_bytes: 16 * 1024 * 1024,
                    caller_retained_bytes: 0,
                    caller_construction_bytes: 0,
                },
            },
            Vec::new(),
        )
        .ok()
        .unwrap();
    let mut next = hex_project::Editor::new(hex_project::Project::new(48000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(next_track) = next
        .execute(hex_project::Command::AddTrack {
            name: "next actual graph".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    next.execute(hex_project::Command::RemoveInstrument { track: next_track })
        .unwrap();
    let hex_project::Created::Asset(next_asset) = next
        .execute(hex_project::Command::AddAsset {
            path: "next.wav".into(),
            sample_rate: 48000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    next.execute(hex_project::Command::PlaceClip {
        track: next_track,
        asset: next_asset,
        start: 0,
        duration: 8,
        source_offset: 0,
        source_length: 8,
        looping: false,
    })
    .unwrap();
    let next_pcm = hex_engine::Audio::new(48000, vec![[0.5; 2]; 8]).unwrap();
    assert!(matches!(
        worker
            .control()
            .unwrap()
            .prepare_audio_candidate_on_control(token, |mut operations| {
                operations.copy_resident_source_on_control(next_asset, &next_pcm)?;
                operations.prepare_on_control(
                    next.project(),
                    1,
                    hex_engine::AudioSourceContext {
                        project_generation: 1,
                        editor_revision: 4,
                    },
                )
            }),
        Ok(AudioCandidatePreparationProgress::Ready)
    ));
    {
        let mut host = NativePreparationHost::acquire(&root, token).unwrap();
        assert!(host.bank.native.is_some());
        assert_eq!(
            host.bank.publication.slots[ticket.stamp.slot].phase,
            Phase::Accepted
        );
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        let mut visited = None;
        let measured = allocation_counter::measure(|| {
            visited = Some(ledger.borrow_sampler_native_fields_on_control(&host));
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
        visited.unwrap().unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert!(ledger.summary().unwrap().require_complete().is_err());
        let observed = ledger.summary().unwrap();
        drop(ledger);
        let inspect = |host: &NativePreparationHost<'_>, reservation| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
            let mut result = None;
            let measured = allocation_counter::measure(|| {
                result = Some(ledger.borrow_sampler_native_fields_on_control(host));
            });
            assert_eq!((measured.count_total, measured.count_current), (0, 0));
            result.unwrap()?;
            ledger.validate_selected_shared_handles_on_control()?;
            ledger.summary()
        };
        // Temporarily hide actual records without destroying their owners.
        // Every refusal must preserve the live graph and all restored records.
        macro_rules! require_each_record {
            ($field:ident) => {
                for index in 0..host.bank.$field.len() {
                    let record = host.bank.$field[index].take();
                    assert!(record.is_some());
                    assert_eq!(
                        inspect(&host, limits()),
                        Err(AudioStorageError::UnknownStorage)
                    );
                    host.bank.$field[index] = record;
                }
            };
        }
        require_each_record!(carriers);
        require_each_record!(prepared_rings);
        require_each_record!(atomic_signals);
        let status = host.bank.status.take();
        assert_eq!(
            inspect(&host, limits()),
            Err(AudioStorageError::UnknownStorage)
        );
        host.bank.status = status;
        for (retained_minus, peak_minus) in [(0, 0), (1, 0), (0, 1)] {
            let mut exact = limits();
            exact.max_retained_bytes = observed.retained_bytes - retained_minus;
            exact.max_construction_peak_bytes = observed.construction_peak_bytes - peak_minus;
            if retained_minus == 0 && peak_minus == 0 {
                assert_eq!(inspect(&host, exact).unwrap(), observed);
            } else {
                assert_eq!(
                    inspect(&host, exact),
                    Err(AudioStorageError::ReservationExceeded)
                );
            }
        }
        assert_eq!(
            host.bank.publication.slots[ticket.stamp.slot].phase,
            Phase::Accepted
        );
    }
    worker
        .control()
        .unwrap()
        .cancel_audio_candidate_preparation(token);
    // Cancelling the second actual owned graph must leave the first ACTIVE
    // graph and its published revision intact.
    output.fill([0.0; 2]);
    let after_cancel = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
    });
    assert_eq!(output, [[0.25; 2]; 4]);
    assert_eq!(
        (after_cancel.count_total, after_cancel.count_current),
        (0, 0)
    );
    assert_eq!(
        client
            .status()
            .unwrap()
            .applied_revision
            .load(Ordering::Acquire),
        1
    );
    // ACTIVE needs normal destruction of the actual backend-handle owner,
    // rather than caller guard drops or a supplied completed boolean.
    struct Backend {
        _data:
            crate::native_callback_owner::Guard<crate::native_callback_owner::NativeCallbackOwner>,
        _errors:
            crate::native_callback_owner::Guard<crate::native_callback_owner::ErrorCallbackOwner>,
    }
    let mut backend = Some(Backend {
        _data: data,
        _errors: errors,
    });
    drop((client, worker));
    let mut pending = false;
    let mut disposed = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !crate::lifecycle::test_dispose_backend(
        &mut backend,
        &mut pending,
        &mut disposed,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "{}",
            root.test_status()
        );
        std::thread::yield_now();
    }
    assert!(backend.is_none() && disposed && !pending);
    crate::prepared::candidate_tests::finish(&mut retirement);
}
#[test]
fn actual_queued_runtime_and_second_preparation_visit_retained_graph_without_extracting() {
    let mut owner = crate::prepared::candidate_tests::candidate()
        .expect("pinned constructor layout supports real nonempty audio graph");
    let measured = owner.seal_constructor_on_control(limits()).unwrap();
    let (root, token) = fixture_with_rows(
        CandidateRoot::new(3).unwrap(),
        measured.member_rows.checked_mul(2).unwrap(),
        measured.transfer_rows.checked_mul(2).unwrap(),
    );
    let identity = owner.identity();
    let summary = root.0.eligible.seal_for_stage(&mut owner, 0).unwrap();
    let (mut queued, ticket) = {
        let _terminal = lock(&root.0.bank);
        root.0
            .eligible
            .stage_sealed(
                &root,
                owner,
                Vec::new(),
                Header {
                    revision: 1,
                    context_generation: 19,
                    policy: crate::prepared::PublicationPolicy::InheritRuntime,
                },
                summary,
            )
            .unwrap_or_else(|refusal| panic!("{}", refusal.reason))
    };
    assert_ne!(ticket.stamp.slot, token.slot);
    {
        let mut host = NativePreparationHost::acquire(&root, token).unwrap();
        let occupied = host.runtime(ticket.stamp.slot).unwrap();
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        ledger
            .borrow_sampler_native_fields_on_control(&host)
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert_eq!(
            host.bank.publication.slots[ticket.stamp.slot].identity,
            Some(identity)
        );
        assert!(
            occupied
                .body
                .same(BodyRef::of(&host.bank.publication.slots[ticket.stamp.slot]))
        );
        assert!(ledger.summary().unwrap().require_complete().is_err());
        let observed = ledger.summary().unwrap();
        drop(ledger);
        let inspect = |host: &NativePreparationHost<'_>, reservation| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
            let mut result = None;
            let measured = allocation_counter::measure(|| {
                result = Some(ledger.borrow_sampler_native_fields_on_control(host));
            });
            assert_eq!((measured.count_total, measured.count_current), (0, 0));
            result.unwrap()?;
            ledger.validate_selected_shared_handles_on_control()?;
            ledger.summary()
        };
        let occupied = ticket.stamp.slot;
        let anchors = host.bank.publication.slots[occupied].anchors.take();
        assert_eq!(
            inspect(&host, limits()),
            Err(AudioStorageError::UnknownStorage)
        );
        host.bank.publication.slots[occupied].anchors = anchors;
        let state = host.bank.publication.slots[occupied].state_storage.take();
        assert_eq!(
            inspect(&host, limits()),
            Err(AudioStorageError::UnknownStorage)
        );
        host.bank.publication.slots[occupied].state_storage = state;
        let incarnation = host.bank.publication.slots[occupied]
            .life
            .incarnation
            .load(Ordering::Acquire);
        host.bank.publication.slots[occupied]
            .life
            .incarnation
            .store(incarnation + 1, Ordering::Release);
        assert_eq!(
            inspect(&host, limits()),
            Err(AudioStorageError::SourceMismatch)
        );
        host.bank.publication.slots[occupied]
            .life
            .incarnation
            .store(incarnation, Ordering::Release);
        for (retained_minus, peak_minus) in [(0, 0), (1, 0), (0, 1)] {
            let mut exact = limits();
            exact.max_retained_bytes = observed.retained_bytes - retained_minus;
            exact.max_construction_peak_bytes = observed.construction_peak_bytes - peak_minus;
            let result = inspect(&host, exact);
            if retained_minus == 0 && peak_minus == 0 {
                assert_eq!(result.unwrap(), observed);
            } else {
                assert_eq!(result, Err(AudioStorageError::ReservationExceeded));
            }
        }
        assert_eq!(
            host.bank.publication.slots[occupied]
                .anchors
                .as_ref()
                .unwrap()
                .identity(),
            identity
        );
        eprintln!(
            "SUPPORTED actual queued graph + second reserved preparation, direct normal traversal0alloc/free; actual anchors/State omissions and changed Life incarnation refuse; exact/retained-minus1/peak-minus1 inventory boundaries preserve original owners; NOT compiler-beforebirth or nativeSome/full-Service admission"
        );
    }
    // Every inspection/ledger/guard is gone before actual DSP. Rendering does
    // not move either constructor half or release the second preparation.
    let mut output = [[0.0; 2]; 4];
    let rendered = allocation_counter::measure(|| {
        let runtime = queued.state.runtime.as_mut().unwrap();
        runtime
            .apply_action(hex_engine::PlaybackAction::Transport(
                hex_engine::PlaybackCommand::Play,
            ))
            .unwrap();
        runtime.render(&mut output).unwrap();
    });
    assert_eq!(output, [[0.25; 2]; 4]);
    assert_eq!((rendered.count_total, rendered.count_current), (0, 0));
    {
        let host = NativePreparationHost::acquire(&root, token).unwrap();
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        ledger
            .borrow_sampler_native_fields_on_control(&host)
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
    }
    root.cancel_runtime_preparation(token);
    root.0
        .eligible
        .ingest_publication(&root, queued.reject_before_install())
        .ok()
        .unwrap();
    root.advance_on_control();
    assert_eq!(
        root.0
            .eligible
            .recover_publication(&ticket)
            .unwrap()
            .disposed
            .identity(),
        identity
    );
    assert!(root.0.eligible.complete());
}
fn fixture() -> (CandidateRoot, AudioCandidatePreparationToken) {
    fixture_with_root(CandidateRoot::new(3).unwrap())
}
fn fixture_with_root(root: CandidateRoot) -> (CandidateRoot, AudioCandidatePreparationToken) {
    fixture_with_rows(root, 4096, 4096)
}
fn fixture_with_rows(
    root: CandidateRoot,
    member_rows: usize,
    transfer_rows: usize,
) -> (CandidateRoot, AudioCandidatePreparationToken) {
    root.configure_publications(
        AudioCandidatePublicationReservation {
            runtime_slots: 2,
            archive_slots: 2,
            member_rows,
            transfer_rows,
            receipt_rows: 4,
            data_rows: 4096,
            handle_rows: 2048,
            max_retained_data_bytes: 96 * 1024 * 1024,
            max_construction_peak_data_bytes: 96 * 1024 * 1024,
        },
        0,
    )
    .unwrap();
    let token = root
        .reserve_runtime_preparation(
            AudioCandidatePreparationReservation {
                source_capacity: 3,
                member_rows: 1024,
                transfer_rows: 1024,
                handle_rows: 512,
                storage: hex_engine::AudioStorageReservation {
                    allocation_items: 256,
                    handle_obligations: 512,
                    max_prepared_data_bytes: 16 * 1024 * 1024,
                    max_construction_data_bytes: 16 * 1024 * 1024,
                    caller_retained_bytes: 0,
                    caller_construction_bytes: 0,
                },
            },
            Vec::with_capacity(2),
        )
        .ok()
        .unwrap();
    (root, token)
}
fn asset(name: &str) -> hex_project::AssetId {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48000).unwrap(), 4).unwrap();
    let hex_project::Created::Asset(asset) = editor
        .execute(hex_project::Command::AddAsset {
            path: name.into(),
            sample_rate: 48000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    asset
}
#[test]
fn actual_nonempty_guarded_host_inspects_without_birth_and_reborrows_same_cell_after_ledger_drop() {
    let (root, token) = fixture();
    let audio = hex_engine::Audio::new(48000, vec![[0.25; 2]; 8]).unwrap();
    let first = asset("first.wav");
    root.with_runtime_preparation(token, |cell| {
        cell.operations_on_control()
            .copy_resident_source_on_control(first, &audio)
    })
    .unwrap();
    let mut host = NativePreparationHost::acquire(&root, token).unwrap();
    let address = host.selected_address;
    {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        let measured = allocation_counter::measure(|| {
            ledger
                .borrow_sampler_native_fields_on_control(&host)
                .unwrap()
        });
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(ledger.summary().unwrap().data_bytes > root_allocation_bytes().unwrap());
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert!(ledger.summary().unwrap().require_complete().is_err());
    }
    let selected = host.selected_mut().unwrap();
    assert_eq!(std::ptr::from_ref(selected).addr(), address);
    // No extraction/replacement: mutate the same retained actual source vector.
    // Fresh Editor asset IDs are fixture-local; use the next actual ID from one
    // real Editor for this second source.
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48000).unwrap(), 4).unwrap();
    for name in ["unused.wav", "second.wav"] {
        let hex_project::Created::Asset(next) = editor
            .execute(hex_project::Command::AddAsset {
                path: name.into(),
                sample_rate: 48000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        if name == "second.wav" {
            selected
                .operations_on_control()
                .copy_resident_source_on_control(next, &audio)
                .unwrap();
        }
    }
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    ledger
        .borrow_sampler_native_fields_on_control(&host)
        .unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    eprintln!(
        "SUPPORTED actual selected nonempty source+spare native backing traversal0alloc/free; all immutable ledger loans end before SAME persistent cell mutable reborrow; full admission remains refused"
    );
}
#[test]
fn actual_five_guards_each_refuse_immediately_and_wrong_token_or_cell_cannot_reborrow() {
    use crate::candidate_custody::inline_lock_tests::held_on_other_thread;
    let (root, token) = fixture();
    let inspect = || {
        assert!(matches!(
            NativePreparationHost::acquire(&root, token),
            Err(AudioStorageError::ExternalReadersPending)
        ))
    };
    held_on_other_thread(&root.0.bank, inspect);
    held_on_other_thread(&root.0.execution, inspect);
    held_on_other_thread(&root.0.returns, inspect);
    held_on_other_thread(&root.0.eligible.work, inspect);
    held_on_other_thread(&root.0.eligible.bank, inspect);
    let mut stale = token;
    stale.incarnation += 1;
    assert!(matches!(
        NativePreparationHost::acquire(&root, stale),
        Err(AudioStorageError::SourceMismatch)
    ));
    let mut host = NativePreparationHost::acquire(&root, token).unwrap();
    host.selected_address += 1;
    assert!(matches!(
        host.selected_mut(),
        Err(AudioStorageError::SourceMismatch)
    ));
    eprintln!(
        "SUPPORTED same actual five other-thread holds: immediate try refusal/no new wait; stale incarnation/wrong selected-address refuse before mutable loan"
    );
}
#[test]
fn actual_native_constructor_boundary_is_not_hidden_as_absent_fields() {
    let _serial = crate::prepared::candidate_tests::serial();
    let (mut control, data, errors, mut retirement) = crate::prepared::candidate_tests::anchored();
    let root = control.native_candidate_root();
    root.configure_publications(
        AudioCandidatePublicationReservation {
            runtime_slots: 2,
            archive_slots: 2,
            member_rows: 4096,
            transfer_rows: 4096,
            receipt_rows: 4,
            data_rows: 4096,
            handle_rows: 2048,
            max_retained_data_bytes: 96 * 1024 * 1024,
            max_construction_peak_data_bytes: 96 * 1024 * 1024,
        },
        0,
    )
    .unwrap();
    let token = root
        .reserve_runtime_preparation(
            AudioCandidatePreparationReservation {
                source_capacity: 1,
                member_rows: 1024,
                transfer_rows: 1024,
                handle_rows: 512,
                storage: hex_engine::AudioStorageReservation {
                    allocation_items: 256,
                    handle_obligations: 512,
                    max_prepared_data_bytes: 16 * 1024 * 1024,
                    max_construction_data_bytes: 16 * 1024 * 1024,
                    caller_retained_bytes: 0,
                    caller_construction_bytes: 0,
                },
            },
            vec![],
        )
        .ok()
        .unwrap();
    {
        let host = NativePreparationHost::acquire(&root, token).unwrap();
        assert!(
            host.bank.native.is_some(),
            "ACTUAL anchored native constructor, not fake None"
        );
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        assert_eq!(
            ledger.borrow_sampler_native_fields_on_control(&host),
            Err(AudioStorageError::UnknownStorage)
        );
        assert!(
            ledger.summary().is_err(),
            "refused partial prefix is not reusable"
        );
    }
    root.cancel_runtime_preparation(token);
    drop((data, errors));
    control.reclaim();
    drop(control);
    crate::prepared::candidate_tests::finish(&mut retirement);
    eprintln!(
        "REFUSED actual normal native endpoint boundary requires actual callback/current/queue field bindings; constructor counts cannot become a fake complete host"
    );
}

#[test]
fn actual_claim_lineages_bind_guarded_root_and_birth_slot_and_refuse_omission() {
    use hex_engine::sampler_host_fields::{
        NativeClaimStorageRegistration, SharedMetadataRef, VecRef,
    };
    let (root, token) = fixture();
    let mut host = NativePreparationHost::acquire(&root, token).unwrap();
    // Actual preexisting fields under the SAME exclusions, no copied approval.
    {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        ledger
            .borrow_sampler_native_fields_on_control(&host)
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        let before = ledger.summary().unwrap();
        let extra = NativeClaimStorageRegistration::metadata_requested_bytes_on_control().unwrap();
        let scratch = NativeClaimStorageRegistration::construction_scratch_bytes_on_control()
            .unwrap()
            + std::mem::size_of::<ClaimPayload>()
            + std::mem::size_of::<ClaimHandles>();
        assert!(before.retained_bytes.checked_add(extra).unwrap() <= limits().max_retained_bytes);
        assert!(
            before
                .construction_peak_bytes
                .checked_add(extra)
                .unwrap()
                .checked_add(scratch)
                .unwrap()
                <= limits().max_construction_peak_bytes
        );
    }
    let (record, handles) = prepare_claim_handles(&root, &host.bank, token.slot).unwrap();
    host.bank.publication.slots[token.slot].claim_storage[0] = Some(record);
    fn inspect(
        host: &NativePreparationHost<'_>,
    ) -> Result<hex_engine::AudioRootStorageSummary, AudioStorageError> {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        ledger.borrow_sampler_native_fields_on_control(host)?;
        ledger.validate_selected_shared_handles_on_control()?;
        ledger.summary()
    }
    let live = inspect(&host).unwrap();
    assert!(live.require_complete().is_err()); // No native/channel/Service admission.
    let record = host.bank.publication.slots[token.slot].claim_storage[0]
        .take()
        .unwrap();
    assert_eq!(inspect(&host), Err(AudioStorageError::UnknownStorage));
    host.bank.publication.slots[token.slot].claim_storage[0] = Some(record);
    inspect(&host).unwrap();
    // A different REAL Root has identically shaped metadata; it is not the
    // current record's containing Root or birth-slot anchor.
    let (foreign, foreign_token) = fixture();
    let foreign_host = NativePreparationHost::acquire(&foreign, foreign_token).unwrap();
    {
        let record = host.bank.publication.slots[token.slot].claim_storage[0]
            .as_ref()
            .unwrap();
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        assert_eq!(
            record.visit_storage_on_control(
                SharedMetadataRef::of(&foreign.0),
                VecRef::of(&foreign_host.bank.publication.slots),
                foreign_token.slot,
                SharedMetadataRef::of(
                    &foreign_host.bank.publication.slots[foreign_token.slot].life
                ),
                &mut ledger,
            ),
            Err(AudioStorageError::SourceMismatch)
        );
        assert!(ledger.summary().is_err());
    }
    drop(foreign_host);
    drop(handles); // Actual owning Root+Life fields normally drop before lease completion.
    let disposed = inspect(&host).unwrap();
    assert!(
        host.bank.publication.slots[token.slot].claim_storage[0]
            .as_ref()
            .unwrap()
            .is_normally_disposed_on_control()
    );
    assert_eq!(live.data_bytes, disposed.data_bytes); // Metadata remains physically retained.
    host.bank.publication.slots[token.slot].claim_storage[0] = None;
    let released = inspect(&host).unwrap();
    assert_eq!(
        disposed.data_bytes - released.data_bytes,
        NativeClaimStorageRegistration::metadata_requested_bytes_on_control().unwrap()
    );
    eprintln!(
        "SUPPORTED actual guarded Root/birth-Life anchors + constructor-born individual owning lineages close normally; omitted record and equal-layout foreign Root refuse; actual payload drop removes lineages, metadata stays until explicit normal control release; no full native/Service claim"
    );
}

#[test]
fn registered_resident_sixth_guard_is_real_nonblocking_and_releases_first_five_on_refusal() {
    let (root, token) = fixture_with_root(CandidateRoot::new_registered(3).unwrap());
    let archive = root.0.resident.as_ref().unwrap();
    archive.test_while_slot_held(|| {
        let refusal = allocation_counter::measure(|| {
            assert!(matches!(
                NativePreparationHost::acquire(&root, token),
                Err(AudioStorageError::ExternalReadersPending)
            ));
        });
        assert_eq!((refusal.count_total, refusal.bytes_total), (0, 0));
        // All five earlier guards were unwound without mutation or construction.
        drop(root.0.bank.try_lock().unwrap());
        drop(root.0.execution.try_lock().unwrap());
        drop(root.0.returns.try_lock().unwrap());
        drop(root.0.eligible.work.try_lock().unwrap());
        drop(root.0.eligible.bank.try_lock().unwrap());
    });
    let mut host = NativePreparationHost::acquire(&root, token).unwrap();
    assert!(host.resident().unwrap().is_some());
    let address = host.selected_address;
    {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        let inspection = allocation_counter::measure(|| {
            ledger
                .borrow_sampler_native_fields_on_control(&host)
                .unwrap();
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
        });
        assert_eq!((inspection.count_total, inspection.bytes_total), (0, 0));
    }
    assert_eq!(
        std::ptr::from_ref(host.selected_mut().unwrap()).addr(),
        address
    );
    drop(host);
    // Sixth guard is gone before subsequent worker/Client turns.
    drop(archive.try_read_on_control().unwrap());
}

#[test]
fn actual_engine_operations_bridge_keeps_selected_field_and_all_exclusions() {
    use hex_engine::sampler_host_fields::OriginalSamplerNativeHost;
    let (root, token) = fixture();
    let mut host = NativePreparationHost::acquire(&root, token).unwrap();
    let original = host.selected_address;
    {
        let observed = host.selected_preparation_on_control().unwrap();
        assert_eq!(std::ptr::from_ref(observed).addr(), original);
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        ledger
            .borrow_sampler_native_fields_on_control(&host)
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        // No whole-host admission/permit is produced by this borrowed bridge.
        assert!(ledger.summary().unwrap().require_complete().is_err());
    }
    // The actual operations loan cannot be requested before the ledger ends;
    // then the same host retains all six possible exclusions through reborrow.
    assert!(
        host.selected_operations_on_control()
            .unwrap()
            .prepared_identity()
            .is_none()
    );
    assert_eq!(
        std::ptr::from_ref(host.selected_preparation_on_control().unwrap()).addr(),
        original
    );
    host.selected_address = original.wrapping_add(1);
    assert!(matches!(
        host.selected_operations_on_control(),
        Err(AudioStorageError::SourceMismatch)
    ));
}
