use super::*;
use hex_stream::{
    IntentStatus, PageExtent, PageReadiness, ProtectionRole as Role, ReadinessProgress,
    ReadinessRefusal, ReadinessStamp, VerifiedPageEndpoint, WantedPageManifest,
};

pub(super) fn pair(
    wav: &TestWav,
    frames: usize,
    pages: usize,
) -> (hex_stream::StreamControl, PageReadiness, StreamSourceKey) {
    let reader = VerifiedWavReader::open(wav.path(), reader_limits(frames)).unwrap();
    let key = StreamSourceKey::new(401, asset_id(frames, 48_000));
    let metadata = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        frames as u64,
        frames as u64,
        hex_project::IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .unwrap();
    let mut runtime = StreamRuntime::new(
        StreamConfig::with_pool_pages(pages),
        vec![SourceDefinition::new(
            wav.path().into(),
            metadata,
            reader_limits(frames),
        )],
    )
    .unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let (control, pages_endpoint) = runtime.split().ok().unwrap();
    let endpoint =
        VerifiedPageEndpoint::new(pages_endpoint, control.catalog_lease().unwrap()).unwrap();
    (
        control,
        PageReadiness::new(endpoint, pages - 1).unwrap(),
        key,
    )
}

fn stamp(epoch: u64) -> ReadinessStamp {
    ReadinessStamp {
        request_epoch: epoch,
        artifact: 89,
        project_generation: 21,
        editor_revision: 9,
        device_epoch: 4,
        project_rate: 48_000,
        timeline_epoch: 17,
        performance_frame: 1234,
        arrangement_frame: 2345,
        window_frames: 256,
        active_revision: 54,
        route_generation: 2,
        admitted_command_prefix: 12,
    }
}

pub(super) fn manifest(
    control: &hex_stream::StreamControl,
    key: StreamSourceKey,
    epoch: u64,
    page: u64,
) -> WantedPageManifest {
    WantedPageManifest::new(
        control.catalog_lease().unwrap(),
        stamp(epoch),
        vec![PageExtent {
            key: PageKey {
                source: key,
                page_index: page,
            },
            start: 0,
            end: 256,
        }],
    )
    .unwrap()
}

pub(super) fn install(readiness: &mut PageReadiness, role: Role, wanted: &WantedPageManifest) {
    assert_eq!(readiness.submit(role, wanted), IntentStatus::Accepted);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match readiness.service(role, wanted.stamp(), 4, 4) {
            ReadinessProgress::InstalledProtected => break,
            ReadinessProgress::Pending(_) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(1))
            }
            other => panic!("installation did not finish: {other:?}"),
        }
    }
    assert!(readiness.installed(role, wanted));
}

fn shutdown(control: hex_stream::StreamControl, readiness: PageReadiness) {
    let (pages, catalog) = readiness.into_endpoint().into_parts();
    drop(catalog);
    let report = control.shutdown(pages).unwrap();
    assert!(report.worker_joined && report.accounting_complete);
    assert_eq!(
        report.page_buffers,
        report.queued_pages_reclaimed
            + report.held_return_pages_reclaimed
            + report.worker_pages_reclaimed
    );
}

#[test]
fn worker_decode_is_not_installation_or_a_receipt() {
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, 8192);
    let (control, mut readiness, key) = pair(&wav, pcm.len(), 4);
    let wanted = manifest(&control, key, 1, 0);
    assert_eq!(
        readiness.submit(Role::Candidate, &wanted),
        IntentStatus::Accepted
    );
    assert_eq!(
        readiness.submit(Role::Candidate, &wanted),
        IntentStatus::Pending
    );
    assert!(
        matches!(readiness.service(Role::Candidate, wanted.stamp(), 0, 1), ReadinessProgress::Pending(p) if p.accepted == 1)
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while readiness.decoded_pages_waiting() == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!readiness.installed(Role::Candidate, &wanted));
    assert!(readiness.acknowledge(Role::Candidate, &wanted).is_none());
    assert_eq!(
        readiness.service(Role::Candidate, wanted.stamp(), 1, 0),
        ReadinessProgress::InstalledProtected
    );
    let receipt = readiness.acknowledge(Role::Candidate, &wanted).unwrap();
    assert_eq!(receipt.stamp(), wanted.stamp());
    assert_eq!(receipt.role(), Role::Candidate);
    assert!(receipt.same_catalog(wanted.catalog()));
    assert!(readiness.acknowledge(Role::Candidate, &wanted).is_none());
    assert_eq!(
        &readiness.pages().page(key, 0).unwrap().frames[..256],
        &pcm[..256]
    );
    drop(wanted);
    shutdown(control, readiness);
}

