use super::*;
use hex_engine::{AudioCandidatePreparation, AudioCandidateReservationQuote, AudioSourceContext};
use hex_project::{Command, Created, Editor, Project};

// Real nonempty historical audio-clip constructor fixture, not Sampler loading
// or cross-platform lock/accounting qualification. The existing owner remains
// alive throughout record cleanup. No OS/backend or filesystem work is started.
pub(in crate::snapshot_exports) fn compiled()
-> Option<(AudioCandidatePreparation, AudioCompiledRevision)> {
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return None;
    }
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "record".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "record.wav".into(),
            sample_rate: 48_000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 8,
            source_offset: 0,
            source_length: 8,
            looping: false,
        })
        .unwrap();
    let quote = AudioCandidateReservationQuote::for_project(editor.project(), 1).unwrap();
    let mut owner = AudioCandidatePreparation::try_new_on_control(
        1,
        quote
            .storage_reservation(8 * std::mem::size_of::<hex_engine::Frame>())
            .unwrap(),
    )
    .unwrap();
    owner
        .copy_resident_source_on_control(
            asset,
            &hex_engine::Audio::new(48_000, vec![[0.25; 2]; 8]).unwrap(),
        )
        .unwrap();
    owner
        .prepare_on_control(
            editor.project(),
            11,
            AudioSourceContext {
                project_generation: 2,
                editor_revision: 3,
            },
        )
        .unwrap();
    let reader = owner.register_compiled_reader_on_control().unwrap();
    Some((owner, reader))
}

#[test]
fn compiled_record_state_transfers_keep_actual_reader_until_explicit_retirement() {
    let Some((mut owner, reader)) = compiled() else {
        return;
    };
    let mut records = NativeCompiledRecords::new(1);
    assert_eq!(
        NativeCompiledRecords::inline_bytes(),
        std::mem::size_of_val(&records)
    );
    let stamp = records.reserve().unwrap();
    let identity = reader.identity();
    let mut source = Some(reader);
    records.install(stamp, &mut source).unwrap();
    assert!(source.is_none());
    assert_eq!(records.borrowed(stamp).unwrap().identity(), identity);
    assert_eq!(
        records.borrowed(stamp).unwrap().metadata().sample_rate,
        48_000
    );
    records.claim(stamp).unwrap();
    assert_eq!(records.claim(stamp), Err(Error::Stale));
    records.complete(stamp).unwrap();
    assert_eq!(records.complete(stamp), Err(Error::Stale));
    records.retire(stamp).unwrap();
    assert!(records.borrowed(stamp).is_err());
    assert_eq!(
        owner.cleanup_on_control(),
        Err(AudioStorageError::ExternalReadersPending)
    );
    assert!(records.cleanup_one().unwrap());
    assert!(!records.cleanup_one().unwrap());
    // Releasing the record is NOT a receipt or the parent owner's cleanup.
    assert!(!owner.failed_incomplete());
    owner.cleanup_on_control().unwrap();
    owner.cleanup_on_control().unwrap();
    let next = records.reserve().unwrap();
    assert_eq!(next.index, stamp.index);
    assert_ne!(next.generation, stamp.generation);
    assert_eq!(records.retire(stamp), Err(Error::Stale));
    records.retire(next).unwrap(); // cancelled reservation: no payload birth
    assert!(records.cleanup_one().unwrap());
}

#[test]
fn compiled_record_full_foreign_and_stale_refusals_preserve_producer_payload() {
    let Some((mut owner, reader)) = compiled() else {
        return;
    };
    let mut records = NativeCompiledRecords::new(9);
    let mut producer = Some(reader.try_duplicate_on_control().unwrap());
    let original_identity = producer.as_ref().unwrap().identity();
    let first = records.reserve().unwrap();
    let second = records.reserve().unwrap();
    assert_eq!(records.reserve(), Err(Error::Reservation));
    assert_eq!(producer.as_ref().unwrap().identity(), original_identity);
    assert_eq!(
        records.install(
            CompiledStamp {
                family: 10,
                ..first
            },
            &mut producer
        ),
        Err(Error::Stale)
    );
    assert_eq!(
        records.install(
            CompiledStamp {
                generation: first.generation + 1,
                ..first
            },
            &mut producer
        ),
        Err(Error::Stale)
    );
    assert_eq!(producer.as_ref().unwrap().identity(), original_identity);
    let mut primary = Some(reader);
    records.install(first, &mut primary).unwrap();
    records.install(second, &mut producer).unwrap();
    let mut refused = Some(
        records
            .borrowed(first)
            .unwrap()
            .try_duplicate_on_control()
            .unwrap(),
    );
    let held_identity = refused.as_ref().unwrap().identity();
    records.close();
    assert_eq!(records.reserve(), Err(Error::Closed));
    assert_eq!(records.install(first, &mut refused), Err(Error::Closed));
    assert_eq!(refused.as_ref().unwrap().identity(), held_identity);
    assert!(records.cleanup_one().unwrap());
    assert!(records.cleanup_one().unwrap());
    assert!(!records.cleanup_one().unwrap());
    assert_eq!(
        owner.cleanup_on_control(),
        Err(AudioStorageError::ExternalReadersPending)
    );
    drop(refused);
    owner.cleanup_on_control().unwrap();
}

