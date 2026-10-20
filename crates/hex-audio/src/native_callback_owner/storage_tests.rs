//! Real existing Guard/Terminal transport, not a model of its lifecycle. Until
//! the complete Root factory has a before-birth union, production reserve stays
//! explicitly unqualified. This fixture supplies finite local constructor caps.
use super::*;
use hex_engine::{
    AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation,
    sampler_host_fields::NativeCarrierStorageRegistration,
};

fn reserved<T>(
    value: T,
    retained: usize,
    peak: usize,
) -> Result<(Guard<T>, Terminal<T>, NativeCarrierStorageRegistration), AudioStorageError> {
    let bytes = NativeCarrierStorageRegistration::requested_bytes_on_control::<T>()?;
    let scratch = NativeCarrierStorageRegistration::construction_scratch_bytes_on_control::<T>()?
        .checked_add(size_of::<Guard<T>>())
        .and_then(|n| n.checked_add(size_of::<Terminal<T>>()))
        .ok_or(AudioStorageError::Overflow)?;
    if bytes > retained
        || bytes
            .checked_add(scratch)
            .ok_or(AudioStorageError::Overflow)?
            > peak
    {
        return Err(AudioStorageError::ReservationExceeded);
    }
    // Exactly the existing private fresh constructor, after both cap checks.
    let (mut guard, mut terminal) = reserve(value);
    let GuardState::Live { owner, cell } = &guard.state else {
        unreachable!()
    };
    let (registration, [body, producer, receiver]) =
        NativeCarrierStorageRegistration::pair_actual_fresh_carrier_on_control(
            owner.as_ref(),
            cell,
            terminal.cell.as_ref().unwrap(),
        )?;
    guard.storage = Some(producer);
    terminal.cell_storage = Some(receiver);
    terminal.body_storage = Some(body);
    Ok((guard, terminal, registration))
}

fn bound<T>() -> (usize, usize) {
    let bytes = NativeCarrierStorageRegistration::requested_bytes_on_control::<T>().unwrap();
    let peak = bytes
        + NativeCarrierStorageRegistration::construction_scratch_bytes_on_control::<T>().unwrap()
        + size_of::<Guard<T>>()
        + size_of::<Terminal<T>>();
    (bytes, peak)
}
fn data(registration: &NativeCarrierStorageRegistration) -> Result<usize, AudioStorageError> {
    let mut ledger = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        data_rows: 16,
        handle_rows: 16,
        root_rows: 4,
        max_retained_bytes: 1 << 20,
        max_construction_peak_bytes: 1 << 20,
    })?;
    registration.visit_storage_on_control(&mut ledger)?;
    ledger.validate_selected_shared_handles_on_control()?;
    Ok(ledger.summary()?.data_bytes)
}