#[test]
fn identical_keys_and_metadata_in_distinct_actual_catalogs_never_authorize_installation() {
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, 8192);
    let (control, mut readiness, key) = pair(&wav, pcm.len(), 4);
    let (other_control, other_readiness, other_key) = pair(&wav, pcm.len(), 4);
    assert_eq!(key, other_key);
    let wrong = manifest(&other_control, key, 1, 0);
    assert_eq!(
        readiness.submit(Role::Candidate, &wrong),
        IntentStatus::Refused(ReadinessRefusal::WrongCatalog)
    );
    let wanted = manifest(&control, key, 1, 0);
    install(&mut readiness, Role::Candidate, &wanted);
    assert!(readiness.acknowledge(Role::Candidate, &wrong).is_none());
    assert!(!readiness.installed(Role::Candidate, &wrong));
    assert!(readiness.acknowledge(Role::Candidate, &wanted).is_some());
    drop(wrong);
    drop(wanted);
    shutdown(control, readiness);
    shutdown(other_control, other_readiness);
}

#[test]
fn unread_receipts_and_cancelled_queued_success_preserve_other_roles() {
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, 8192);
    let (control, mut readiness, key) = pair(&wav, pcm.len(), 4);
    let active = manifest(&control, key, 1, 0);
    install(&mut readiness, Role::Active, &active);
    let replacement = manifest(&control, key, 2, 1);
    assert_eq!(
        readiness.submit(Role::Active, &replacement),
        IntentStatus::Refused(ReadinessRefusal::ReceiptPending)
    );
    assert!(readiness.installed(Role::Active, &active));
    assert!(readiness.acknowledge(Role::Active, &active).is_some());
    assert_eq!(
        readiness.submit(Role::Candidate, &replacement),
        IntentStatus::Accepted
    );
    readiness.service(Role::Candidate, replacement.stamp(), 0, 1);
    let deadline = Instant::now() + Duration::from_secs(2);
    while readiness.decoded_pages_waiting() == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!readiness.cancel(Role::Candidate, 1));
    assert!(readiness.cancel(Role::Candidate, 2));
    assert_eq!(
        readiness.service(Role::Candidate, replacement.stamp(), 4, 4),
        ReadinessProgress::Refused(ReadinessRefusal::StaleWindow)
    );
    readiness.pages().pump_ready(4);
    assert!(!readiness.installed(Role::Candidate, &replacement));
    assert!(
        readiness
            .acknowledge(Role::Candidate, &replacement)
            .is_none()
    );
    readiness.pages().unpin_all();
    assert!(readiness.installed(Role::Active, &active));
    // Same buffer can carry all roles; releasing one leaves other receipts valid.
    let candidate = manifest(&control, key, 3, 0);
    install(&mut readiness, Role::Candidate, &candidate);
    let transition = manifest(&control, key, 4, 0);
    install(&mut readiness, Role::Transition, &transition);
    assert!(readiness.cancel(Role::Candidate, 3));
    assert!(readiness.installed(Role::Active, &active));
    assert!(readiness.installed(Role::Transition, &transition));
    drop((active, replacement, candidate, transition));
    shutdown(control, readiness);
}

#[test]
fn capacity_and_stale_window_refusals_do_not_mutate_existing_protection_or_epochs() {
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, 8192);
    let (control, mut readiness, key) = pair(&wav, pcm.len(), 2);
    let active = manifest(&control, key, 1, 0);
    install(&mut readiness, Role::Active, &active);
    assert!(readiness.acknowledge(Role::Active, &active).is_some());
    let candidate = manifest(&control, key, 2, 1);
    assert_eq!(
        readiness.submit(Role::Candidate, &candidate),
        IntentStatus::Refused(ReadinessRefusal::Capacity)
    );
    let same_page = manifest(&control, key, 2, 0);
    assert_eq!(
        readiness.submit(Role::Candidate, &same_page),
        IntentStatus::Accepted
    );
    let expected = same_page.stamp();
    let mut stale = Vec::new();
    macro_rules! change {
        ($field:ident) => {{
            let mut value = expected;
            value.$field += 1;
            stale.push(value);
        }};
    }
    change!(request_epoch);
    change!(artifact);
    change!(project_generation);
    change!(editor_revision);
    change!(device_epoch);
    change!(project_rate);
    change!(timeline_epoch);
    change!(performance_frame);
    change!(arrangement_frame);
    change!(window_frames);
    change!(active_revision);
    change!(route_generation);
    change!(admitted_command_prefix);
    for stamp in stale {
        assert_eq!(
            readiness.service(Role::Candidate, stamp, 4, 4),
            ReadinessProgress::Refused(ReadinessRefusal::StaleWindow)
        );
        assert!(readiness.installed(Role::Active, &active));
        assert!(!readiness.installed(Role::Candidate, &same_page));
    }
    assert_eq!(
        readiness.service(Role::Candidate, expected, 4, 4),
        ReadinessProgress::InstalledProtected
    );
    assert!(readiness.acknowledge(Role::Candidate, &same_page).is_some());
    assert_eq!(
        readiness.submit(Role::Candidate, &same_page),
        IntentStatus::Refused(ReadinessRefusal::StaleEpoch)
    );
    drop((active, candidate, same_page));
    shutdown(control, readiness);
}

