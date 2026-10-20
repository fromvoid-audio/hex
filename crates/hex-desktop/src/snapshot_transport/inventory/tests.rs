//! Actual three registered Session families, all guard lists, and selected
//! Event-child union. No output/backend/GUI or whole-root admission claim.
use super::*;
use crate::snapshot_transport::{self as transport, tests as fixtures};
use hex_session::{service::Service, snapshot_exports::SnapshotExportReservation};
use std::time::{Duration, Instant};

#[test]
fn three_registered_snapshot_families_and_vacancy_form_one_guarded_allocation_free_union() {
    let _serial = fixtures::TABLE_TEST_GATE.lock().unwrap();
    if !hex_engine::audio_storage_constructor_layout_supported()
        || !hex_engine::retained_control_job_box_layout_supported()
    {
        eprintln!("unsupported all-family storage constructors; no positive union proof");
        return;
    }
    eprintln!(
        "SUPPORTED actual three Session families, vacant outer and nonstatic inner guard Vec"
    );
    fixtures::initialize_test_table();
    let failed_releases = transport::failed_release_count();
    let service = Service::new(48_000, 8, 1024).unwrap();
    let initial = fixtures::receive(&service);
    let mut producers = Vec::new();
    let mut capabilities = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..3 {
        let (family, producer) =
            SessionExportFamily::try_new_on_control(SnapshotExportReservation {
                snapshot_records: 2,
                read_records: 2,
                max_bookkeeping_bytes: 16_384,
            })
            .unwrap();
        let snapshot = family
            .insert_on_control(&producer, &mut Some(fixtures::copy_view(&initial.state)))
            .unwrap();
        let mut read = Some(family.reserve_read_on_control(&producer, snapshot).unwrap());
        let slot = hex_audio::reserve_retained_family_slot_on_control().unwrap();
        let id = transport::register_family_on_control(
            &slot,
            &mut Some(family),
            hex_audio::retained_family_job_body_quote::<SessionExportFamily>(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let capability = loop {
            match transport::bind_registered_read_on_control(id, &mut read) {
                Ok(capability) => break capability,
                Err(transport::Error::Busy) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                result => panic!("actual family binding failed: {:?}", result.err()),
            }
        };
        assert!(read.is_none());
        producers.push(producer);
        ids.push(id);
        capabilities.push(Some(capability));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let (scratch, prepared_revisions) = loop {
        let result = with_snapshot_family_inventory_on_control(65_536, |inventory| {
            assert_eq!(inventory.configured_slots(), 4);
            assert_eq!(inventory.active_families(), 3);
            assert_eq!(inventory.incarnations_and_revisions().count(), 3);
            assert!(
                matches!(
                    hex_audio::reserve_retained_family_slot_on_control(),
                    Err(RetainedFamilyError::Reservation)
                ),
                "the ACTUAL vacant slot is guarded too"
            );
            let mut ledger = AudioRootStorageLedger::try_new_on_control(
                hex_engine::AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 512,
                    root_rows: 16,
                    max_retained_bytes: 4_000_000,
                    max_construction_peak_bytes: 4_000_000,
                },
            )
            .unwrap();
            inventory
                .visit_selected_storage_on_control(&mut ledger)
                .unwrap();
            let selected = ledger.summary().unwrap();
            // The legacy Event/worker still has physical owner fields. They
            // cannot be omitted merely because all registered families joined.
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            initial
                .state
                .visit_children_selected_storage_on_control(&mut ledger)
                .unwrap();
            let joined = ledger.summary().unwrap();
            use hex_engine::sampler_host_fields::{
                BodyRef, SamplerFamilyGuardFields, SamplerFamilyInventoryFields,
                SamplerOuterFamilyFields, VecRef,
            };
            struct AlteredFamilies<'a, 'g> {
                actual: &'a SnapshotFamilyInventory<'g>,
                mode: u8,
                foreign: Vec<u8>,
            }
            impl SamplerFamilyInventoryFields for AlteredFamilies<'_, '_> {
                fn body(&self) -> BodyRef<'_> {
                    self.actual.body()
                }
                fn outer(&self) -> &dyn SamplerOuterFamilyFields {
                    self.actual.outer()
                }
                fn guards(&self) -> VecRef<'_> {
                    if self.mode == 2 {
                        VecRef::of(&self.foreign)
                    } else {
                        self.actual.guards()
                    }
                }
                fn family(
                    &self,
                    index: usize,
                ) -> Result<SamplerFamilyGuardFields<'_>, hex_engine::AudioStorageError>
                {
                    match self.mode {
                        0 => Err(hex_engine::AudioStorageError::UnknownStorage),
                        1 => self
                            .actual
                            .family((index + 1) % self.actual.active_families()),
                        _ => self.actual.family(index),
                    }
                }
            }
            let reservation = hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 512,
                root_rows: 16,
                max_retained_bytes: 4_000_000,
                max_construction_peak_bytes: 4_000_000,
            };
            for mode in 0..3 {
                let altered = AlteredFamilies {
                    actual: &inventory,
                    mode,
                    foreign: Vec::new(),
                };
                let mut refusal = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
                let mut result = None;
                let measured = allocation_counter::measure(|| {
                    result =
                        Some(refusal.borrow_sampler_family_inventory_fields_on_control(&altered));
                });
                assert_eq!(
                    result.unwrap(),
                    Err(if mode == 0 {
                        hex_engine::AudioStorageError::UnknownStorage
                    } else {
                        hex_engine::AudioStorageError::SourceMismatch
                    })
                );
                assert_eq!((measured.count_total, measured.count_current), (0, 0));
                assert!(
                    refusal.summary().is_err(),
                    "failed prefix cannot be resumed as a ledger"
                );
            }
            for (retained_minus, peak_minus) in [(0, 0), (1, 0), (0, 1)] {
                let mut exact = reservation;
                exact.max_retained_bytes = joined.retained_bytes - retained_minus;
                exact.max_construction_peak_bytes = joined.construction_peak_bytes - peak_minus;
                let mut bounded = AudioRootStorageLedger::try_new_on_control(exact).unwrap();
                let result = inventory.visit_selected_storage_on_control(&mut bounded);
                if retained_minus == 0 && peak_minus == 0 {
                    result.unwrap();
                    initial
                        .state
                        .visit_children_selected_storage_on_control(&mut bounded)
                        .unwrap();
                    assert_eq!(bounded.summary().unwrap(), joined);
                } else {
                    assert_eq!(
                        result,
                        Err(hex_engine::AudioStorageError::ReservationExceeded)
                    );
                }
            }
            assert_eq!(selected.data_bytes, joined.data_bytes);
            assert!(joined.data_bytes > inventory.requested_scratch_bytes());
            let mut checksum = 0;
            let measured = allocation_counter::measure(|| {
                for _ in 0..128 {
                    inventory
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
            assert!(joined.require_complete().is_err());
            eprintln!(
                "SUPPORTED three-family128 data_bytes={} allocations={} checksum={checksum} total/net alloc/free0",
                joined.data_bytes, joined.data_allocations
            );
            // Durable release on another thread remains available while BOTH
            // kinds of structural read guards stay held. No native barrier.
            let capability = capabilities[0].take().unwrap();
            std::thread::scope(|scope| scope.spawn(move || drop(capability)).join().unwrap());
            assert_eq!(inventory.active_families(), 3);
            let mut observations = inventory.incarnations_and_revisions();
            let prepared = [
                observations.next().unwrap(),
                observations.next().unwrap(),
                observations.next().unwrap(),
            ];
            (inventory.requested_scratch_bytes(), prepared)
        });
        match result {
            Ok(values) => break values,
            Err(SnapshotFamilyInventoryError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            result => panic!("actual all-family union refused: {result:?}"),
        }
    };
    let mut called = false;
    assert_eq!(
        with_snapshot_family_inventory_on_control(scratch - 1, |_| {
            called = true;
        }),
        Err(SnapshotFamilyInventoryError::Reservation)
    );
    assert!(!called);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match with_snapshot_family_inventory_on_control(scratch, |inventory| {
            assert_eq!(inventory.requested_scratch_bytes(), scratch);
            assert_eq!(inventory.active_families(), 3);
        }) {
            Ok(()) => break,
            Err(SnapshotFamilyInventoryError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            result => panic!("exact actual guard capacity refused: {result:?}"),
        }
    }
    // A real structural insertion changes a protected preparation revision.
    // A previous scalar observation is never a reusable source/commit permit.
    let mut next = Some(fixtures::copy_view(&initial.state));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match hex_audio::read_retained_family(ids[0], |family| {
            family.insert_on_control(&producers[0], &mut next)
        }) {
            Ok(Ok(_)) => break,
            Err(RetainedFamilyError::Busy) | Ok(Err(ExportRecordError::Busy))
                if Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(1))
            }
            result => panic!("actual changed preparation insertion failed: {result:?}"),
        }
    }
    assert!(next.is_none());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match with_snapshot_family_inventory_on_control(65_536, |inventory| {
            inventory.check_prepared_revisions(&prepared_revisions)
        }) {
            Ok(Err(SnapshotFamilyInventoryError::Stale)) => break,
            Err(SnapshotFamilyInventoryError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            result => panic!("stale preparation must refuse: {result:?}"),
        }
    }
    // Guard release allows the formerly protected vacancy to reserve/cancel.
    let slot = hex_audio::reserve_retained_family_slot_on_control().unwrap();
    hex_audio::cancel_retained_family_slot_on_control(&slot).unwrap();
    drop(capabilities);
    for (id, producer) in ids.iter().zip(&producers) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match hex_audio::read_retained_family(*id, |family| family.abandon_on_control(producer))
            {
                Ok(Ok(())) => break,
                Err(RetainedFamilyError::Busy) | Ok(Err(ExportRecordError::Busy))
                    if Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(1))
                }
                result => panic!("actual producer close failed: {result:?}"),
            }
        }
    }
    assert_eq!(transport::failed_release_count(), failed_releases);
    for id in ids {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match hex_audio::read_retained_family(id, |_| ()) {
                Err(RetainedFamilyError::Stale) => break,
                Ok(()) | Err(RetainedFamilyError::Busy) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                result => panic!("actual family control retirement failed: {result:?}"),
            }
        }
    }
}
