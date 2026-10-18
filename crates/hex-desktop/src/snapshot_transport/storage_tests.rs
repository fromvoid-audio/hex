//! Actual table + sealed Box + guarded Session child inventory. The legacy
//! worker/Event owning path is deliberately still an unqualified outer root.
use super::*;
use hex_engine::{AudioRootStorageLedger, AudioStorageLedgerReservation};
use hex_session::{service::Service, snapshot_exports::SnapshotExportReservation};
use std::time::{Duration, Instant};

#[test]
fn registered_snapshot_job_and_guarded_children_inspection_allocates_and_frees_nothing() {
    let _serial = tests::TABLE_TEST_GATE.lock().unwrap();
    if !hex_engine::audio_storage_constructor_layout_supported()
        || !hex_engine::retained_control_job_box_layout_supported()
    {
        eprintln!("unsupported snapshot/job constructor; no positive inspection proof");
        return;
    }
    eprintln!(
        "SUPPORTED actual registered job, persistent table and guarded Session snapshot inventory"
    );
    hex_audio::reserve_retained_families_on_control(hex_audio::RetainedFamilyReservation {
        family_slots: 2,
        max_table_bytes: 16_384,
    })
    .unwrap();
    let service = Service::new(48_000, 8, 1024).unwrap();
    let initial = tests::receive(&service);
    let (family, key) = SessionExportFamily::try_new_on_control(SnapshotExportReservation {
        snapshot_records: 2,
        read_records: 2,
        max_bookkeeping_bytes: 16_384,
    })
    .unwrap();
    let stamp = family
        .insert_on_control(&key, &mut Some(tests::copy_view(&initial.state)))
        .unwrap();
    let mut pending = Some(family.reserve_read_on_control(&key, stamp).unwrap());
    let slot = hex_audio::reserve_retained_family_slot_on_control().unwrap();
    let id = register_family_on_control(
        &slot,
        &mut Some(family),
        hex_audio::retained_family_job_body_quote::<SessionExportFamily>(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let (data_bytes, allocations, checksum) = loop {
        let result = hex_audio::inspect_retained_family_on_control(id, |job, family| {
            family.inspect_queries_on_control(|queries| {
                let mut ledger =
                    AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                        data_rows: 512,
                        handle_rows: 512,
                        root_rows: 16,
                        max_retained_bytes: 4_000_000,
                        max_construction_peak_bytes: 4_000_000,
                    })
                    .unwrap();
                // Outer family holder guard precedes inner records guard. Both
                // stay held for actual inspection and all repeated queries.
                hex_audio::visit_retained_family_table_backing_on_control(&mut ledger).unwrap();
                job.visit_outer_storage_on_control(&mut ledger).unwrap();
                queries
                    .visit_selected_storage_on_control(&mut ledger)
                    .unwrap();
                let selected = ledger.summary().unwrap();
                assert!(selected.data_bytes > job.actual_body_layout_on_control().size());
                assert!(selected.data_allocations > 4);
                // Real legacy Event/worker still holds other physical fields.
                // A known job/record is not authority to ignore those roots.
                assert!(
                    ledger
                        .validate_selected_shared_handles_on_control()
                        .is_err()
                );
                assert!(selected.require_complete().is_err());
                // Join the ACTUAL live Event child fields under the same guard.
                // Its raw Arc parent and worker/prepared owners stay unqualified.
                initial
                    .state
                    .visit_children_selected_storage_on_control(&mut ledger)
                    .unwrap();
                let joined = ledger.summary().unwrap();
                assert_eq!(
                    selected.data_bytes, joined.data_bytes,
                    "shared child bodies deduplicate while physical fields join"
                );
                let mut checksum = 0;
                let measured = allocation_counter::measure(|| {
                    for _ in 0..128 {
                        hex_audio::visit_retained_family_table_backing_on_control(&mut ledger)
                            .unwrap();
                        job.visit_outer_storage_on_control(&mut ledger).unwrap();
                        queries
                            .visit_selected_storage_on_control(&mut ledger)
                            .unwrap();
                        initial
                            .state
                            .visit_children_selected_storage_on_control(&mut ledger)
                            .unwrap();
                        checksum += ledger.summary().unwrap().data_bytes;
                    }
                });
                assert_eq!(checksum, joined.data_bytes * 128);
                assert_eq!(
                    (
                        measured.count_total,
                        measured.bytes_total,
                        measured.count_current,
                        measured.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
                (joined.data_bytes, joined.data_allocations, checksum)
            })
        });
        match result {
            Ok(Ok(values)) => break values,
            Err(RetainedFamilyError::Busy) | Ok(Err(ExportRecordError::Busy))
                if Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(1))
            }
            result => panic!("registered guarded inventory failed: {result:?}"),
        }
    };
    eprintln!(
        "SUPPORTED guarded snapshot128 data_bytes={data_bytes} allocations={allocations} checksum={checksum}; measured total/net allocation/free zero"
    );
    // ACK is OUTSIDE corresponding writers/read-inspection guards. No native
    // endpoint/output exists, no GUI is awaited and no owning retry is queued.
    release_pending_on_control(id, &mut pending).unwrap();
    assert!(pending.is_none());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match hex_audio::read_retained_family(id, |family| family.abandon_on_control(&key)) {
            Ok(Ok(())) => break,
            Err(RetainedFamilyError::Busy) | Ok(Err(ExportRecordError::Busy))
                if Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(1))
            }
            result => panic!("producer close failed: {result:?}"),
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match hex_audio::read_retained_family(id, |_| ()) {
            Err(RetainedFamilyError::Stale) => break,
            Ok(()) | Err(RetainedFamilyError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            result => panic!("actual inspected family retirement did not finish: {result:?}"),
        }
    }
}