#[test]
fn manifest_validation_preserves_input_owners_and_capacity_on_every_refusal() {
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, 4096 + 17);
    let (control, readiness, key) = pair(&wav, pcm.len(), 4);
    let valid = PageExtent {
        key: PageKey {
            source: key,
            page_index: 1,
        },
        start: 3,
        end: 17,
    };
    let mut invalids = vec![
        vec![PageExtent { end: 18, ..valid }],
        vec![PageExtent { start: 17, ..valid }],
        vec![PageExtent { end: 4097, ..valid }],
        vec![valid, valid],
        vec![PageExtent {
            key: PageKey {
                page_index: u64::MAX,
                ..valid.key
            },
            ..valid
        }],
        vec![PageExtent {
            key: PageKey {
                source: StreamSourceKey::new(402, key.asset()),
                ..valid.key
            },
            ..valid
        }],
    ];
    let first = PageExtent {
        key: PageKey {
            page_index: 0,
            ..valid.key
        },
        ..valid
    };
    invalids.push(vec![valid, first]);
    for input in invalids {
        let mut extents = Vec::with_capacity(64);
        extents.extend(input.iter().copied());
        let pointer = extents.as_ptr();
        let capacity = extents.capacity();
        let catalog = control.catalog_lease().unwrap();
        let expected_catalog = control.catalog_lease().unwrap();
        let failure = WantedPageManifest::new(catalog, stamp(1), extents)
            .err()
            .unwrap();
        let (catalog, extents, reason) = failure.into_parts();
        assert_eq!(reason, ReadinessRefusal::InvalidManifest);
        assert!(catalog.same_catalog(&expected_catalog));
        assert_eq!(extents.as_ptr(), pointer);
        assert_eq!(extents.capacity(), capacity);
        assert_eq!(extents, input);
    }
    let mut invalid_stamp = stamp(1);
    invalid_stamp.project_rate = 44_100;
    assert!(
        WantedPageManifest::new(control.catalog_lease().unwrap(), invalid_stamp, vec![valid])
            .is_err()
    );
    invalid_stamp = stamp(1);
    invalid_stamp.performance_frame = u64::MAX;
    assert!(
        WantedPageManifest::new(control.catalog_lease().unwrap(), invalid_stamp, vec![valid])
            .is_err()
    );
    invalid_stamp = stamp(0);
    assert!(
        WantedPageManifest::new(control.catalog_lease().unwrap(), invalid_stamp, vec![valid])
            .is_err()
    );
    let valid =
        WantedPageManifest::new(control.catalog_lease().unwrap(), stamp(1), vec![valid]).unwrap();
    drop(valid);
    shutdown(control, readiness);
}

#[test]
fn pending_replacement_cancel_keeps_old_role_and_extra_manifest_owner_refuses_shutdown() {
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, 8192);
    let (control, mut readiness, key) = pair(&wav, pcm.len(), 4);
    let active = manifest(&control, key, 1, 0);
    install(&mut readiness, Role::Active, &active);
    readiness.acknowledge(Role::Active, &active).unwrap();
    let replacement = manifest(&control, key, 2, 1);
    assert_eq!(
        readiness.submit(Role::Active, &replacement),
        IntentStatus::Accepted
    );
    assert!(readiness.cancel(Role::Active, 2));
    assert!(readiness.installed(Role::Active, &active));
    drop(replacement);
    let (pages, catalog) = readiness.into_endpoint().into_parts();
    drop(catalog);
    let error = control.shutdown(pages).unwrap_err();
    let hex_stream::ShutdownError::Busy(busy) = error else {
        panic!("expected Busy")
    };
    assert_eq!(
        busy.external_references(),
        1,
        "only the original manifest lease remains"
    );
    let (control, mut pages) = busy.into_parts();
    assert_eq!(&pages.page(key, 0).unwrap().frames[..256], &pcm[..256]);
    drop(active);
    let report = control.shutdown(pages).unwrap();
    assert!(report.worker_joined && report.accounting_complete);
}