#[test]
fn actual_guard_terminal_lease_pair_follows_box_to_control_and_each_physical_drop() {
    // Cell proves Send-only payload support without widening it to Sync.
    type Payload = std::cell::Cell<[u64; 7]>;
    fn is_send<T: Send>() {}
    is_send::<Guard<Payload>>();
    is_send::<Terminal<Payload>>();
    let (bytes, peak) = bound::<Payload>();
    for (retained, peak) in [(bytes - 1, peak), (bytes, peak - 1)] {
        let refusal = allocation_counter::measure(|| {
            assert!(matches!(
                reserved(Payload::new([29; 7]), retained, peak),
                Err(AudioStorageError::ReservationExceeded)
            ));
        });
        assert_eq!((refusal.count_total, refusal.bytes_total), (0, 0));
    }
    let mut actual = None;
    let allocated = allocation_counter::measure(|| {
        actual = Some(reserved(Payload::new([29; 7]), bytes, peak).unwrap());
    });
    assert_eq!(
        (allocated.count_total, allocated.bytes_total),
        (3, bytes as u64)
    );
    let (mut guard, mut terminal, registration) = actual.unwrap();
    assert_eq!(data(&registration).unwrap(), bytes);
    let address = guard.owner_ref().unwrap() as *const Payload;
    guard.owner_mut().unwrap().set([31; 7]);
    let closed = allocation_counter::measure(|| {
        guard.close();
        guard.close();
    });
    assert_eq!(
        (
            closed.count_total,
            closed.count_current,
            closed.bytes_total,
            closed.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(terminal.ready());
    assert_eq!(data(&registration).unwrap(), bytes);
    let owner = terminal.take().unwrap();
    assert_eq!(owner.as_ref() as *const Payload, address);
    assert_eq!(owner.get(), [31; 7]);
    let metadata = NativeCarrierStorageRegistration::metadata_requested_bytes_on_control().unwrap();
    assert_eq!(
        data(&registration).unwrap(),
        metadata + size_of::<Payload>()
    );
    terminal.begin_body_drop().unwrap();
    let dropped = allocation_counter::measure(|| drop(owner));
    assert_eq!(
        (
            dropped.count_total,
            dropped.count_current,
            dropped.bytes_current
        ),
        (0, -1, -(size_of::<Payload>() as i64))
    );
    // Pending persists until the actual existing outer control completion step.
    assert_eq!(data(&registration), Err(AudioStorageError::UnknownStorage));
    terminal.complete_body_drop().unwrap();
    assert_eq!(data(&registration).unwrap(), metadata);
    assert!(registration.is_normally_disposed_on_control());
    drop(terminal);
    drop(guard);
    assert_eq!(data(&registration).unwrap(), metadata);
    eprintln!(
        "SUPPORTED actual Audio Guard/Terminal Send-only Cell;3alloc{bytes}; both-minus1 beforebirth0alloc; callback close twice0alloc/free; actual cell unwrap followed by sameBox pending/normaldrop; immutable metadata{metadata} remains"
    );
}

#[test]
fn actual_body_destructor_panic_keeps_first_payload_and_failed_carrier_charge() {
    struct Fails {
        drops: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Drop for Fails {
        fn drop(&mut self) {
            self.drops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::panic::panic_any(137u64);
        }
    }
    let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (bytes, peak) = bound::<Fails>();
    let (mut guard, mut terminal, registration) = reserved(
        Fails {
            drops: drops.clone(),
        },
        bytes,
        peak,
    )
    .unwrap();
    guard.close();
    let owner = terminal.take().unwrap();
    terminal.begin_body_drop().unwrap();
    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(owner))).unwrap_err();
    assert_eq!(first.downcast_ref::<u64>(), Some(&137));
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
    for _ in 0..2 {
        assert_eq!(data(&registration), Err(AudioStorageError::UnknownStorage));
    }
    assert_eq!(
        registration.retained_metadata_bytes_on_control(),
        Err(AudioStorageError::UnknownStorage)
    );
    // Exactly the existing outer failure protocol: no complete/retry after a
    // destructive unwind; release merely latches failure in the retained record.
    drop(terminal);
    assert_eq!(data(&registration), Err(AudioStorageError::UnknownStorage));
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(first.downcast_ref::<u64>(), Some(&137));
    drop(guard);
}

#[test]
fn actual_registered_worker_constructor_binds_all_four_carriers_at_birth() {
    let limits = crate::AudioCandidatePublicationReservation {
        runtime_slots: 3,
        archive_slots: 3,
        member_rows: 4096,
        transfer_rows: 4096,
        receipt_rows: 4,
        data_rows: 4096,
        handle_rows: 2048,
        max_retained_data_bytes: 96 * 1024 * 1024,
        max_construction_peak_data_bytes: 96 * 1024 * 1024,
    };
    let (retained, peak) = crate::prepared::factory::native_birth_bound().unwrap();
    let exact = crate::AudioCandidatePublicationReservation {
        max_retained_data_bytes: retained,
        max_construction_peak_data_bytes: peak,
        ..limits
    };
    let mut actual = None;
    let birth = allocation_counter::measure(|| {
        let (control, callback) =
            crate::prepared::channel_with_factory_reservation(48000, None, Some(exact)).unwrap();
        let root = control.native_candidate_root();
        actual = Some((
            root,
            reserve_native_worker(control, callback, None).unwrap(),
        ));
    });
    assert_eq!(
        (birth.count_total, birth.bytes_total),
        (57, retained as u64)
    );
    let (root, (data, errors, terminals, client, worker)) = actual.unwrap();
    assert!(
        data.storage.is_some(),
        "actual data carrier has no constructor field lease"
    );
    assert!(
        errors.storage.is_some(),
        "actual error carrier has no constructor field lease"
    );
    assert!(
        client.guard().storage.is_some(),
        "actual client carrier has no constructor field lease"
    );
    assert!(
        worker.guard().storage.is_some(),
        "actual worker carrier has no constructor field lease"
    );
    let endpoints = terminals.endpoints.as_ref().unwrap();
    assert!(terminals.data.cell_storage.is_some() && terminals.data.body_storage.is_some());
    assert!(terminals.errors.cell_storage.is_some() && terminals.errors.body_storage.is_some());
    assert!(endpoints.client.cell_storage.is_some() && endpoints.client.body_storage.is_some());
    assert!(
        endpoints.publication.cell_storage.is_some()
            && endpoints.publication.body_storage.is_some()
    );
    root.test_native_root_header_fields().expect("actual callback/publication Root fields must close, not only their enclosing carrier Boxes");
    let before = root.test_native_carrier_facts().unwrap();
    assert_eq!(before, (4, carrier_constructor_bound().unwrap().0));
    let mut retirement = NativeRetirement::new(terminals, root.clone());
    root.test_native_root_header_fields()
        .expect("all three actual Root actor fields close");
    let omitted = root.clone();
    assert_eq!(
        root.test_native_root_header_fields(),
        Err(AudioStorageError::UnknownStorage),
        "an unobserved actual Root alias is not certified by actor metadata"
    );
    drop(omitted);
    root.test_native_root_header_fields().unwrap();
    let actor_drop = allocation_counter::measure(|| drop((data, errors, client, worker)));
    assert_eq!((actor_drop.count_total, actor_drop.count_current), (0, 0));
    crate::prepared::candidate_tests::finish(&mut retirement);
    root.test_native_root_header_fields()
        .expect("retirement field remains bound after its actual child disposal");
    drop(retirement);
    root.test_native_root_header_fields()
        .expect("normal actor release leaves no fabricated Root handle");
    assert_eq!(
        root.test_native_carrier_facts().unwrap(),
        (
            4,
            4 * NativeCarrierStorageRegistration::metadata_requested_bytes_on_control().unwrap()
        )
    );
    eprintln!(
        "SUPPORTED real queue+all FOUR carrier births57/{retained}bytes exactly typed local bound, peak{peak}; actual Root records close all physical carrier handle rows; actor release0alloc/free then actual NativeRetirement removes bodies/cells leaving4metadata. Payload-child/full native/Service admission NOT inferred"
    );
}

#[test]
fn actual_foreign_callback_root_refuses_before_any_carrier_birth() {
    let (left, left_callback) = crate::prepared::channel(48000).unwrap();
    let (right, right_callback) = crate::prepared::channel(48000).unwrap();
    let left_root = left.native_candidate_root();
    let right_root = right.native_candidate_root();
    let measured = allocation_counter::measure(|| {
        assert!(matches!(
            reserve_native_worker(left, right_callback, None),
            Err(hex_engine::Invalid("native worker callback source differs"))
        ));
    });
    assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
    assert!(!left_root.same_constructor_as(&right_root));
    drop((left_callback, right));
}

#[test]
fn actual_registered_root_keeps_failed_carrier_unknown_and_never_retries_destructor() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (control, callback) = crate::prepared::channel_with_factory_reservation(
        48000,
        None,
        Some(crate::AudioCandidatePublicationReservation {
            runtime_slots: 3,
            archive_slots: 3,
            member_rows: 4096,
            transfer_rows: 4096,
            receipt_rows: 4,
            data_rows: 4096,
            handle_rows: 2048,
            max_retained_data_bytes: 96 << 20,
            max_construction_peak_data_bytes: 96 << 20,
        }),
    )
    .unwrap();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, client, worker) =
        reserve_native_worker(control, callback, None).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    // Fault injection only: this additional test-owned probe is NOT covered by
    // the ordinary constructor budget or asserted as a complete payload visit.
    data.owner_mut().unwrap().probe = Some(DropProbe(Box::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        std::panic::panic_any(137_u64);
    })));
    let mut retirement = NativeRetirement::new(terminals, root.clone());
    drop((data, errors, client, worker));
    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| retirement.advance()))
        .unwrap_err();
    assert_eq!(first.downcast_ref::<u64>(), Some(&137));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        retirement.errors.is_some(),
        "unretired real error carrier stays rooted"
    );
    assert_eq!(
        root.test_native_carrier_facts(),
        Err(AudioStorageError::UnknownStorage)
    );
    assert!(retirement.advance().is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(first.downcast_ref::<u64>(), Some(&137));
}

