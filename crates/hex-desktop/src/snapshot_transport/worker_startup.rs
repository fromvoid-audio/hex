//! Actual desktop startup on the existing family table, before Service escape.
//! No new registry/queue/device or origin/admission certificate is created.
use super::*;
use hex_engine::RetainedControlCancellation;
use hex_session::snapshot_exports::{SnapshotProducerKey, SnapshotWorkerControl};
use std::time::{Duration, Instant};

pub(crate) fn configure_native_table_on_control() -> Result<(), Error> {
    // One backend is constructed at the sole production main() callsite. The
    // same Service/family survives New/Open/project replacement. Exhaustion or
    // prior incompatible configuration refuses; it never enlarges the table.
    let limits = hex_audio::RetainedFamilyReservation {
        family_slots: 1,
        max_table_bytes: 16_384,
    };
    hex_audio::retained_family_table_reservation_quote(limits).map_err(family_error)?;
    hex_audio::reserve_retained_families_on_control(limits).map_err(family_error)
}
fn retry_busy<T>(mut operation: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match operation() {
            Err(Error::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            result => return result,
        }
    }
}
// Only existing owners, kept together through fallible metadata/register/bind
// steps. If no Service escaped, there are NO Session/native/worker children to
// dispose before this constructor's real base-worker completion.
#[derive(Default)]
struct PendingStartup {
    slot: Option<hex_audio::RetainedFamilySlot>,
    family: Option<SessionExportFamily>,
    key: Option<SnapshotProducerKey>,
    registered: Option<RetainedFamilyId<SessionExportFamily>>,
    worker: Option<RetainedControlCancellation>,
    cancel: Option<hex_audio::RetainedFamilyCancellationEndpoint>,
}
impl Drop for PendingStartup {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel {
            let _ = cancel.cancel();
        }
        if let (Some(id), Some(key)) = (self.registered, self.key.as_ref()) {
            // Durable shared acknowledgement, outside every inner guard. Failed
            // cleanup remains in the existing persistent failed/charged slot.
            let _ = hex_audio::acknowledge_retained_family(id, |family| {
                family
                    .abandon_on_control(key)
                    .map_err(|_| RetainedFamilyError::Failed)
            });
            if self.worker.is_some() {
                let _ = retry_busy(|| {
                    hex_audio::finish_retained_family_worker_on_control(id, &mut self.worker)
                        .map_err(family_error)
                });
            }
        }
        if let Some(slot) = &self.slot {
            let _ = hex_audio::cancel_retained_family_slot_on_control(slot);
        }
    }
}
/// This exact constructor is shared by the real native backend and headless
/// fixture. Test callers supply an already initialized private table; production
/// configures its single application family before calling it.
pub(crate) fn worker_from_existing_table_on_control()
-> Result<(RetainedFamilyId<SessionExportFamily>, SnapshotWorkerControl), Error> {
    let mut pending = PendingStartup::default();
    let limits =
        SessionExportFamily::native_editor_reservation_on_control(65_536).map_err(record_error)?;
    let (family, key) =
        SessionExportFamily::try_new_native_on_control(limits).map_err(record_error)?;
    pending.family = Some(family);
    pending.key = Some(key);
    pending.slot = Some(retry_busy(|| {
        hex_audio::reserve_retained_family_slot_on_control().map_err(family_error)
    })?);
    let slot = pending.slot.as_ref().ok_or(Error::Protocol)?;
    let id = retry_busy(|| {
        register_family_on_control(
            slot,
            &mut pending.family,
            hex_audio::retained_family_job_body_quote::<SessionExportFamily>(),
        )
        .map_err(family_error)
    })?;
    pending.registered = Some(id);
    pending.slot = None; // actual typed job now owns the family, not the reservation
    let (worker, cancel) =
        retry_busy(|| hex_audio::bind_retained_family_worker_on_control(id).map_err(family_error))?;
    pending.worker = Some(worker);
    pending.cancel = Some(cancel);
    let control = retry_busy(|| {
        super::worker_control::bind_registered_worker_on_control(
            &mut pending.worker,
            &mut pending.key,
        )
    })?;
    pending.cancel = None;
    pending.registered = None;
    Ok((id, control))
}
