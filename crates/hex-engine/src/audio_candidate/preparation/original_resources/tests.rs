use super::*;
use crate::native_device_resources::{OriginalResourceInput, ResourceCatalogReservation};
use std::sync::atomic::{AtomicUsize, Ordering};
struct Query {
    calls: AtomicUsize,
    cancel: bool,
    panic: bool,
}
impl crate::CancellationQuery for Query {
    fn is_cancelled(&self) -> bool {
        self.calls.fetch_add(1, Ordering::Relaxed);
        assert!(
            !self.panic,
            "injected original producer cancellation-query panic"
        );
        self.cancel
    }
}
fn query(cancel: bool, panic: bool) -> Query {
    Query {
        calls: AtomicUsize::new(0),
        cancel,
        panic,
    }
}
fn outer() -> crate::AudioStorageReservation {
    crate::AudioStorageReservation {
        allocation_items: 256,
        handle_obligations: 128,
        max_prepared_data_bytes: 16 * 1024 * 1024,
        max_construction_data_bytes: 16 * 1024 * 1024,
        caller_retained_bytes: 0,
        caller_construction_bytes: 0,
    }
}
fn allowance() -> OriginalResourcePlanReservation {
    OriginalResourcePlanReservation {
        inputs: crate::AudioStorageReservation {
            allocation_items: 16,
            handle_obligations: 16,
            max_prepared_data_bytes: 65536,
            max_construction_data_bytes: 65536,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
        catalog: ResourceCatalogReservation {
            metadata_bytes: 4096,
            hash_frames: 4,
        },
        max_preparation_metadata_bytes: 4096,
        max_retained_data_bytes: 65536,
        max_construction_peak_bytes: 65536,
    }
}
fn source(stereo: bool) -> crate::OwnedAudioSource {
    if stereo {
        crate::OwnedAudioSource::try_resident(24000, vec![[0.25, -0.5]; 4])
            .ok()
            .unwrap()
    } else {
        crate::OwnedAudioSource::try_resident_mono(24000, vec![0.25; 4])
            .ok()
            .unwrap()
    }
}
fn supported() -> bool {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported affine original install");
        false
    } else {
        true
    }
}
#[test]
fn affine_original_install_validates_before_producer_retains_once_and_cleans_on_control() {
    if !supported() {
        return;
    }
    let asset = crate::native_device_resources::tests::ids()[0];
    for stereo in [false, true] {
        eprintln!("SUPPORTED affine original install stereo={stereo}");
        let original = source(stereo);
        let entries = vec![OriginalResourceInput::native(asset, &original)];
        let plan = OriginalResourceBankPlan::borrow(&entries);
        let signal = query(false, false);
        let mut cell = AudioCandidatePreparation::try_new_on_control(0, outer()).unwrap();
        let mut too_large = allowance();
        too_large.max_retained_data_bytes = outer().max_prepared_data_bytes;
        assert_eq!(
            cell.operations_on_control()
                .install_original_resources_on_control(&plan, too_large, &signal),
            Err(ResourcePreparationError::Storage(
                AudioStorageError::ReservationExceeded
            ))
        );
        assert_eq!(signal.calls.load(Ordering::Relaxed), 0);
        assert!(!cell.original_resource_attempted);
        cell.operations_on_control()
            .install_original_resources_on_control(&plan, allowance(), &signal)
            .unwrap();
        let calls = signal.calls.load(Ordering::Relaxed);
        assert!(calls > 0);
        let project = hex_project::Project::new(48000).unwrap();
        assert_eq!(
            cell.prepare_on_control(
                &project,
                1,
                crate::AudioSourceContext {
                    project_generation: 1,
                    editor_revision: 1
                }
            ),
            Err(AudioStorageError::SourceMismatch)
        );
        assert!(cell.bank.is_none() && cell.token.is_none() && cell.owner.is_none());
        assert!(cell.original_resource_attempted && !cell.original_resource_refused);
        assert_eq!(
            cell.custody
                .as_ref()
                .unwrap()
                .capacity_report()
                .original_resource_retained_allowance,
            allowance().max_retained_data_bytes
        );
        assert_eq!(
            cell.operations_on_control()
                .install_original_resources_on_control(&plan, allowance(), &signal),
            Err(ResourcePreparationError::Storage(
                AudioStorageError::SourceMismatch
            ))
        );
        assert_eq!(signal.calls.load(Ordering::Relaxed), calls);
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
            crate::AudioStorageLedgerReservation {
                data_rows: 128,
                handle_rows: 64,
                root_rows: 16,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            },
        )
        .unwrap();
        ledger
            .borrow_custody_on_control(
                cell.custody.as_ref().unwrap(),
                crate::AudioStorageRole::Current,
            )
            .unwrap();
        assert!(ledger.validate_closed_shared_handles().is_err());
        ledger
            .borrow_native_source_on_control(&original, crate::AudioStorageRole::Caller)
            .unwrap();
        ledger.validate_closed_shared_handles().unwrap();
        assert!(ledger.summary().unwrap().data_bytes > 0);
        drop(ledger);
        cell.cleanup_on_control().unwrap();
        assert!(cell.complete && cell.custody.is_none());
        if stereo {
            assert_eq!(
                original.view().resident_frames().unwrap(),
                &[[0.25, -0.5]; 4]
            );
        } else {
            assert_eq!(original.view().resident_mono_samples().unwrap(), &[0.25; 4]);
        }
    }
}
#[test]
fn affine_original_install_reported_cancel_prevents_second_producer_and_allows_cleanup() {
    if !supported() {
        return;
    }
    eprintln!("SUPPORTED affine original install cancellation");
    let asset = crate::native_device_resources::tests::ids()[0];
    let original = source(false);
    let entries = vec![OriginalResourceInput::native(asset, &original)];
    let plan = OriginalResourceBankPlan::borrow(&entries);
    let signal = query(true, false);
    let mut cell = AudioCandidatePreparation::try_new_on_control(0, outer()).unwrap();
    assert_eq!(
        cell.operations_on_control()
            .install_original_resources_on_control(&plan, allowance(), &signal),
        Err(ResourcePreparationError::Cancelled)
    );
    assert!(cell.original_resource_refused && cell.original_resource_attempted);
    assert!(!cell.failed_incomplete());
    let calls = signal.calls.load(Ordering::Relaxed);
    assert_eq!(
        cell.operations_on_control()
            .install_original_resources_on_control(&plan, allowance(), &signal),
        Err(ResourcePreparationError::Storage(
            AudioStorageError::SourceMismatch
        ))
    );
    assert_eq!(signal.calls.load(Ordering::Relaxed), calls);
    let project = hex_project::Project::new(48000).unwrap();
    assert_eq!(
        cell.prepare_on_control(
            &project,
            1,
            crate::AudioSourceContext {
                project_generation: 1,
                editor_revision: 1
            }
        ),
        Err(AudioStorageError::SourceMismatch)
    );
    cell.cleanup_on_control().unwrap();
    assert!(cell.complete);
    assert_eq!(original.view().resident_mono_samples().unwrap(), &[0.25; 4]);
}
#[test]
fn affine_original_install_query_unwind_latches_actual_cell_and_refuses_retry_cleanup() {
    if !supported() {
        return;
    }
    eprintln!("SUPPORTED affine original install caught producer unwind");
    let asset = crate::native_device_resources::tests::ids()[0];
    let original = source(false);
    let entries = vec![OriginalResourceInput::native(asset, &original)];
    let plan = OriginalResourceBankPlan::borrow(&entries);
    let signal = query(false, true);
    let mut cell = AudioCandidatePreparation::try_new_on_control(0, outer()).unwrap();
    assert_eq!(
        cell.operations_on_control()
            .install_original_resources_on_control(&plan, allowance(), &signal),
        Err(ResourcePreparationError::Storage(
            AudioStorageError::UnknownStorage
        ))
    );
    assert!(cell.failed_incomplete() && cell.first_panic.is_some());
    assert!(cell.custody.is_some());
    assert_eq!(
        cell.custody
            .as_ref()
            .unwrap()
            .capacity_report()
            .original_resource_construction_allowance,
        allowance().max_construction_peak_bytes
    );
    let calls = signal.calls.load(Ordering::Relaxed);
    assert_eq!(
        cell.operations_on_control()
            .install_original_resources_on_control(&plan, allowance(), &signal),
        Err(ResourcePreparationError::Storage(
            AudioStorageError::SourceMismatch
        ))
    );
    assert_eq!(signal.calls.load(Ordering::Relaxed), calls);
    assert_eq!(
        cell.cleanup_on_control(),
        Err(AudioStorageError::UnknownStorage)
    );
    assert!(!cell.complete && cell.custody.is_some());
    assert_eq!(original.view().resident_mono_samples().unwrap(), &[0.25; 4]);
}
