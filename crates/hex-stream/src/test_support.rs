//! Optional control-side fixture for holding one real worker page request.
//! This is not a readiness authority and is absent unless explicitly enabled.
use crate::PageKey;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
use std::time::Duration;

/// A one-shot worker-side hold. The callback never observes or waits on it.
pub struct PageGate {
    pub(crate) key: PageKey,
    pub(crate) read_failure_after: Option<u32>,
    entered: SyncSender<()>,
    release: Receiver<()>,
}

impl PageGate {
    /// Create the worker token and bounded control-side entered/release endpoints.
    pub fn new(key: PageKey) -> (Self, Receiver<()>, SyncSender<()>) {
        let (entered, observed) = mpsc::sync_channel(1);
        let (resume, release) = mpsc::sync_channel(1);
        (
            Self {
                key,
                read_failure_after: None,
                entered,
                release,
            },
            observed,
            resume,
        )
    }

    /// After release, fail one verified raw-page read following this many
    /// successful raw reads in the selected request. The existing reader owns
    /// error conversion and full-output clearing; admission and file bytes are
    /// unchanged. The gate and failure plan are consumed once, so retry works.
    pub fn with_read_failure_after(mut self, successful_reads: u32) -> Self {
        self.read_failure_after = Some(successful_reads);
        self
    }

    pub(crate) fn wait(self, stop: &AtomicBool) -> bool {
        if self.entered.try_send(()).is_err() {
            return true;
        }
        while !stop.load(Ordering::Acquire) {
            match self.release.recv_timeout(Duration::from_millis(2)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return true,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        false
    }
}