#[cfg(feature = "allocation-probe")]
#[test]
fn service_receipts_protection_and_refusals_have_zero_total_and_net_allocations() {
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, 8192);
    let (control, mut readiness, key) = pair(&wav, pcm.len(), 4);
    let active = manifest(&control, key, 1, 0);
    install(&mut readiness, Role::Active, &active);
    let candidate = manifest(&control, key, 2, 1);
    let transition = manifest(&control, key, 3, 1);
    let saturated = manifest(&control, key, 4, 0);
    let before = readiness.allocation_footprint();
    let request_info = allocation_counter::measure(|| {
        assert!(readiness.acknowledge(Role::Active, &active).is_some());
        assert_eq!(
            readiness.submit(Role::Candidate, &candidate),
            IntentStatus::Accepted
        );
        assert_eq!(
            readiness.submit(Role::Candidate, &candidate),
            IntentStatus::Pending
        );
        assert!(
            matches!(readiness.service(Role::Candidate, candidate.stamp(), 0, 1), ReadinessProgress::Pending(p) if p.accepted == 1)
        );
    });
    assert_eq!(request_info.count_total, 0);
    assert_eq!(request_info.bytes_total, 0);
    assert_eq!(request_info.count_current, 0);
    assert_eq!(request_info.bytes_current, 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    while readiness.decoded_pages_waiting() == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    let info = allocation_counter::measure(|| {
        assert_eq!(
            readiness.service(Role::Candidate, candidate.stamp(), 4, 4),
            ReadinessProgress::InstalledProtected
        );
        assert!(readiness.acknowledge(Role::Candidate, &candidate).is_some());
        assert_eq!(
            readiness.submit(Role::Transition, &transition),
            IntentStatus::Accepted
        );
        assert_eq!(
            readiness.service(Role::Transition, transition.stamp(), 4, 4),
            ReadinessProgress::InstalledProtected
        );
        assert!(readiness.installed(Role::Active, &active));
        assert!(readiness.cancel(Role::Candidate, 2));
        assert!(readiness.installed(Role::Transition, &transition));
        assert_eq!(
            readiness.submit(Role::Transition, &saturated),
            IntentStatus::Refused(ReadinessRefusal::ReceiptPending)
        );
        assert_eq!(
            readiness.service(Role::Active, saturated.stamp(), 4, 4),
            ReadinessProgress::Refused(ReadinessRefusal::StaleWindow)
        );
        readiness.pages().unpin_all();
        assert_eq!(readiness.pages().page(key, 0).unwrap().frames[0], pcm[0]);
    });
    assert_eq!(info.count_total, 0);
    assert_eq!(info.bytes_total, 0);
    assert_eq!(info.count_current, 0);
    assert_eq!(info.bytes_current, 0);
    assert_eq!(readiness.allocation_footprint(), before);
    drop((active, candidate, transition, saturated));
    shutdown(control, readiness);
}

#[test]
fn final_epoch_can_install_but_exhaustion_cannot_reset_after_cancellation() {
    let (wav, pcm) = TestWav::stereo_pcm16(48_000, 8192);
    let (control, mut readiness, key) = pair(&wav, pcm.len(), 4);
    let final_wanted = manifest(&control, key, u64::MAX, 0);
    install(&mut readiness, Role::Candidate, &final_wanted);
    assert!(
        readiness
            .acknowledge(Role::Candidate, &final_wanted)
            .is_some()
    );
    assert!(readiness.cancel(Role::Candidate, u64::MAX));
    // Control recovery/reconfiguration of this same pair cannot reset epochs.
    let endpoint = readiness.into_endpoint();
    let mut readiness = PageReadiness::new(endpoint, 3).unwrap();
    let reset = manifest(&control, key, 1, 0);
    assert_eq!(
        readiness.submit(Role::Candidate, &reset),
        IntentStatus::Refused(ReadinessRefusal::EpochExhausted)
    );
    drop((final_wanted, reset));
    shutdown(control, readiness);
}