fn budget() -> hex_engine::AudioStorageLedgerReservation {
    hex_engine::AudioStorageLedgerReservation {
        data_rows: 32,
        handle_rows: 32,
        root_rows: 4,
        max_retained_bytes: 1024 * 1024,
        max_construction_peak_bytes: 1024 * 1024,
    }
}

#[test]
fn compiled_record_visits_actual_handles_but_never_qualifies_omitted_constructor_custody() {
    let Some((mut owner, reader)) = compiled() else {
        return;
    };
    let mut records = NativeCompiledRecords::new(2);
    let stamp = records.reserve().unwrap();
    records.install(stamp, &mut Some(reader)).unwrap();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
    records.visit(&mut ledger).unwrap();
    let summary = ledger.summary().unwrap();
    assert_eq!(summary.data_allocations, 2); // actual revision + registration
    assert_eq!(summary.unresolved_handles, 2);
    // Neither the enclosing records body nor the original constructor custodian
    // is declared by this selected child visit. No fake positive closure.
    assert!(
        ledger
            .validate_selected_shared_handles_on_control()
            .is_err()
    );
    assert!(summary.require_complete().is_err());
    drop(ledger);
    records.close();
    assert!(records.cleanup_one().unwrap());
    owner.cleanup_on_control().unwrap();
}

#[test]
fn compiled_record_pending_failed_and_exhausted_slots_are_not_reusable_vacancies() {
    let mut records = NativeCompiledRecords::new(3);
    let stamp = records.reserve().unwrap();
    records.records[stamp.index].cleanup_pending = true;
    assert_eq!(records.retire(stamp), Err(Error::Stale));
    let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
    assert_eq!(
        records.visit(&mut ledger),
        Err(AudioStorageError::UnknownStorage)
    );
    drop(ledger);
    records.records[stamp.index].phase = Phase::Failed;
    records.close();
    assert!(!records.cleanup_one().unwrap());
    assert!(records.records[stamp.index].cleanup_pending);
    assert_eq!(records.reserve(), Err(Error::Closed));
    let mut exhausted = NativeCompiledRecords::new(4);
    exhausted.records[0].generation = u64::MAX;
    assert_eq!(exhausted.reserve(), Err(Error::Exhausted));
    assert_eq!(exhausted.records[0].phase, Phase::Vacant);
    assert!(exhausted.records[0].payload.is_none());
}

#[test]
fn compiled_same_family_lock_bounds_transfers_backpressure_and_scalar_queries() {
    use crate::snapshot_exports::{
        SessionExportFamily, SnapshotExportReservation, SnapshotFamilyCleanup,
    };
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SessionExportFamily>(); // no hidden Send-only panic payload/lock
    let Some((mut owner, reader)) = compiled() else {
        return;
    };
    let (mut family, key) =
        SessionExportFamily::try_new_native_on_control(SnapshotExportReservation {
            snapshot_records: 2,
            read_records: 2,
            max_bookkeeping_bytes: 16_384,
        })
        .unwrap();
    let stamp = family.reserve_compiled_on_control(&key).unwrap();
    let mut source = Some(reader);
    let guard = family.try_inventory_guard_on_control().unwrap();
    assert_eq!(
        family.insert_compiled_on_control(&key, stamp, &mut source),
        Err(Error::Busy)
    );
    assert!(source.is_some());
    drop(guard);
    family
        .insert_compiled_on_control(&key, stamp, &mut source)
        .unwrap();
    assert!(source.is_none());
    assert_eq!(
        family
            .compiled_metadata_on_control(stamp)
            .unwrap()
            .sample_rate,
        48_000
    );
    let second = family.reserve_compiled_on_control(&key).unwrap();
    assert_eq!(
        family.reserve_compiled_on_control(&key),
        Err(Error::Reservation)
    );
    family
        .duplicate_compiled_on_control(&key, stamp, second)
        .unwrap();
    assert_eq!(
        family.duplicate_compiled_on_control(&key, stamp, second),
        Err(Error::Stale)
    );
    family.claim_compiled_on_control(&key, stamp).unwrap();
    family.complete_compiled_on_control(&key, stamp).unwrap();
    assert_eq!(
        owner.cleanup_on_control(),
        Err(AudioStorageError::ExternalReadersPending)
    );
    family.abandon_on_control(&key).unwrap();
    assert_eq!(
        family.claim_compiled_on_control(&key, second),
        Err(Error::Closed)
    );
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Pending)
    );
    assert_eq!(
        owner.cleanup_on_control(),
        Err(AudioStorageError::ExternalReadersPending)
    );
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Pending)
    );
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Disposed)
    );
    assert!(family.compiled_metadata_on_control(stamp).is_err());
    owner.cleanup_on_control().unwrap();
    eprintln!(
        "SUPPORTED same registered family: real two compiled readers, held inventory Busy preserves producer, full/stale/generation, scalar queries, actual parent Pending until both releases"
    );
}