#[test]
fn actual_native_status_wrong_actor_role_refuses_before_carrier_allocation() {
    for field in 0..2 {
        let (retained, peak) = crate::prepared::factory::native_birth_bound().unwrap();
        let limits = crate::AudioCandidatePublicationReservation {
            runtime_slots: 3,
            archive_slots: 3,
            member_rows: 128,
            transfer_rows: 128,
            receipt_rows: 3,
            data_rows: 512,
            handle_rows: 512,
            max_retained_data_bytes: retained,
            max_construction_peak_data_bytes: peak,
        };
        let (control, callback) =
            crate::prepared::channel_with_factory_reservation(48000, None, Some(limits)).unwrap();
        let root = control.native_candidate_root();
        let role = if field == 0 {
            status_owner::NATIVE_ERROR
        } else {
            status_owner::NATIVE_DATA
        };
        let wrong = root.duplicate_status(&control.status, role).unwrap();
        let mut refused = false;
        let birth = allocation_counter::measure(|| {
            refused = if field == 0 {
                reserve_for_root(
                    &root,
                    NativeCallbackOwner {
                        callback,
                        status: wrong,
                        probe: None,
                    },
                )
                .is_err()
            } else {
                reserve_for_root(
                    &root,
                    ErrorCallbackOwner {
                        status: wrong,
                        probe: None,
                    },
                )
                .is_err()
            };
        });
        assert!(
            refused,
            "actual native Status field{field} must match its exact actor role"
        );
        assert_eq!(birth.count_total, 0);
        assert_eq!(root.test_native_carrier_facts().unwrap().0, 0);
    }
}
