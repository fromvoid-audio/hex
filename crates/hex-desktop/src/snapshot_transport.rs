//! Concrete static Session adapter over the existing lifecycle family table.
//! Routing only, not joint storage/admission readiness.
use hex_audio::{RetainedFamilyError, RetainedFamilyId};
use hex_engine::RetainedControlJobRoute;
use hex_session::snapshot_exports::{
    ExportRecordError, SessionExportFamily, SnapshotReadCapability, SnapshotReadContinuation,
    SnapshotReadOperations, SnapshotReadStamp, SnapshotTransportError as Error,
};
use std::{
    io::Write,
    sync::atomic::{AtomicUsize, Ordering},
};
static RELEASE_FAILURES: AtomicUsize = AtomicUsize::new(0);
static OPERATIONS: SnapshotReadOperations =
    SnapshotReadOperations::new_unqualified(validate, read, derive, release, report);
fn family_error(error: RetainedFamilyError) -> Error {
    match error {
        RetainedFamilyError::Reservation
        | RetainedFamilyError::Allocation
        | RetainedFamilyError::Exhausted => Error::Reservation,
        RetainedFamilyError::Busy => Error::Busy,
        RetainedFamilyError::Stale => Error::Stale,
        RetainedFamilyError::Failed => Error::Failed,
    }
}
fn record_error(error: ExportRecordError) -> Error {
    match error {
        ExportRecordError::Reservation
        | ExportRecordError::Allocation
        | ExportRecordError::Exhausted => Error::Reservation,
        ExportRecordError::Busy => Error::Busy,
        ExportRecordError::Stale | ExportRecordError::Closed => Error::Stale,
    }
}
fn validate(route: RetainedControlJobRoute, stamp: &SnapshotReadStamp) -> Result<(), Error> {
    hex_audio::read_retained_family_route::<SessionExportFamily, _>(route, |family| {
        family.read_with(stamp, |_| ()).map_err(record_error)
    })
    .map_err(family_error)?
}
fn read(
    route: RetainedControlJobRoute,
    stamp: &SnapshotReadStamp,
    continuation: &mut dyn SnapshotReadContinuation,
) -> Result<(), Error> {
    hex_audio::read_retained_family_route::<SessionExportFamily, _>(route, |family| {
        family
            .read_with(stamp, |snapshot| continuation.read(snapshot))
            .map_err(record_error)
    })
    .map_err(family_error)?
}
fn derive(
    route: RetainedControlJobRoute,
    stamp: &SnapshotReadStamp,
) -> Result<SnapshotReadStamp, Error> {
    hex_audio::read_retained_family_route::<SessionExportFamily, _>(route, |family| {
        family.derive_read(stamp).map_err(record_error)
    })
    .map_err(family_error)?
}
fn release(route: RetainedControlJobRoute, stamp: SnapshotReadStamp) -> Result<(), Error> {
    hex_audio::acknowledge_retained_family_route::<SessionExportFamily>(route, |family| {
        family
            .release_read(stamp)
            .map_err(|_| RetainedFamilyError::Failed)
    })
    .map_err(family_error)
}
fn report(route: RetainedControlJobRoute, error: Error) {
    // Actual failed/read obligations stay in existing custody. No owning retry
    // queue, successful ACK, budget release or replacement mutation is implied.
    let _ = RELEASE_FAILURES.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        Some(count.saturating_add(1))
    });
    let _ = writeln!(
        std::io::stderr().lock(),
        "snapshot release failed: {route:?}: {error:?}"
    );
}
/// Validate the actual registered typed holder and reserved read before escape.
/// Refusal preserves its caller's unexported pending cell.
pub fn bind_registered_read_on_control(
    family: RetainedFamilyId<SessionExportFamily>,
    pending: &mut Option<SnapshotReadStamp>,
) -> Result<SnapshotReadCapability, Error> {
    let route = hex_audio::retained_family_route_on_control(family).map_err(family_error)?;
    SnapshotReadCapability::try_bind_on_control(route, pending, &OPERATIONS)
}
/// Worker-only owned-view staging, before Event insertion. The family must be
/// registered and all old/new/input storage quoted by the actual control owner.
/// No raw Arc adoption or owning UI field is introduced. Insertion/read-slot
/// refusal preserves the view. A later binding refusal keeps the exact stamp
/// in the caller's bounded pending cell for retry or durable cancellation.
/// Neither this component nor a successful binding proves graph admission.
pub fn publish_pending_snapshot_on_control(
    family: RetainedFamilyId<SessionExportFamily>,
    producer: &hex_session::snapshot_exports::SnapshotProducerKey,
    pending_view: &mut Option<hex_session::service::ViewState>,
    pending_read: &mut Option<SnapshotReadStamp>,
) -> Result<SnapshotReadCapability, Error> {
    if pending_read.is_some() {
        if pending_view.is_some() {
            return Err(Error::Protocol);
        }
    } else {
        if pending_view.is_none() {
            return Err(Error::Protocol);
        }
        let read = hex_audio::read_retained_family(family, |family| {
            family.insert_event_read_on_control(producer, pending_view)
        })
        .map_err(family_error)?
        .map_err(record_error)?;
        *pending_read = Some(read);
    }
    // Release the actual writer/outer inspection from insertion BEFORE the
    // binder takes shared guards. Never ACK/drop under that same-thread writer.
    bind_registered_read_on_control(family, pending_read)
}
/// Cancel a binding refusal. Failure before actual dispatch preserves pending.
pub fn release_pending_on_control(
    family: RetainedFamilyId<SessionExportFamily>,
    pending: &mut Option<SnapshotReadStamp>,
) -> Result<(), Error> {
    hex_audio::acknowledge_retained_family(family, |family| {
        let stamp = pending.take().ok_or(RetainedFamilyError::Stale)?;
        family
            .release_read(stamp)
            .map_err(|_| RetainedFamilyError::Failed)
    })
    .map_err(family_error)
}
/// Nonowning diagnostic count, never storage bytes or successful ACK count.
pub fn failed_release_count() -> usize {
    RELEASE_FAILURES.load(Ordering::Relaxed)
}

