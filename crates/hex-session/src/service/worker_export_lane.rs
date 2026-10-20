//! Actual constructor-selected export lane. Native File jobs remain in the
//! existing worker; this is not a second work queue or an admission certificate.
use super::*;

pub(super) enum WorkerExportSender {
    Unqualified(SyncSender<ExportWork>),
    NativeInline,
}
impl From<SyncSender<ExportWork>> for WorkerExportSender {
    fn from(sender: SyncSender<ExportWork>) -> Self {
        Self::Unqualified(sender)
    }
}
impl WorkerExportSender {
    /// Return the SAME unsubmitted owner; callers already handle full and
    /// disconnected identically. No error Box or additional allocation.
    pub(super) fn try_send(&self, work: ExportWork) -> Option<ExportWork> {
        match self {
            Self::Unqualified(sender) => match sender.try_send(work) {
                Ok(()) => None,
                Err(TrySendError::Full(work) | TrySendError::Disconnected(work)) => Some(work),
            },
            // Never adopt the already-born legacy prepared Arc into a Native
            // File destination. The actual Main constructor entry stays gated.
            Self::NativeInline => Some(work),
        }
    }
}

pub(super) struct NativeMainFiles {
    slots: Vec<MainControlFileJob>,
}
impl NativeMainFiles {
    pub(super) fn new() -> Self {
        // No reserve/owner birth before the closed full-union prefix. This
        // actual header belongs to WorkerEvents through startup and dispatch.
        Self { slots: Vec::new() }
    }
    pub(super) fn fields(&self) -> impl hex_engine::sampler_host_fields::FileSlotsFields<'_> {
        MainControlFileJob::reserved_slots_fields_on_control(&self.slots)
    }
}
