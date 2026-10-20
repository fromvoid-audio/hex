use super::*;
use crate::snapshot_exports::*;

fn limits() -> SnapshotExportReservation {
    SnapshotExportReservation {
        snapshot_records: 2,
        read_records: 4,
        max_bookkeeping_bytes: 16_384,
    }
}
fn view() -> Option<crate::service::ViewState> {
    let session = crate::Session::new(48_000, 2, 1024).unwrap();
    Some(crate::service::view_state_owned(
        &session,
        hex_engine::empty_midi_preview_on_control(),
        7,
        1,
        false,
        None,
    ))
}
fn metadata() -> Option<SourceEventMetadata> {
    Some(SourceEventMetadata {
        facts: SourceEventFacts {
            request_complete: true,
            computer_keyboard_complete: Some(19),
            ..SourceEventFacts::default()
        },
        status: Some("actual worker status".into()),
        error: None,
        audio_batch_failure: Some(AudioBatchMemberFailure {
            member: 2,
            path: "/not-opened/sample.wav".into(),
            explanation: "actual member error".into(),
        }),
        gesture_ack: None,
        tuning_gesture_ack: None,
        tuning_library: Some(TuningLibraryResult {
            operation_id: 21,
            selected_slot: Some(3),
            slots: vec![hex_storage::tuning_library::Slot {
                number: 3,
                name: Some("actual slot".into()),
                error: None,
            }],
            loaded_name: Some("actual scale".into()),
            error: None,
        }),
    })
}

#[test]
fn source_metadata_and_view_share_actual_read_and_retain_owning_children_until_last_ack() {
    let (mut family, key) = SessionExportFamily::try_new_on_control(limits()).unwrap();
    let mut source = view();
    let original = std::ptr::from_ref(&source.as_ref().unwrap().project);
    assert!(matches!(
        family
            .try_event_construction_on_control(&key)
            .unwrap()
            .insert_source_on_control(&mut source, &mut None),
        Err(ExportRecordError::Stale)
    ));
    assert_eq!(
        std::ptr::from_ref(&source.as_ref().unwrap().project),
        original
    );
    let mut event = metadata();
    let first = family
        .try_event_construction_on_control(&key)
        .unwrap()
        .insert_source_on_control(&mut source, &mut event)
        .unwrap();
    assert!(source.is_none() && event.is_none());
    let second = family.derive_read(&first).unwrap();
    let query = |read: SnapshotRead<'_>| {
        let event = read.event_metadata().unwrap();
        assert_eq!(event.status(), Some("actual worker status"));
        assert_eq!(event.facts().computer_keyboard_complete, Some(19));
        assert_eq!(event.audio_batch_failure().unwrap().member(), 2);
        assert_eq!(
            event
                .tuning_library()
                .unwrap()
                .slots()
                .next()
                .unwrap()
                .name(),
            Some("actual slot")
        );
        assert_eq!(
            read.canonical_source_context().unwrap().project_generation,
            1
        );
        read.revision()
    };
    assert_eq!(family.read_with(&first, query), Ok(7));
    let guard = family.try_inventory_guard_on_control().unwrap();
    let fields = guard.sampler_family_fields_on_control().unwrap();
    let budget = hex_engine::AudioStorageLedgerReservation {
        data_rows: 1024,
        handle_rows: 1024,
        root_rows: 16,
        max_retained_bytes: 4_000_000,
        max_construction_peak_bytes: 4_000_000,
    };
    let mut ledger = hex_engine::AudioRootStorageLedger::try_new_on_control(budget).unwrap();
    ledger
        .borrow_sampler_family_fields_on_control(fields.records)
        .unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    let actual = ledger.summary().unwrap();
    assert!(
        actual.data_bytes
            >= hex_engine::sampler_host_fields::SharedMetadataRef::constructor_bytes::<
                SourceEventMetadata,
            >()
            .unwrap()
    );
    assert!(
        actual.require_complete().is_err(),
        "no public retirement/admission receipt"
    );
    drop(ledger);
    drop(guard);
    family.abandon_on_control(&key).unwrap();
    family.release_read(first).unwrap();
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Pending)
    );
    assert_eq!(family.read_with(&second, query), Ok(7));
    family.release_read(second).unwrap();
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Disposed)
    );
    assert_eq!(
        family.cleanup_one_on_control(),
        Ok(SnapshotFamilyCleanup::Disposed)
    );
}