#[test]
fn compiled_same_family_inventory_observes_paired_fields_without_adopting_root() {
    use crate::snapshot_exports::{SessionExportFamily, SnapshotExportReservation};
    let Some((mut owner, reader)) = compiled() else {
        return;
    };
    let (mut family, key) =
        SessionExportFamily::try_new_native_on_control(SnapshotExportReservation {
            snapshot_records: 2,
            read_records: 2,
            max_bookkeeping_bytes: 16_384,
        })
        .unwrap();
    let stamp = family.reserve_compiled_on_control(&key).unwrap();
    family
        .insert_compiled_on_control(&key, stamp, &mut Some(reader))
        .unwrap();
    let guard = family.try_inventory_guard_on_control().unwrap();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
    guard
        .visit_selected_storage_on_control(&mut ledger)
        .unwrap();
    let summary = ledger.summary().unwrap();
    assert_eq!(summary.unresolved_handles, 2);
    assert!(
        ledger
            .validate_selected_shared_handles_on_control()
            .is_err()
    );
    drop(ledger);
    drop(guard);
    family.abandon_on_control(&key).unwrap();
    family.cleanup_one_on_control().unwrap();
    owner.cleanup_on_control().unwrap();
    eprintln!(
        "SUPPORTED actual family guard visits paired compiled fields; omitted constructor root still refuses"
    );
}

#[test]
fn compiled_same_family_cleanup_unwind_keeps_pending_charge_without_poison_or_retry() {
    use crate::snapshot_exports::{SessionExportFamily, SnapshotExportReservation};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let Some((mut owner, reader)) = compiled() else {
        return;
    };
    let (mut family, key) =
        SessionExportFamily::try_new_native_on_control(SnapshotExportReservation {
            snapshot_records: 2,
            read_records: 2,
            max_bookkeeping_bytes: 16_384,
        })
        .unwrap();
    let stamp = family.reserve_compiled_on_control(&key).unwrap();
    family
        .insert_compiled_on_control(&key, stamp, &mut Some(reader))
        .unwrap();
    family.abandon_on_control(&key).unwrap();
    RETIREMENT_HOOK
        .with(|hook| *hook.borrow_mut() = Some(Box::new(|| std::panic::panic_any(0x395_u64))));
    let payload = catch_unwind(AssertUnwindSafe(|| family.cleanup_one_on_control())).unwrap_err();
    RETIREMENT_HOOK.with(|hook| *hook.borrow_mut() = None);
    assert_eq!(*payload.downcast::<u64>().unwrap(), 0x395);
    assert_eq!(family.cleanup_one_on_control(), Err(Error::Stale));
    // The inner writer was released BEFORE the panic boundary. The latch,
    // not mutex poison or a manufactured vacancy, preserves failed custody.
    let guard = family.try_inventory_guard_on_control().unwrap();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
    assert_eq!(
        guard.visit_selected_storage_on_control(&mut ledger),
        Err(AudioStorageError::UnknownStorage)
    );
    drop(ledger);
    drop(guard);
    owner.cleanup_on_control().unwrap();
    assert_eq!(family.cleanup_one_on_control(), Err(Error::Stale));
    eprintln!(
        "SUPPORTED actual family retirement: pending-before-take, writer released before capability drop, first panic identity retained by caller, retry/inspection refuse"
    );
}

#[test]
fn compiled_same_family_inline_storage_is_in_exact_constructor_quote() {
    use crate::snapshot_exports::{
        SessionExportFamily, SnapshotExportRecords, SnapshotExportReservation,
    };
    let mut limits = SnapshotExportReservation {
        snapshot_records: 2,
        read_records: 2,
        max_bookkeeping_bytes: usize::MAX,
    };
    let records = SnapshotExportRecords::reservation_quote(limits).unwrap();
    let enclosing =
        std::mem::size_of::<SessionExportFamily>() - std::mem::size_of::<SnapshotExportRecords>();
    let exact = records.checked_add(enclosing).unwrap();
    limits.max_bookkeeping_bytes = exact - 1;
    assert!(matches!(
        SessionExportFamily::try_new_native_on_control(limits),
        Err(Error::Reservation)
    ));
    limits.max_bookkeeping_bytes = exact;
    let (mut family, key) = SessionExportFamily::try_new_native_on_control(limits).unwrap();
    // Enabling the two inline slots adds NO new backing or backend child.
    // Original constructor still checks actual snapshot/read Vec capacities.
    family.reserve_compiled_on_control(&key).unwrap();
    family.reserve_compiled_on_control(&key).unwrap();
    assert_eq!(
        family.reserve_compiled_on_control(&key),
        Err(Error::Reservation)
    );
    family.abandon_on_control(&key).unwrap();
    family.cleanup_one_on_control().unwrap();
    family.cleanup_one_on_control().unwrap();
    eprintln!(
        "SUPPORTED inline native compiled records + actual enclosing retirement destination quote={exact}; minus1 refused, exact constructs two bounded slots"
    );
}
