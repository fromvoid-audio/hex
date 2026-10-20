use super::*;
fn ledger(rows: usize) -> AudioRootStorageLedger<'static> {
    AudioRootStorageLedger::try_new_on_control(crate::AudioStorageLedgerReservation {
        data_rows: 16,
        handle_rows: rows,
        root_rows: 4,
        max_retained_bytes: 1 << 20,
        max_construction_peak_bytes: 1 << 20,
    })
    .unwrap()
}
fn facts(record: &NativeAtomicSignalRegistration) -> Result<(usize, usize), AudioStorageError> {
    let mut ledger = ledger(16);
    record.visit_storage_on_control(&mut ledger)?;
    ledger.validate_selected_shared_handles_on_control()?;
    Ok((
        ledger.summary()?.data_bytes,
        ledger.selected_shared_counts_on_control().1,
    ))
}
#[test]
fn actual_signal_constructor_pair_handles_anchor_and_zero_callback_allocation() {
    let bytes = NativeAtomicSignalRegistration::requested_bytes_on_control().unwrap();
    let mut pair = None;
    let measured = allocation_counter::measure(|| {
        pair = Some(NativeAtomicSignalRegistration::fresh_pair_on_control(3).unwrap())
    });
    assert_eq!(
        (measured.count_total, measured.bytes_total),
        (1, bytes as u64)
    );
    let (record, [first, second]) = pair.unwrap();
    assert!(record.matches_actual_fields_on_control([&first, &second]));
    assert_eq!(facts(&record).unwrap(), (bytes, 5));
    let measured = allocation_counter::measure(|| {
        first.store(17, Ordering::Release);
        assert_eq!(second.load(Ordering::Acquire), 17);
        second.fetch_add(4, Ordering::AcqRel);
        assert_eq!(first.load(Ordering::Relaxed), 21);
        drop(first);
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
    assert_eq!(facts(&record).unwrap(), (bytes, 3));
    let measured = allocation_counter::measure(|| drop(second));
    assert_eq!(
        (
            measured.count_total,
            measured.count_current,
            measured.bytes_total,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(facts(&record).unwrap(), (bytes, 1));
    let measured = allocation_counter::measure(|| drop(record));
    assert_eq!(
        (measured.count_current, measured.bytes_current),
        (-1, -(bytes as i64))
    );
    eprintln!(
        "SUPPORTED actualAtomicCore1alloc/{bytes}B,5→3→1individual owning fields accounted; atomicops+bothfielddrops0alloc/free, actualanchor-finaldrop1free; no wholeService admission"
    );
}
#[test]
fn actual_distinct_equal_signal_owners_foreign_pair_and_omitted_handle_refuse() {
    let (a, [left, right]) = NativeAtomicSignalRegistration::fresh_pair_on_control(0).unwrap();
    let (b, [other_left, other_right]) =
        NativeAtomicSignalRegistration::fresh_pair_on_control(0).unwrap();
    let matching = allocation_counter::measure(|| {
        assert!(a.matches_actual_field_on_control(&left, 0));
        assert!(a.matches_actual_field_on_control(&right, 1));
        assert!(!a.matches_actual_field_on_control(&left, 1));
        assert!(!a.matches_actual_field_on_control(&right, 0));
        assert!(!a.matches_actual_field_on_control(&other_left, 0));
        assert!(!a.matches_actual_field_on_control(&other_right, 1));
        assert!(!a.matches_actual_field_on_control(&left, usize::MAX));
    });
    assert_eq!((matching.count_total, matching.count_current), (0, 0));
    assert!(!a.matches_actual_fields_on_control([&left, &other_right]));
    assert!(!a.matches_actual_fields_on_control([&right, &left]));
    assert!(b.matches_actual_fields_on_control([&other_left, &other_right]));
    let mut inventory = ledger(16);
    a.visit_storage_on_control(&mut inventory).unwrap();
    b.visit_storage_on_control(&mut inventory).unwrap();
    inventory
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert_eq!(
        inventory.summary().unwrap().data_bytes,
        2 * NativeAtomicSignalRegistration::requested_bytes_on_control().unwrap()
    );
    // Counterfactual omitted actual alias, never a forged callback slot/count.
    inventory.handles.pop().unwrap();
    assert!(
        inventory
            .validate_selected_shared_handles_on_control()
            .is_err()
    );
    drop(inventory);
    let extra = a.core.clone(); // only this private negative can access the owner
    assert!(facts(&a).is_err());
    drop(extra);
    drop((left, right, other_left, other_right));
}
#[test]
fn actual_signal_row_shortage_and_interrupted_release_stay_refused() {
    let (record, [first, second]) =
        NativeAtomicSignalRegistration::fresh_pair_on_control(0).unwrap();
    let mut small = ledger(4);
    assert!(record.visit_storage_on_control(&mut small).is_err());
    drop(small); // never reuse partially visited invalid inventory
    record.core.phases[0].store(DROP_PENDING, Ordering::Release);
    assert!(matches!(
        facts(&record),
        Err(AudioStorageError::UnknownStorage)
    ));
    drop(first); // interrupted prior step cannot be retried as a normal release
    assert_eq!(record.core.phases[0].load(Ordering::Acquire), FAILED);
    assert!(facts(&record).is_err());
    drop(second);
    assert!(facts(&record).is_err());
}
#[test]
fn actual_signal_local_exact_and_each_minus_one_refuse_before_ids_and_birth() {
    let bytes = NativeAtomicSignalRegistration::requested_bytes_on_control().unwrap();
    let peak =
        bytes + NativeAtomicSignalRegistration::construction_scratch_bytes_on_control().unwrap();
    for (retained_limit, peak_limit) in [(bytes - 1, peak), (bytes, peak - 1)] {
        let ids = crate::audio_only_inputs::ISSUED_HANDLES.with(|v| v.get());
        let measured = allocation_counter::measure(|| {
            assert!(construct_limited(retained_limit, peak_limit).is_err());
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
        assert_eq!(
            crate::audio_only_inputs::ISSUED_HANDLES.with(|v| v.get()),
            ids
        );
    }
    let (_, fields) = construct_limited(bytes, peak).unwrap();
    drop(fields);
}
#[test]
fn actual_signal_field_moves_threads_without_cloning_identity_or_waiting() {
    let (record, [control, callback]) =
        NativeAtomicSignalRegistration::fresh_pair_on_control(0).unwrap();
    let measured = std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            allocation_counter::measure(|| {
                for _ in 0..128 {
                    callback.fetch_add(1, Ordering::AcqRel);
                }
                drop(callback);
            })
        });
        for _ in 0..128 {
            control.fetch_add(1, Ordering::AcqRel);
        }
        worker.join().unwrap()
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
    assert_eq!(control.load(Ordering::Acquire), 256);
    assert_eq!(facts(&record).unwrap().1, 3);
    drop(control);
    assert_eq!(facts(&record).unwrap().1, 1);
}
fn construct_limited(
    retained: usize,
    peak: usize,
) -> Result<(NativeAtomicSignalRegistration, [NativeAtomicSignal; 2]), AudioStorageError> {
    let bytes = NativeAtomicSignalRegistration::requested_bytes_on_control()?;
    if bytes > retained
        || bytes
            .checked_add(NativeAtomicSignalRegistration::construction_scratch_bytes_on_control()?)
            .ok_or(AudioStorageError::Overflow)?
            > peak
    {
        return Err(AudioStorageError::ReservationExceeded);
    }
    NativeAtomicSignalRegistration::fresh_pair_on_control(0)
}

#[test]
fn safe_stop_operations_preserve_roles_anchor_and_zero_allocations() {
    let (record, [first, second]) =
        NativeAtomicSignalRegistration::fresh_pair_on_control(0).unwrap();
    let expected = NativeAtomicSignalRegistration::requested_bytes_on_control().unwrap();
    let measured = allocation_counter::measure(|| {
        assert_eq!(first.try_load_stop_on_control(), Ok(false));
        assert_eq!(
            first.try_value_identity_on_control(),
            second.try_value_identity_on_control()
        );
        first.try_request_stop_on_control().unwrap();
        second.try_request_stop_on_control().unwrap();
        assert_eq!(second.try_load_stop_on_control(), Ok(true));
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
    assert_eq!(facts(&record).unwrap(), (expected, 5));
    drop(first);
    drop(second);
    assert_eq!(facts(&record).unwrap(), (expected, 1));
}
#[test]
fn request_first_nonboolean_stop_refuses_without_normalizing_and_stays_failed() {
    let (record, [first, second]) =
        NativeAtomicSignalRegistration::fresh_pair_on_control(2).unwrap();
    let measured = allocation_counter::measure(|| {
        assert_eq!(
            first.try_request_stop_on_control(),
            Err(AudioStorageError::UnknownStorage)
        );
        assert_eq!(record.core.value.load(Ordering::Acquire), 2);
        assert_eq!(
            first.try_request_stop_on_control(),
            Err(AudioStorageError::UnknownStorage)
        );
        assert_eq!(
            first.try_load_stop_on_control(),
            Err(AudioStorageError::UnknownStorage)
        );
        assert_eq!(
            second.try_load_stop_on_control(),
            Err(AudioStorageError::UnknownStorage)
        );
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
    assert_eq!(facts(&record), Err(AudioStorageError::UnknownStorage));
}

#[test]
fn safe_stop_missing_or_nonboolean_value_refuses_without_panic_and_stays_failed() {
    let (record, [first, _second]) =
        NativeAtomicSignalRegistration::fresh_pair_on_control(2).unwrap();
    assert_eq!(
        first.try_load_stop_on_control(),
        Err(AudioStorageError::UnknownStorage)
    );
    assert_eq!(
        first.try_request_stop_on_control(),
        Err(AudioStorageError::UnknownStorage)
    );
    assert_eq!(facts(&record), Err(AudioStorageError::UnknownStorage));
    let (record, [mut first, _second]) =
        NativeAtomicSignalRegistration::fresh_pair_on_control(0).unwrap();
    drop(first.value.take());
    let measured = allocation_counter::measure(|| {
        assert_eq!(
            first.try_load_stop_on_control(),
            Err(AudioStorageError::UnknownStorage)
        );
        assert_eq!(
            first.try_request_stop_on_control(),
            Err(AudioStorageError::UnknownStorage)
        );
        assert_eq!(
            first.try_value_identity_on_control(),
            Err(AudioStorageError::UnknownStorage)
        );
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
    assert_eq!(facts(&record), Err(AudioStorageError::UnknownStorage));
}
