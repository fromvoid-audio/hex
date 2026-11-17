use super::*;
use hex_engine::{
    AudioArrangementInput, AudioCandidateOwner, AudioPreparationCustody, AudioStorageReservation,
    OwnedAudioInputs,
};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
fn artifact() -> Option<AudioCandidateOwner> {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return None;
    }
    let project = hex_project::Project::new(48000).unwrap();
    let inputs = OwnedAudioInputs::try_new(
        vec![],
        AudioStorageReservation {
            allocation_items: 64,
            handle_obligations: 256,
            max_prepared_data_bytes: 64 * 1024 * 1024,
            max_construction_data_bytes: 64 * 1024 * 1024,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = custody
        .begin_input_on_control(
            AudioArrangementInput::new(&project, 11, inputs).with_audio_prepared_revision(),
        )
        .ok()
        .unwrap();
    Some(
        AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap(),
    )
}
fn pair() -> (
    crate::worker::tests::Fixture,
    StreamControl,
    StreamCallbackPages,
    StreamSourceKey,
) {
    pair_with_hooks(None)
}
fn pair_with_hooks(
    hooks: Option<crate::worker::TestWorkerHooks>,
) -> (
    crate::worker::tests::Fixture,
    StreamControl,
    StreamCallbackPages,
    StreamSourceKey,
) {
    pair_with_frames(hooks, 16001)
}
fn pair_with_frames(
    hooks: Option<crate::worker::TestWorkerHooks>,
    frames: u16,
) -> (
    crate::worker::tests::Fixture,
    StreamControl,
    StreamCallbackPages,
    StreamSourceKey,
) {
    pair_with_frames_using(frames, |_| hooks)
}
fn pair_with_frames_using(
    frames: u16,
    hooks: impl FnOnce(StreamSourceKey) -> Option<crate::worker::TestWorkerHooks>,
) -> (
    crate::worker::tests::Fixture,
    StreamControl,
    StreamCallbackPages,
    StreamSourceKey,
) {
    let wav = crate::worker::tests::Fixture::pcm16_frames(frames);
    let limits = VerifiedWavReaderLimits::new(
        WavReaderLimits::new(2 * 1024 * 1024, u64::from(frames), 128, 32768),
        1024 * 1024,
    );
    let reader = VerifiedWavReader::open(&wav.0, limits).unwrap();
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48000).unwrap(), 4).unwrap();
    let hex_project::Created::Asset(asset) = editor
        .execute(hex_project::Command::AddAsset {
            path: "source.wav".into(),
            sample_rate: 48000,
            frames: u64::from(frames),
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let key = StreamSourceKey::new(71, asset);
    let metadata = hex_engine::StreamedSourceMetadata::new(
        key,
        48000,
        48000,
        u64::from(frames),
        u64::from(frames),
        hex_project::IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .unwrap();
    drop(reader);
    let mut runtime = StreamRuntime::new_inner(
        StreamConfig::with_pool_pages(4),
        vec![SourceDefinition::new(wav.0.clone(), metadata, limits)],
        hooks(key),
        #[cfg(feature = "test-support")]
        None,
    )
    .unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let (control, pages) = runtime.split().ok().unwrap();
    (wav, control, pages, key)
}
fn stamp(owner: &AudioCandidateOwner, epoch: u64) -> ReadinessStamp {
    ReadinessStamp {
        request_epoch: epoch,
        artifact: owner.identity().into(),
        project_generation: 0,
        editor_revision: 7,
        device_epoch: 5,
        device_rate: 48000,
        predecessor: None,
        admitted_command_prefix: 9,
        window: hex_engine::StreamChunkWindow {
            performance_frame: 4094,
            arrangement_frame: 4094,
            route_generation: 0,
            frames: 8,
        },
    }
}
fn finish(domain: &mut ConstructorStreamDomain) -> PagePoolReport {
    for _ in 0..10 {
        if let Some(report) = domain.advance_disposal_on_control().unwrap() {
            return report;
        }
    }
    panic!("domain disposal did not finish")
}

#[test]
fn cancellation_retains_prefetch_origin_after_real_service_coalesces() {
    let Some(mut owner) = artifact() else { return };
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let (publication_tx, _publication_rx) = mpsc::sync_channel(2);
    let (_wav, control, pages, key) = pair_with_hooks(Some(crate::worker::TestWorkerHooks {
        read_gate: None,
        scalar_steps: None,
        entered: entered_tx,
        release: release_rx,
        publications: publication_tx,
    }));
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut domain = ConstructorStreamDomain::try_new_with_readiness_limits_on_control(
        control,
        pages,
        usize::MAX,
        usize::MAX,
        usize::MAX,
        Some(StreamReadinessReservation {
            extents_per_role: 3,
        }),
    )
    .ok()
    .unwrap();
    let mut loan = domain.loan_pages_on_control().unwrap();
    loan.pages.prefetch_page(key, 1);
    let extent = [StreamPageExtent {
        source: key,
        page_index: 1,
        start: 0,
        end: 8,
    }];
    let candidate = stamp(&owner, 1);
    loan.submit(ProtectionRole::Candidate, candidate, &extent)
        .unwrap();
    #[cfg(feature = "allocation-probe")]
    let mut progress = None;
    #[cfg(feature = "allocation-probe")]
    let mut cancelled = false;
    #[cfg(feature = "allocation-probe")]
    let allocations = allocation_counter::measure(|| {
        progress = Some(loan.service(ProtectionRole::Candidate, candidate, 1, 1));
        cancelled = loan.cancel(ProtectionRole::Candidate, candidate.request_epoch);
    });
    #[cfg(not(feature = "allocation-probe"))]
    let (progress, cancelled) = (
        Some(loan.service(ProtectionRole::Candidate, candidate, 1, 1)),
        loan.cancel(ProtectionRole::Candidate, candidate.request_epoch),
    );
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(progress, Some(Ok(ReadinessProgress::Pending)));
    assert_eq!(loan.pages.endpoint.stats().coalesced_requests, 1);
    assert!(cancelled);
    assert!(
        loan.pages
            .endpoint
            .test_speculative_request(crate::ResourcePageKey {
                source: key.into(),
                page_index: 1,
            })
    );
    release_tx.send(()).unwrap();
    let mut loan = Some(loan);
    domain.return_pages_on_control(&mut loan).unwrap();
    let report = finish(&mut domain);
    assert!(report.accounting_complete && report.worker_joined);
    owner.dispose_on_control().unwrap();
}

#[test]
fn cancelled_coalesced_prefetch_reclaims_before_real_worker_returns() {
    let Some(mut owner) = artifact() else { return };
    let (entered, entered_rx) = mpsc::sync_channel(1);
    let (start, start_rx) = mpsc::sync_channel(1);
    let (read_entered, read_entered_rx) = mpsc::sync_channel(1);
    let (resume, resume_rx) = mpsc::sync_channel(1);
    let (publications, published) = mpsc::sync_channel(8);
    let (_wav, control, pages, key) = pair_with_frames_using(24000, |key| {
        Some(crate::worker::TestWorkerHooks {
            entered,
            release: start_rx,
            publications,
            scalar_steps: None,
            read_gate: Some(crate::worker::TestReadGate::new(
                crate::PageKey {
                    source: key,
                    page_index: 3,
                },
                1,
                read_entered,
                resume_rx,
                None,
            )),
        })
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    start.send(()).unwrap();
    let mut domain = ConstructorStreamDomain::try_new_with_readiness_limits_on_control(
        control,
        pages,
        usize::MAX,
        usize::MAX,
        usize::MAX,
        Some(StreamReadinessReservation {
            extents_per_role: 3,
        }),
    )
    .ok()
    .unwrap();
    let mut loan = domain.loan_pages_on_control().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    for index in 0..3 {
        loan.pages.request_page(key, index);
        while loan.pages.page(key, index).is_none() {
            loan.pages.pump_ready(4);
            assert!(std::time::Instant::now() < deadline, "cache warmup stalled");
            std::thread::yield_now();
        }
        published.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    let current = [StreamPageExtent {
        source: key,
        page_index: 0,
        start: 0,
        end: 8,
    }];
    let initial_active = stamp(&owner, 1);
    loan.submit(ProtectionRole::Active, initial_active, &current)
        .unwrap();
    assert_eq!(
        loan.service(ProtectionRole::Active, initial_active, 4, 1),
        Ok(ReadinessProgress::InstalledProtected)
    );
    assert!(loan.acknowledge(ProtectionRole::Active, initial_active, &current));
    loan.pages.prefetch_page(key, 3);
    read_entered_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let candidate = stamp(&owner, 2);
    let candidate_extent = [StreamPageExtent {
        source: key,
        page_index: 3,
        start: 0,
        end: 8,
    }];
    loan.submit(ProtectionRole::Candidate, candidate, &candidate_extent)
        .unwrap();
    assert_eq!(
        loan.service(ProtectionRole::Candidate, candidate, 1, 1),
        Ok(ReadinessProgress::Pending)
    );
    assert_eq!(loan.pages.endpoint.stats().coalesced_requests, 1);
    let survivors: Vec<_> = (0..3)
        .filter(|&index| loan.pages.page(key, index).is_some())
        .collect();
    assert_eq!(
        survivors.len(),
        2,
        "real service must reclaim one slot before requesting the missing page"
    );
    assert!(loan.cancel(ProtectionRole::Candidate, candidate.request_epoch));
    assert!(
        survivors.contains(&0),
        "active protection must remain resident"
    );
    let active = stamp(&owner, 3);
    loan.submit(ProtectionRole::Active, active, &current)
        .unwrap();
    resume.send(()).unwrap();
    published.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        loan.service(ProtectionRole::Active, active, 4, 1),
        Ok(ReadinessProgress::InstalledProtected)
    );
    assert!(
        survivors
            .into_iter()
            .all(|index| loan.pages.page(key, index).is_some())
    );
    assert!(loan.acknowledge(ProtectionRole::Active, active, &current));
    let mut loan = Some(loan);
    domain.return_pages_on_control(&mut loan).unwrap();
    let report = finish(&mut domain);
    assert!(report.accounting_complete && report.worker_joined);
    owner.dispose_on_control().unwrap();
}

#[test]
fn current_quantum_reservation_preserves_resident_pages_and_accounts_for_capacity() {
    let Some(mut owner) = artifact() else { return };
    let (_wav, control, pages, key) = pair_with_frames(None, 24000);
    let mut domain = ConstructorStreamDomain::try_new_with_readiness_limits_on_control(
        control,
        pages,
        usize::MAX,
        usize::MAX,
        usize::MAX,
        Some(StreamReadinessReservation {
            extents_per_role: 3,
        }),
    )
    .ok()
    .unwrap();
    let mut loan = domain.loan_pages_on_control().unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    for index in 0..3 {
        loan.pages.request_page(key, index);
        while loan.pages.page(key, index).is_none() {
            loan.pages.pump_ready(4);
            assert!(std::time::Instant::now() < deadline, "cache warmup stalled");
            std::thread::yield_now();
        }
    }
    let extent = |page_index| StreamResourceExtent {
        source: key.into(),
        page_index,
        start: 0,
        end: 8,
    };
    assert!(matches!(
        loan.reserve_resident_current_quantum(&[extent(0), extent(0)]),
        Err(CurrentQuantumRefusal::InvalidExtents)
    ));
    assert!(matches!(
        loan.reserve_resident_current_quantum(&[StreamResourceExtent {
            end: hex_engine::STREAM_PAGE_FRAMES + 1,
            ..extent(0)
        }]),
        Err(CurrentQuantumRefusal::InvalidExtents)
    ));
    assert!(matches!(
        loan.reserve_resident_current_quantum(&[extent(0), extent(1), extent(2), extent(3)]),
        Err(CurrentQuantumRefusal::InvalidExtents)
    ));
    let pending = stamp(&owner, 1);
    loan.submit_resources(ProtectionRole::Transition, pending, &[extent(3)])
        .unwrap();
    assert!(matches!(
        loan.reserve_resident_current_quantum(&[extent(0), extent(1), extent(2)]),
        Err(CurrentQuantumRefusal::Capacity)
    ));
    assert!(loan.cancel(ProtectionRole::Transition, pending.request_epoch));
    let current = [extent(0), extent(1), extent(4)];
    let original = [
        loan.pages.page(key, 0).unwrap().frames[0],
        loan.pages.page(key, 1).unwrap().frames[0],
    ];
    #[cfg(feature = "allocation-probe")]
    let mut reserved = None;
    #[cfg(feature = "allocation-probe")]
    let allocations = allocation_counter::measure(|| {
        reserved = Some(loan.reserve_resident_current_quantum(&current));
    });
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    #[cfg(feature = "allocation-probe")]
    let (token, complete) = reserved.unwrap().ok().unwrap();
    #[cfg(not(feature = "allocation-probe"))]
    let (token, complete) = loan
        .reserve_resident_current_quantum(&current)
        .ok()
        .unwrap();
    assert!(
        !complete,
        "the absent page must remain visible to the caller"
    );
    assert!(matches!(
        loan.reserve_current_quantum(&current),
        Err(CurrentQuantumRefusal::Busy)
    ));
    let candidate = stamp(&owner, 2);
    let demand = [extent(3)];
    loan.submit_resources(ProtectionRole::Candidate, candidate, &demand)
        .unwrap();
    assert_eq!(
        loan.service(ProtectionRole::Candidate, candidate, 4, 1),
        Ok(ReadinessProgress::Pending)
    );
    let transition = stamp(&owner, 3);
    assert_eq!(
        loan.submit_resources(ProtectionRole::Transition, transition, &[extent(2)]),
        Err(ReadinessRefusal::Capacity)
    );
    while loan
        .service(ProtectionRole::Candidate, candidate, 4, 1)
        .unwrap()
        != ReadinessProgress::InstalledProtected
    {
        assert!(
            std::time::Instant::now() < deadline,
            "candidate did not arrive"
        );
        std::thread::yield_now();
    }
    assert!(loan.acknowledge_resources(ProtectionRole::Candidate, candidate, &demand));
    assert_eq!(loan.pages.page(key, 0).unwrap().frames[0], original[0]);
    assert_eq!(loan.pages.page(key, 1).unwrap().frames[0], original[1]);
    let mut owned = Some(loan);
    assert!(matches!(
        domain.return_pages_on_control(&mut owned),
        Err(AudioStreamStorageError::UnresolvedObligations)
    ));
    let mut loan = owned.take().unwrap();
    #[cfg(feature = "allocation-probe")]
    let mut released = false;
    #[cfg(feature = "allocation-probe")]
    let allocations = allocation_counter::measure(|| {
        released = loan.release_current_quantum(&token);
    });
    #[cfg(not(feature = "allocation-probe"))]
    let released = loan.release_current_quantum(&token);
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(released);
    assert!(!loan.release_current_quantum(&token));
    let (second, complete) = loan
        .reserve_resident_current_quantum(&[extent(0)])
        .ok()
        .unwrap();
    assert!(complete);
    assert!(
        !loan.release_current_quantum(&token),
        "stale token must not clear a new reservation"
    );
    assert!(loan.cancel_current_quantum());
    assert!(!loan.release_current_quantum(&second));
    assert!(!loan.cancel_current_quantum());
    assert!(matches!(
        loan.reserve_current_quantum(&[extent(4)]),
        Err(CurrentQuantumRefusal::NotResident)
    ));
    assert_eq!(
        loan.submit_resources(ProtectionRole::Transition, transition, &[extent(2)]),
        Ok(())
    );
    while loan
        .service(ProtectionRole::Transition, transition, 4, 1)
        .unwrap()
        != ReadinessProgress::InstalledProtected
    {
        assert!(
            std::time::Instant::now() < deadline,
            "worker turnover stalled"
        );
        std::thread::yield_now();
    }
    assert!(loan.acknowledge_resources(ProtectionRole::Transition, transition, &[extent(2)]));
    let mut loan = Some(loan);
    domain.return_pages_on_control(&mut loan).unwrap();
    let report = finish(&mut domain);
    assert!(report.accounting_complete && report.worker_joined);
    owner.dispose_on_control().unwrap();
}

#[test]
fn current_quantum_tokens_cannot_release_another_domain() {
    let (_wav_a, control_a, pages_a, _) = pair();
    let (_wav_b, control_b, pages_b, _) = pair();
    let build = |control, pages| {
        ConstructorStreamDomain::try_new_with_readiness_limits_on_control(
            control,
            pages,
            usize::MAX,
            usize::MAX,
            usize::MAX,
            Some(StreamReadinessReservation {
                extents_per_role: 3,
            }),
        )
        .ok()
        .unwrap()
    };
    let mut a = build(control_a, pages_a);
    let mut b = build(control_b, pages_b);
    let mut loan_a = a.loan_pages_on_control().unwrap();
    let mut loan_b = b.loan_pages_on_control().unwrap();
    let token_a = loan_a.reserve_current_quantum(&[]).ok().unwrap();
    let token_b = loan_b.reserve_current_quantum(&[]).ok().unwrap();
    assert!(!loan_a.release_current_quantum(&token_b));
    assert!(!loan_b.release_current_quantum(&token_a));
    assert!(matches!(
        loan_a.reserve_current_quantum(&[]),
        Err(CurrentQuantumRefusal::Busy)
    ));
    assert!(matches!(
        loan_b.reserve_current_quantum(&[]),
        Err(CurrentQuantumRefusal::Busy)
    ));
    assert!(loan_a.release_current_quantum(&token_a));
    assert!(loan_b.release_current_quantum(&token_b));
    a.return_pages_on_control(&mut Some(loan_a)).unwrap();
    b.return_pages_on_control(&mut Some(loan_b)).unwrap();
    assert!(finish(&mut a).accounting_complete);
    assert!(finish(&mut b).accounting_complete);
}

#[test]
fn pending_arrivals_survive_unrelated_cache_turnover_until_receipt_or_cancel() {
    let Some(mut owner) = artifact() else { return };
    let (_wav, control, pages, key) = pair_with_frames(None, 24000);
    let mut domain = ConstructorStreamDomain::try_new_with_readiness_limits_on_control(
        control,
        pages,
        usize::MAX,
        usize::MAX,
        usize::MAX,
        Some(StreamReadinessReservation {
            extents_per_role: 3,
        }),
    )
    .ok()
    .unwrap();
    let mut loan = domain.loan_pages_on_control().unwrap();
    let extent = |page_index| StreamPageExtent {
        source: key,
        page_index,
        start: 0,
        end: 8,
    };
    let active = [extent(0)];
    let active_stamp = stamp(&owner, 1);
    loan.submit(ProtectionRole::Active, active_stamp, &active)
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if loan
            .service(ProtectionRole::Active, active_stamp, 3, 3)
            .unwrap()
            == ReadinessProgress::InstalledProtected
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "active page did not arrive"
        );
        std::thread::yield_now();
    }
    assert!(loan.acknowledge(ProtectionRole::Active, active_stamp, &active));
    let pending = [extent(1), extent(2)];
    let candidate = stamp(&owner, 2);
    loan.submit(ProtectionRole::Candidate, candidate, &pending)
        .unwrap();
    // Only the first wanted page is requested. No receipt can be issued while
    // the second remains absent, even though the first must retain custody.
    loan.pages.request_page(key, 1);
    while loan.pages.page(key, 1).is_none() {
        #[cfg(feature = "allocation-probe")]
        let mut result = None;
        #[cfg(feature = "allocation-probe")]
        let allocations = allocation_counter::measure(|| {
            result = Some(loan.service(ProtectionRole::Candidate, candidate, 3, 0));
        });
        #[cfg(not(feature = "allocation-probe"))]
        let result = loan.service(ProtectionRole::Candidate, candidate, 3, 0);
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        #[cfg(feature = "allocation-probe")]
        let result = result.unwrap();
        assert_eq!(result, Ok(ReadinessProgress::Pending));
        assert!(
            std::time::Instant::now() < deadline,
            "first candidate page did not arrive"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        loan.service(ProtectionRole::Candidate, candidate, 0, 0),
        Ok(ReadinessProgress::Pending)
    );
    assert!(
        loan.protected_pages(ProtectionRole::Candidate, candidate, &pending)
            .is_none()
    );
    // Fill the remaining cache slot, then rotate unrelated pages through it.
    for index in [3, 4, 3, 4, 3, 4, 3, 4] {
        loan.pages.request_page(key, index);
        while loan.pages.page(key, index).is_none() {
            loan.pages.pump_ready(3);
            assert!(
                std::time::Instant::now() < deadline,
                "unrelated page did not arrive"
            );
            std::thread::yield_now();
        }
        assert!(
            loan.pages.page(key, 1).is_some(),
            "partially arrived candidate was evicted"
        );
        assert!(
            loan.protected_pages(ProtectionRole::Active, active_stamp, &active)
                .is_some()
        );
    }
    assert!(loan.cancel(ProtectionRole::Candidate, candidate.request_epoch));
    assert!(
        loan.protected_pages(ProtectionRole::Active, active_stamp, &active)
            .is_some()
    );
    let mut released = false;
    for index in [3, 4, 3, 4, 3, 4, 3, 4] {
        loan.pages.request_page(key, index);
        while loan.pages.page(key, index).is_none() {
            loan.pages.pump_ready(3);
            assert!(
                std::time::Instant::now() < deadline,
                "turnover after cancellation stalled"
            );
            std::thread::yield_now();
        }
        if loan.pages.page(key, 1).is_none() {
            released = true;
            break;
        }
    }
    assert!(released, "cancelled partial page remained protected");
    let mut loan = Some(loan);
    domain.return_pages_on_control(&mut loan).unwrap();
    let report = finish(&mut domain);
    assert!(report.accounting_complete && report.worker_joined);
    owner.dispose_on_control().unwrap();
}
#[test]
fn real_decode_needs_installed_protection_receipt_and_exact_loan_return_before_join() {
    exact_decode_receipts(false);
}
#[test]
fn ordinary_identity_needs_the_same_actual_installed_protection_and_loan_return() {
    exact_decode_receipts(true);
}
fn exact_decode_receipts(ordinary: bool) {
    let Some(mut owner) = artifact() else { return };
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let (publication_tx, publication_rx) = mpsc::sync_channel(2);
    let (_wav, control, pages, key) = pair_with_hooks(Some(crate::worker::TestWorkerHooks {
        read_gate: None,
        scalar_steps: None,
        entered: entered_tx,
        release: release_rx,
        publications: publication_tx,
    }));
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let pair_id = Arc::as_ptr(&control.pair_id);
    let mut domain = ConstructorStreamDomain::try_new_with_readiness_limits_on_control(
        control,
        pages,
        usize::MAX,
        usize::MAX,
        usize::MAX,
        Some(StreamReadinessReservation {
            extents_per_role: 3,
        }),
    )
    .ok()
    .unwrap();
    assert_eq!(
        domain.summary().shared_handle_bound,
        22 + domain.summary().kernel_handle_bound + domain.summary().data.worker.manifest_handles
    );
    let bytes = domain.metadata_bytes().unwrap();
    let mut loan = domain.loan_pages_on_control().unwrap();
    assert_eq!(Arc::as_ptr(&loan.pages.pair_id), pair_id);
    assert!(domain.loan_pages_on_control().is_none());
    assert_eq!(domain.metadata_bytes(), Some(bytes));
    assert!(domain.advance_disposal_on_control().unwrap().is_none());
    let extents = [
        StreamPageExtent {
            source: key,
            page_index: 0,
            start: 4094,
            end: 4096,
        },
        StreamPageExtent {
            source: key,
            page_index: 1,
            start: 0,
            end: 6,
        },
    ];
    let make_stamp = |epoch| {
        let mut value = stamp(&owner, epoch);
        if ordinary {
            value.artifact = ReadinessArtifact::Ordinary { publication: 11 };
        }
        value
    };
    let first = make_stamp(1);
    if ordinary {
        let mut invalid = first;
        invalid.artifact = ReadinessArtifact::Ordinary { publication: 0 };
        assert_eq!(
            loan.submit(ProtectionRole::Active, invalid, &extents),
            Err(ReadinessRefusal::InvalidExtents)
        );
    }
    loan.submit(ProtectionRole::Active, first, &extents)
        .unwrap();
    assert!(
        loan.protected_pages(ProtectionRole::Active, first, &extents)
            .is_none()
    );
    for extent in extents {
        loan.pages.request_page(extent.source, extent.page_index);
    }
    release_tx.send(()).unwrap();
    assert_eq!(
        publication_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        0
    );
    assert_eq!(
        publication_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        0
    );
    // Both actual decoded buffers are now queued, but neither is installed or
    // protected. A zero-pump service cannot manufacture readiness from decode.
    assert_eq!(
        loan.service(ProtectionRole::Active, first, 0, 0),
        Ok(ReadinessProgress::Pending)
    );
    assert!(
        loan.protected_pages(ProtectionRole::Active, first, &extents)
            .is_none()
    );
    // Decoding is actual worker work; only service's complete installed/protected
    // transaction can publish the bounded receipt. All caller operations measured.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        #[cfg(feature = "allocation-probe")]
        let mut result = None;
        #[cfg(feature = "allocation-probe")]
        let measured = allocation_counter::measure(|| {
            result = Some(loan.service(ProtectionRole::Active, first, 2, 2));
        });
        #[cfg(not(feature = "allocation-probe"))]
        let result = loan.service(ProtectionRole::Active, first, 2, 2);
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        #[cfg(feature = "allocation-probe")]
        let result = result.unwrap();
        if result.unwrap() == ReadinessProgress::InstalledProtected {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(
        loan.protected_pages(ProtectionRole::Active, first, &extents)
            .is_none()
    ); // unread receipt
    assert_eq!(
        loan.submit(ProtectionRole::Active, make_stamp(2), &extents),
        Err(ReadinessRefusal::ReceiptPending)
    );
    let mut foreign = first;
    foreign.artifact = if ordinary {
        owner.identity().into()
    } else {
        ReadinessArtifact::Ordinary { publication: 11 }
    };
    assert!(!loan.acknowledge(ProtectionRole::Active, foreign, &extents));
    foreign = first;
    foreign.device_epoch += 1;
    assert!(!loan.acknowledge(ProtectionRole::Active, foreign, &extents));
    let mut wrong = first;
    wrong.admitted_command_prefix += 1;
    assert!(!loan.acknowledge(ProtectionRole::Active, wrong, &extents));
    assert!(loan.acknowledge(ProtectionRole::Active, first, &extents));
    assert!(!loan.acknowledge(ProtectionRole::Active, first, &extents));
    assert!(
        loan.protected_pages(ProtectionRole::Active, first, &extents)
            .unwrap()
            .page(key, 1)
            .is_some()
    );
    let requests = loan.pages.stats().queued_requests;
    {
        let mut protected = loan
            .protected_pages(ProtectionRole::Active, first, &extents)
            .unwrap();
        assert!(protected.page(key, 2).is_none());
        protected.request_page(key, 2);
    }
    assert_eq!(loan.pages.stats().queued_requests, requests);
    let second = make_stamp(2);
    let over_pool = [
        StreamPageExtent {
            source: key,
            page_index: 2,
            start: 0,
            end: 1,
        },
        StreamPageExtent {
            source: key,
            page_index: 3,
            start: 0,
            end: 1,
        },
    ];
    assert_eq!(
        loan.submit(ProtectionRole::Candidate, second, &over_pool),
        Err(ReadinessRefusal::Capacity)
    );
    assert!(
        loan.protected_pages(ProtectionRole::Active, first, &extents)
            .is_some()
    );
    let mut wrong_rate = second;
    wrong_rate.device_rate = 44100;
    assert_eq!(
        loan.submit(ProtectionRole::Candidate, wrong_rate, &extents[..1]),
        Err(ReadinessRefusal::InvalidExtents)
    );
    loan.submit(ProtectionRole::Candidate, second, &extents[..1])
        .unwrap();
    assert_eq!(
        loan.service(ProtectionRole::Candidate, second, 2, 2),
        Ok(ReadinessProgress::InstalledProtected)
    );
    assert!(!loan.can_promote(ProtectionRole::Candidate, second, &extents[..1]));
    assert!(loan.acknowledge(ProtectionRole::Candidate, second, &extents[..1]));
    assert!(loan.cancel(ProtectionRole::Active, 1));
    assert!(
        loan.protected_pages(ProtectionRole::Candidate, second, &extents[..1])
            .is_some()
    );
    assert_eq!(
        loan.submit(ProtectionRole::Active, first, &extents),
        Err(ReadinessRefusal::StaleEpoch)
    );
    assert!(loan.matches_intent(ProtectionRole::Candidate, second, &extents[..1]));
    let actual_backing = loan.readiness.roles[ProtectionRole::Candidate.index()]
        .protected
        .as_ptr();
    assert!(!loan.promote_to_active(ProtectionRole::Candidate, wrong, &extents[..1]));
    #[cfg(feature = "allocation-probe")]
    let measured = allocation_counter::measure(|| {
        assert!(loan.promote_to_active(ProtectionRole::Candidate, second, &extents[..1]))
    });
    #[cfg(not(feature = "allocation-probe"))]
    assert!(loan.promote_to_active(ProtectionRole::Candidate, second, &extents[..1]));
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        loan.readiness.roles[ProtectionRole::Active.index()]
            .protected
            .as_ptr(),
        actual_backing
    );
    assert!(
        loan.protected_pages(ProtectionRole::Active, second, &extents[..1])
            .is_some()
    );
    assert!(
        loan.protected_pages(ProtectionRole::Candidate, second, &extents[..1])
            .is_none()
    );

    let mut loan = Some(loan);
    domain.return_pages_on_control(&mut loan).unwrap();
    assert!(loan.is_none());
    let report = finish(&mut domain);
    assert!(report.accounting_complete && report.worker_joined);
    assert_eq!(report.page_buffers, 4);
    owner.dispose_on_control().unwrap();
}
#[test]
fn readiness_reservation_and_wrong_pair_refusals_preserve_actual_owners() {
    let (_wav, control, pages, _) = pair();
    let identity = Arc::as_ptr(&control.pair_id);
    let existing_data = storage::closed_constructor_pair_data(&control, &pages)
        .unwrap()
        .listed_data_bytes()
        .unwrap();
    let mut refused = None;
    let attempt = || {
        refused = Some(
            ConstructorStreamDomain::try_new_with_readiness_limits_on_control(
                control,
                pages,
                existing_data,
                usize::MAX,
                usize::MAX,
                Some(StreamReadinessReservation {
                    extents_per_role: 3,
                }),
            ),
        );
    };
    #[cfg(feature = "allocation-probe")]
    let allocations = allocation_counter::measure(attempt);
    #[cfg(not(feature = "allocation-probe"))]
    attempt();
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (allocations.count_total, allocations.bytes_total),
        (1, std::mem::size_of::<StreamDomainSealRefusal>() as u64)
    );
    let refused = refused.unwrap().err().unwrap();
    assert_eq!(refused.reason, AudioStreamStorageError::ReservationExceeded);
    assert_eq!(Arc::as_ptr(&refused.control.pair_id), identity);
    let StreamDomainSealRefusal {
        control,
        pages,
        reason: _,
    } = *refused;
    let refusal = ConstructorStreamDomain::try_new_with_readiness_limits_on_control(
        control,
        pages,
        usize::MAX,
        usize::MAX,
        usize::MAX,
        Some(StreamReadinessReservation {
            extents_per_role: 4,
        }),
    )
    .err()
    .unwrap();
    assert_eq!(refusal.reason, AudioStreamStorageError::ReservationExceeded);
    assert_eq!(Arc::as_ptr(&refusal.control.pair_id), identity);
    let mut first = ConstructorStreamDomain::try_new_with_readiness_limits_on_control(
        refusal.control,
        refusal.pages,
        usize::MAX,
        usize::MAX,
        usize::MAX,
        Some(StreamReadinessReservation {
            extents_per_role: 3,
        }),
    )
    .ok()
    .unwrap();
    let (_other_wav, control, pages, _) = pair();
    let mut second = ConstructorStreamDomain::try_new_with_readiness_limits_on_control(
        control,
        pages,
        usize::MAX,
        usize::MAX,
        usize::MAX,
        Some(StreamReadinessReservation {
            extents_per_role: 3,
        }),
    )
    .ok()
    .unwrap();
    let mut a = first.loan_pages_on_control();
    let mut b = second.loan_pages_on_control();
    assert!(!a.as_ref().unwrap().same_catalog(b.as_ref().unwrap()));
    assert_eq!(
        first.return_pages_on_control(&mut b),
        Err(AudioStreamStorageError::PairMismatch)
    );
    assert!(a.is_some() && b.is_some());
    first.return_pages_on_control(&mut a).unwrap();
    second.return_pages_on_control(&mut b).unwrap();
    assert!(finish(&mut first).accounting_complete);
    assert!(finish(&mut second).accounting_complete);
}

#[test]
fn active_audio_and_transition_target_keep_distinct_protection_until_commit() {
    let Some(mut owner) = artifact() else { return };
    let (_wav, control, pages, key) = pair_with_frames(None, 24000);
    let mut domain = ConstructorStreamDomain::try_new_with_readiness_limits_on_control(
        control,
        pages,
        usize::MAX,
        usize::MAX,
        usize::MAX,
        Some(StreamReadinessReservation {
            extents_per_role: 3,
        }),
    )
    .ok()
    .unwrap();
    let mut loan = domain.loan_pages_on_control().unwrap();
    let extent = |page_index| StreamPageExtent {
        source: key,
        page_index,
        start: 0,
        end: 8,
    };
    let old = [extent(0)];
    let target = [extent(1)];
    let old_stamp = stamp(&owner, 1);
    let target_stamp = stamp(&owner, 2);
    loan.submit(ProtectionRole::Active, old_stamp, &old)
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if loan
            .service(ProtectionRole::Active, old_stamp, 3, 3)
            .unwrap()
            == ReadinessProgress::InstalledProtected
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "old page did not arrive"
        );
        std::thread::yield_now();
    }
    assert!(loan.acknowledge(ProtectionRole::Active, old_stamp, &old));
    let overflow = [extent(1), extent(2), extent(3)];
    assert_eq!(
        loan.submit(ProtectionRole::Transition, target_stamp, &overflow),
        Err(ReadinessRefusal::Capacity)
    );
    assert!(
        loan.protected_pages(ProtectionRole::Active, old_stamp, &old)
            .is_some()
    );
    loan.submit(ProtectionRole::Transition, target_stamp, &target)
        .unwrap();
    assert_eq!(
        loan.service(ProtectionRole::Transition, target_stamp, 0, 0),
        Ok(ReadinessProgress::Pending)
    );
    assert!(
        loan.protected_pages(ProtectionRole::Active, old_stamp, &old)
            .is_some()
    );
    let mut stale = target_stamp;
    stale.admitted_command_prefix += 1;
    assert!(!loan.acknowledge(ProtectionRole::Transition, stale, &target));
    loop {
        if loan
            .service(ProtectionRole::Transition, target_stamp, 3, 3)
            .unwrap()
            == ReadinessProgress::InstalledProtected
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "target page did not arrive"
        );
        std::thread::yield_now();
    }
    assert!(!loan.can_promote(ProtectionRole::Transition, target_stamp, &target));
    assert!(loan.acknowledge(ProtectionRole::Transition, target_stamp, &target));
    assert!(
        loan.protected_pages(ProtectionRole::Active, old_stamp, &old)
            .unwrap()
            .page(key, 0)
            .is_some()
    );
    assert!(
        loan.protected_pages(ProtectionRole::Transition, target_stamp, &target)
            .unwrap()
            .page(key, 1)
            .is_some()
    );
    assert!(!loan.promote_to_active(ProtectionRole::Transition, stale, &target));
    assert!(
        loan.protected_pages(ProtectionRole::Active, old_stamp, &old)
            .is_some()
    );
    assert!(loan.cancel(ProtectionRole::Transition, target_stamp.request_epoch));
    assert!(
        loan.protected_pages(ProtectionRole::Active, old_stamp, &old)
            .is_some()
    );
    let next = stamp(&owner, 3);
    loan.submit(ProtectionRole::Transition, next, &target)
        .unwrap();
    loop {
        if loan
            .service(ProtectionRole::Transition, next, 3, 3)
            .unwrap()
            == ReadinessProgress::InstalledProtected
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "replacement target did not arrive"
        );
        std::thread::yield_now();
    }
    assert!(loan.acknowledge(ProtectionRole::Transition, next, &target));
    {
        let mut prepared = loan
            .prepare_promotion(ProtectionRole::Transition, next, &target)
            .unwrap();
        assert!(prepared.pages().page(key, 1).is_some());
        // Dropping a prepared promotion must not switch custody.
    }
    assert!(
        loan.protected_pages(ProtectionRole::Active, old_stamp, &old)
            .is_some()
    );
    #[cfg(feature = "allocation-probe")]
    let measured = allocation_counter::measure(|| {
        loan.prepare_promotion(ProtectionRole::Transition, next, &target)
            .unwrap()
            .commit()
    });
    #[cfg(not(feature = "allocation-probe"))]
    loan.prepare_promotion(ProtectionRole::Transition, next, &target)
        .unwrap()
        .commit();
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(
        loan.protected_pages(ProtectionRole::Active, old_stamp, &old)
            .is_none()
    );
    assert!(
        loan.protected_pages(ProtectionRole::Active, next, &target)
            .is_some()
    );
    let mut loan = Some(loan);
    domain.return_pages_on_control(&mut loan).unwrap();
    let report = finish(&mut domain);
    assert!(report.accounting_complete && report.worker_joined);
    owner.dispose_on_control().unwrap();
}
