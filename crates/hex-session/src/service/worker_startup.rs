//! One startup aggregate, so spawn failure cannot drop completion before its
//! actual Session/Event children. No new queue, flag or readiness state.
use super::*;
use crate::snapshot_exports::{SnapshotWorkerControl, SnapshotWorkerStop};
pub(super) enum ServiceStop {
    Unqualified(Arc<AtomicBool>),
    Registered(SnapshotWorkerStop),
}
impl ServiceStop {
    pub(super) fn request_cancel(&self) {
        match self {
            Self::Unqualified(flag) => flag.store(true, Ordering::Release),
            Self::Registered(stop) => {
                let _ = stop.request_cancel();
            }
        }
    }
}
pub(super) struct WorkerStartup {
    session: Option<Session>,
    events: Option<WorkerEvents>,
    pending_cancellation: Option<SessionCancellation>,
    control: Option<SnapshotWorkerControl>,
}
impl WorkerStartup {
    pub(super) fn new(
        session: Option<Session>,
        control: Option<SnapshotWorkerControl>,
        cancellation: SessionCancellation,
    ) -> Self {
        Self {
            session,
            events: None,
            pending_cancellation: Some(cancellation),
            control,
        }
    }
    pub(super) fn install_events(&mut self, events: WorkerEvents) {
        self.events = Some(events);
    }
    pub(super) fn take_cancellation(&mut self) -> Option<SessionCancellation> {
        self.pending_cancellation.take()
    }
    pub(super) fn take_events(&mut self) -> Option<WorkerEvents> {
        self.events.take()
    }
    pub(super) fn take_session(&mut self) -> Option<Session> {
        self.session.take()
    }
    pub(super) fn control(&self) -> Option<&SnapshotWorkerControl> {
        self.control.as_ref()
    }
}
impl Drop for WorkerStartup {
    fn drop(&mut self) {
        drop(self.session.take());
        // Native close precedes completion, independently of retained UI reads.
        drop(self.events.take());
        drop(self.pending_cancellation.take());
        drop(self.control.take());
    }
}
