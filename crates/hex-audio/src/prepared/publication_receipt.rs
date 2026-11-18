//! One immutable publication identity and one shared atomic decision.
//! Control may cancel Pending. Only the callback can admit or install.
//! Graph retirement on the callback or final destruction on control seals
//! an unfinished receipt. Neither path can overwrite a terminal.
use hex_engine::{AudioSourceContext, PlaybackHandoffRejection};
use std::sync::{
    Arc,
    atomic::{
        AtomicU32,
        Ordering::{AcqRel, Acquire},
    },
};

const PENDING: u32 = 0;
const APPLYING: u32 = 1;
const CANCEL_REQUESTED: u32 = 2;
const INSTALLED: u32 = 3;
const CANCELLED: u32 = 4;
const OUTPUT_CLOSED: u32 = 5;
const REFUSED: u32 = 16;

/// Control-side result of cancelling a conditionally published fresh runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshRuntimeCancellationResult {
    /// Cancellation won before the callback began installing the candidate.
    Cancelled,
    /// The callback had already crossed its final installation decision.
    Applying,
    /// The callback has already returned or installed this candidate.
    Complete,
    /// The nonblocking Client view is contended; retain and retry the request.
    Busy,
    /// There is no currently retained cancellation decision for this revision.
    UnknownRevision,
}

/// The exact graph's disposition remains readable after newer submissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationReceiptState {
    Pending,
    Applying,
    Installed,
    Cancelled,
    Refused(PlaybackHandoffRejection),
    OutputClosed,
}

/// Control-side receipt for one cancellable graph. It owns no graph,
/// endpoints or output. Installation is permanent historical evidence.
#[derive(Clone)]
pub struct CancellablePublicationReceipt(Arc<Decision>);

struct Decision {
    output_identity: u64,
    context: Option<AudioSourceContext>,
    revision: u64,
    phase: AtomicU32,
}

impl CancellablePublicationReceipt {
    pub(super) fn new(
        output_identity: u64,
        context: Option<AudioSourceContext>,
        revision: u64,
    ) -> Self {
        Self(Arc::new(Decision {
            output_identity,
            context,
            revision,
            phase: AtomicU32::new(PENDING),
        }))
    }

    pub fn output_identity(&self) -> u64 {
        self.0.output_identity
    }

    pub fn context(&self) -> Option<AudioSourceContext> {
        self.0.context
    }

    pub fn revision(&self) -> u64 {
        self.0.revision
    }

    pub fn state(&self) -> PublicationReceiptState {
        use PublicationReceiptState::*;
        match self.0.phase.load(Acquire) {
            PENDING => Pending,
            APPLYING => Applying,
            INSTALLED => Installed,
            CANCEL_REQUESTED | CANCELLED => Cancelled,
            code => code
                .checked_sub(REFUSED)
                .and_then(PlaybackHandoffRejection::from_code)
                .map_or(OutputClosed, Refused),
        }
    }

    /// A successful cancel prevents installation even before the callback
    /// returns the graph to control.
    pub fn cancel(&self) -> FreshRuntimeCancellationResult {
        use FreshRuntimeCancellationResult::*;
        match self
            .0
            .phase
            .compare_exchange(PENDING, CANCEL_REQUESTED, AcqRel, Acquire)
        {
            Ok(_) | Err(CANCEL_REQUESTED) => Cancelled,
            Err(APPLYING) => Applying,
            Err(_) => Complete,
        }
    }
}

/// The queued graph retains the writer. Control destruction closes its
/// unfinished receipt even if no further callback can run.
pub(super) struct PublicationReceiptOwner(pub(super) CancellablePublicationReceipt);

impl PublicationReceiptOwner {
    pub(super) fn cancel_requested(&self) -> bool {
        self.phase().load(Acquire) == CANCEL_REQUESTED
    }

    pub(super) fn begin_apply(&self) -> bool {
        self.phase()
            .compare_exchange(PENDING, APPLYING, AcqRel, Acquire)
            .is_ok()
    }

    pub(super) fn installed(&self) {
        let _ = self
            .phase()
            .compare_exchange(APPLYING, INSTALLED, AcqRel, Acquire);
    }

    /// Returns whether cancellation won, including against retirement.
    pub(super) fn returned(&self, rejection: Option<PlaybackHandoffRejection>) -> bool {
        self.finish(rejection.map_or(OUTPUT_CLOSED, |reason| REFUSED + reason as u32))
    }

    fn phase(&self) -> &AtomicU32 {
        &self.0.0.phase
    }

    fn finish(&self, terminal: u32) -> bool {
        // Cancel is the only competing writer and changes Pending at most once.
        matches!(
            self.phase()
                .fetch_update(AcqRel, Acquire, |phase| match phase {
                    PENDING | APPLYING => Some(terminal),
                    CANCEL_REQUESTED => Some(CANCELLED),
                    _ => None,
                }),
            Ok(CANCEL_REQUESTED) | Err(CANCELLED)
        )
    }
}

impl Drop for PublicationReceiptOwner {
    fn drop(&mut self) {
        self.returned(None);
    }
}
