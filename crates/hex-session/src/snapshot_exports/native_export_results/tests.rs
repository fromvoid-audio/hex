use super::*;
use crate::service::ExportEvent;
use crate::snapshot_exports::ExportRecordError as Error;
fn family() -> (SessionExportFamily, SnapshotProducerKey) {
    SessionExportFamily::try_new_on_control(SnapshotExportReservation {
        snapshot_records: 1,
        read_records: 1,
        max_bookkeeping_bytes: 65536,
    })
    .unwrap()
}
fn path(value: &str, spare: usize) -> std::path::PathBuf {
    let mut path = std::path::PathBuf::from(value);
    path.reserve(spare);
    path
}
fn event(revision: u64) -> ExportEvent {
    ExportEvent {
        revision,
        path: path("case/Original.WAV", 127),
        result: Ok(hex_export::ExportReport {
            path: path("case/Original.WAV", 239),
            sample_rate: 44100,
            frames: 17,
            data_bytes: 68,
            recovery_path: Some(path("case/Previous.WAV", 383)),
        }),
    }
}
fn summary(
    family: &SessionExportFamily,
    retained: usize,
    peak: usize,
) -> Result<hex_engine::AudioRootStorageSummary, hex_engine::AudioStorageError> {
    let guard = family.try_inventory_guard_on_control().unwrap();
    let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(
        hex_engine::AudioStorageLedgerReservation {
            data_rows: 64,
            handle_rows: 64,
            root_rows: 8,
            max_retained_bytes: retained,
            max_construction_peak_bytes: peak,
        },
    )?;
    guard.visit_selected_storage_on_control(&mut ledger)?;
    ledger.validate_selected_shared_handles_on_control()?;
    ledger.summary()
}
#[test]
fn native_result_capacity_one_retains_second_payload_and_ack_does_not_drop_first() {
    let (mut family, _) = family();
    family.prepare_export_results_on_control().unwrap();
    let mut first = Some(event(4));
    family.send_export_result_on_control(&mut first).unwrap();
    assert!(first.is_none());
    let mut second = Some(event(5));
    let original = second
        .as_ref()
        .unwrap()
        .path
        .as_os_str()
        .as_encoded_bytes()
        .as_ptr();
    assert_eq!(
        family.send_export_result_on_control(&mut second),
        Err(Error::Reservation)
    );
    assert_eq!(
        second
            .as_ref()
            .unwrap()
            .path
            .as_os_str()
            .as_encoded_bytes()
            .as_ptr(),
        original
    );
    let actual = family
        .read_export_result_on_control(|read| {
            let report = read.result().unwrap();
            assert_eq!(read.path(), std::path::Path::new("case/Original.WAV"));
            assert_eq!(report.path(), read.path());
            assert_eq!(
                report.recovery_path(),
                Some(std::path::Path::new("case/Previous.WAV"))
            );
            (
                read.revision(),
                report.frames(),
                report.sample_rate(),
                report.data_bytes(),
            )
        })
        .unwrap()
        .unwrap();
    assert_eq!(actual, (4, 17, 44100, 68));
    assert!(
        family
            .records
            .read()
            .unwrap()
            .native_export_result
            .fields()
            .unwrap()
            .payload
            .is_some()
    );
    assert!(
        family
            .read_export_result_on_control(|_| panic!("duplicate result read"))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        family.send_export_result_on_control(&mut second),
        Err(Error::Reservation)
    );
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Pending)
    );
    assert!(family.retiring_export_result.vacant());
    family.send_export_result_on_control(&mut second).unwrap();
    assert_eq!(
        family
            .read_export_result_on_control(|read| read.revision())
            .unwrap(),
        Some(5)
    );
    family.cleanup_one_on_control().unwrap();
}
#[test]
fn native_result_writer_busy_and_abandon_preserve_refused_producer() {
    let (mut family, key) = family();
    family.prepare_export_results_on_control().unwrap();
    let mut pending = Some(event(7));
    {
        let _held = family.records.write().unwrap();
        assert_eq!(
            family.send_export_result_on_control(&mut pending),
            Err(Error::Busy)
        );
        assert!(pending.is_some());
    }
    family.send_export_result_on_control(&mut pending).unwrap();
    family.abandon_on_control(&key).unwrap();
    let mut late = Some(event(8));
    assert_eq!(
        family.send_export_result_on_control(&mut late),
        Err(Error::Closed)
    );
    assert!(late.is_some());
    // No registered owning reader escaped: abandonment may retire unread
    // metadata normally, not a Playback/Source/reader disposal surrogate.
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Pending)
    );
    assert!(family.records.read().unwrap().native_export_result.vacant());
    assert!(family.read_export_result_on_control(|_| ()).is_err());
}
#[test]
fn native_result_read_panic_retains_payload_and_never_replays_or_reuses() {
    let (mut family, _) = family();
    family.prepare_export_results_on_control().unwrap();
    family
        .send_export_result_on_control(&mut Some(event(9)))
        .unwrap();
    let original: Box<dyn std::any::Any + Send> = Box::new(931_u64);
    let address = (&*original as *const dyn std::any::Any) as *const () as usize;
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        family.read_export_result_on_control(|_| std::panic::resume_unwind(original))
    }))
    .unwrap_err();
    assert_eq!(
        (&*caught as *const dyn std::any::Any) as *const () as usize,
        address
    );
    assert_eq!(*caught.downcast::<u64>().unwrap(), 931);
    assert_eq!(
        family.read_export_result_on_control(|_| panic!("must not replay")),
        Err(Error::Stale)
    );
    assert_eq!(family.cleanup_one_on_control(), Err(Error::Stale));
    assert!(
        family
            .records
            .read()
            .unwrap()
            .native_export_result
            .test_retains_actual_payload()
    );
    assert!(summary(&family, 1_000_000, 1_000_000).is_err());
}
#[test]
fn native_result_actual_three_path_capacities_and_both_selected_limits() {
    let (family, _) = family();
    family.prepare_export_results_on_control().unwrap();
    let baseline = summary(&family, 1_000_000, 1_000_000).unwrap();
    let pending = event(11);
    let report = pending.result.as_ref().unwrap();
    let capacities = pending.path.capacity()
        + report.path.capacity()
        + report.recovery_path.as_ref().unwrap().capacity();
    family
        .send_export_result_on_control(&mut Some(pending))
        .unwrap();
    let actual = summary(&family, 1_000_000, 1_000_000).unwrap();
    assert_eq!(actual.retained_bytes - baseline.retained_bytes, capacities);
    assert!(
        summary(
            &family,
            actual.retained_bytes,
            actual.construction_peak_bytes
        )
        .is_ok()
    );
    assert!(
        summary(
            &family,
            actual.retained_bytes - 1,
            actual.construction_peak_bytes
        )
        .is_err()
    );
    assert!(
        summary(
            &family,
            actual.retained_bytes,
            actual.construction_peak_bytes - 1
        )
        .is_err()
    );
    assert!(
        family
            .records
            .read()
            .unwrap()
            .native_export_result
            .fields()
            .unwrap()
            .payload
            .is_some()
    );
}
#[test]
fn native_result_error_spare_capacity_is_actual_storage() {
    let (family, _) = family();
    family.prepare_export_results_on_control().unwrap();
    let baseline = summary(&family, 1_000_000, 1_000_000).unwrap();
    let mut message = String::from("original export error");
    message.reserve(701);
    let path = path("case/Error.WAV", 439);
    let expected = path.capacity() + message.capacity();
    family
        .send_export_result_on_control(&mut Some(ExportEvent {
            revision: 3,
            path,
            result: Err(message),
        }))
        .unwrap();
    let actual = summary(&family, 1_000_000, 1_000_000).unwrap();
    assert_eq!(actual.retained_bytes - baseline.retained_bytes, expected);
    assert_eq!(
        family
            .read_export_result_on_control(|read| read.result().err().unwrap().to_owned())
            .unwrap(),
        Some("original export error".into())
    );
}

