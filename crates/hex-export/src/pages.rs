//! Borrowed control-worker page preparation; no worker, catalog or native custody.
use crate::{ExportControl, ExportError};
use hex_engine::{BorrowedMediaPage, MediaPages, SessionPlayback, StreamSourceKey};

/// An offline provider owned by the caller for this entire synchronous job.
/// Preparation/finalization may wait or perform I/O only on the export control
/// worker. The shared renderer never retries a page miss after advancing DSP.
/// Implementations must bound queues/pools/work, independently verify expected
/// file identities and retain pins through the complete chunk render.
pub trait PreparedExportPages: MediaPages {
    /// Prepare every exact next-chunk extent before any runtime mutation. Refuse
    /// unavailable, short or wrong-generation pages; native readiness receipts
    /// are not authority for this independent offline job.
    fn prepare_chunk(
        &mut self,
        playback: &SessionPlayback,
        frames: usize,
        control: &ExportControl,
    ) -> Result<(), ExportError>;

    /// Validate the actual read and release this chunk's pins. Any unexpected
    /// miss must fail the job, never publish source-local fallback silence.
    fn complete_chunk(&mut self) -> Result<(), ExportError>;

    /// Finish source verification and joining/control cleanup before publication.
    /// Failure/cancellation leaves only task-owned staging for ordinary cleanup.
    fn finish(&mut self, control: &ExportControl) -> Result<(), ExportError>;
}

// Keep the engine's existing Sized page-provider API. This reference adds no
// cache, scheduler, source owner or readiness authority.
pub(crate) struct PageView<'a>(pub(crate) &'a mut dyn PreparedExportPages);
impl MediaPages for PageView<'_> {
    fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
        self.0.page(key, index)
    }
    fn request_page(&mut self, key: StreamSourceKey, index: u64) {
        self.0.request_page(key, index);
    }
}