/// Register the concrete Session cleanup adapter in an already reserved slot.
/// Body and all nested/old/new construction charges remain separate obligations.
pub fn register_family_on_control(
    slot: &hex_audio::RetainedFamilySlot,
    pending: &mut Option<SessionExportFamily>,
    max_job_body_bytes: usize,
) -> Result<RetainedFamilyId<SessionExportFamily>, RetainedFamilyError> {
    fn cleanup(
        family: &mut SessionExportFamily,
    ) -> Result<hex_audio::RetainedFamilyProgress, RetainedFamilyError> {
        family
            .cleanup_one_on_control()
            .map(|progress| match progress {
                hex_session::snapshot_exports::SnapshotFamilyCleanup::Pending => {
                    hex_audio::RetainedFamilyProgress::Pending
                }
                hex_session::snapshot_exports::SnapshotFamilyCleanup::Disposed => {
                    hex_audio::RetainedFamilyProgress::Disposed
                }
            })
            .map_err(|_| RetainedFamilyError::Failed)
    }
    hex_audio::register_retained_family_on_control(slot, pending, max_job_body_bytes, cleanup)
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "allocation-probe"))]
mod storage_tests;

mod inventory;
#[cfg(all(test, feature = "allocation-probe"))]
mod publication_tests;
pub use inventory::{
    SnapshotFamilyInventory, SnapshotFamilyInventoryError,
    with_snapshot_family_inventory_on_control,
};

mod worker_control;
mod worker_startup;
pub use worker_control::bind_registered_worker_on_control;
pub(super) use worker_startup::{
    configure_native_table_on_control, worker_from_existing_table_on_control,
};