#[test]
fn native_result_concurrent_reader_cannot_duplicate_ack_or_let_writer_take_payload() {
    let (family, _) = family();
    family.prepare_export_results_on_control().unwrap();
    family
        .send_export_result_on_control(&mut Some(event(19)))
        .unwrap();
    let (entered, entry) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let owner = &family;
        let first = scope.spawn(move || {
            owner.read_export_result_on_control(|read| {
                entered.send(()).unwrap();
                released.recv().unwrap();
                read.revision()
            })
        });
        entry
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let second = family.read_export_result_on_control(|_| panic!("second callback"));
        let mut pending = Some(event(20));
        let busy = family.send_export_result_on_control(&mut pending);
        let retained = pending.is_some();
        release.send(()).unwrap();
        let first_result = first.join().unwrap();
        assert_eq!(first_result, Ok(Some(19)));
        assert!(second.unwrap().is_none());
        assert_eq!(busy, Err(Error::Busy));
        assert!(retained);
    });
    assert!(
        family
            .read_export_result_on_control(|_| panic!("third callback"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn native_result_inline_and_retirement_bodies_fit_only_actual_constructor_quote() {
    let mut limits = SnapshotExportReservation {
        snapshot_records: 1,
        read_records: 1,
        max_bookkeeping_bytes: usize::MAX,
    };
    let quote = SnapshotExportRecords::reservation_quote(limits).unwrap()
        + std::mem::size_of::<SessionExportFamily>()
        - std::mem::size_of::<SnapshotExportRecords>();
    limits.max_bookkeeping_bytes = quote - 1;
    assert!(matches!(
        SessionExportFamily::try_new_on_control(limits),
        Err(Error::Reservation)
    ));
    limits.max_bookkeeping_bytes = quote;
    let (family, _) = SessionExportFamily::try_new_on_control(limits).unwrap();
    family.prepare_export_results_on_control().unwrap();
    assert_eq!(family.read_export_result_on_control(|_| ()), Ok(None));
}
