//! Control-owned terminal observations, never inferred missing inventories.
use crate::{PagePoolReport, StreamRuntimeError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerJoinObservation {
    Joined,
    Panicked,
    MissingHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReportedShutdown {
    pub page_buffers: usize,
    pub page_bytes: usize,
    /// Transfer to the return path, not an additional reclaimed-buffer count.
    pub cached_pages_returned: usize,
    pub queued_pages_reclaimed: usize,
    pub held_return_pages_reclaimed: usize,
    pub worker_pages_reclaimed: Option<usize>,
    pub join: WorkerJoinObservation,
    pub conservation: Option<bool>,
    pub failure: Option<StreamRuntimeError>,
}
impl ReportedShutdown {
    pub fn successful_report(self) -> Option<PagePoolReport> {
        if self.failure.is_some()
            || self.join != WorkerJoinObservation::Joined
            || self.conservation != Some(true)
        {
            return None;
        }
        Some(PagePoolReport {
            page_buffers: self.page_buffers,
            page_bytes: self.page_bytes,
            cached_pages_returned: self.cached_pages_returned,
            queued_pages_reclaimed: self.queued_pages_reclaimed,
            held_return_pages_reclaimed: self.held_return_pages_reclaimed,
            worker_pages_reclaimed: self.worker_pages_reclaimed?,
            worker_joined: true,
            accounting_complete: true,
        })
    }
}
