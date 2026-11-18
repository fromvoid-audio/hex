//! One immutable publication identity and one shared atomic decision.
//! Control may cancel Pending. Only the callback can admit or install.
//! Graph retirement on the callback or final destruction on control seals
//! an unfinished receipt. Neither path can overwrite a terminal.
use super::DeviceInsertBasis;
use hex_engine::{AudioSourceContext, PlaybackHandoffRejection};
use std::sync::{
    Arc,
    atomic::{
        AtomicU32, AtomicU64,
        Ordering::{AcqRel, Acquire, Release},
    },
};
use std::time::Instant;

const PENDING: u32 = 0;
const APPLYING: u32 = 1;
const CANCEL_REQUESTED: u32 = 2;
const INSTALLED: u32 = 3;
const CANCELLED: u32 = 4;
const OUTPUT_CLOSED: u32 = 5;
const EXPIRED: u32 = 6;
const FADING: u32 = 7;
const REFUSED: u32 = 16;

/// Control-side result of cancelling a conditionally published fresh runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshRuntimeCancellationResult {
    /// Cancellation won before the callback began installing the candidate.
    Cancelled,
    /// The callback began a confirmed fade or crossed the installation decision.
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
    Expired,
    Refused(PlaybackHandoffRejection),
    OutputClosed,
}

/// Control-side receipt for one cancellable graph. It owns no graph,
/// endpoints or output. Installation is permanent historical evidence.
#[derive(Clone, Debug)]
pub struct CancellablePublicationReceipt(Arc<Decision>);

#[derive(Debug)]
struct Decision {
    output_identity: u64,
    context: Option<AudioSourceContext>,
    revision: u64,
    device_insert: Option<DeviceInsertBasis>,
    // The callback may advance the original route witness through already
    // admitted render actions; no command is accepted under the new stamp.
    retained_route_generation: AtomicU64,
    phase: AtomicU32,
}

impl CancellablePublicationReceipt {
    pub(crate) const fn payload_bytes() -> usize {
        std::mem::size_of::<Decision>()
    }

    pub(super) fn new(
        output_identity: u64,
        context: Option<AudioSourceContext>,
        revision: u64,
        device_insert: Option<DeviceInsertBasis>,
        route_generation: u64,
    ) -> Self {
        Self(Arc::new(Decision {
            output_identity,
            context,
            revision,
            device_insert,
            retained_route_generation: AtomicU64::new(route_generation),
            phase: AtomicU32::new(PENDING),
        }))
    }

    pub(super) fn same_decision(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(super) fn device_insert_basis(&self) -> Option<&DeviceInsertBasis> {
        self.0.device_insert.as_ref()
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

    pub(super) fn retained_route_generation(&self) -> u64 {
        self.0.retained_route_generation.load(Acquire)
    }

    pub fn state(&self) -> PublicationReceiptState {
        use PublicationReceiptState::*;
        self.expire_if_due();
        match self.0.phase.load(Acquire) {
            PENDING => Pending,
            FADING | APPLYING => Applying,
            INSTALLED => Installed,
            CANCEL_REQUESTED | CANCELLED => Cancelled,
            EXPIRED => Expired,
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
        self.expire_if_due();
        match self
            .0
            .phase
            .compare_exchange(PENDING, CANCEL_REQUESTED, AcqRel, Acquire)
        {
            Ok(_) | Err(CANCEL_REQUESTED) => Cancelled,
            Err(FADING | APPLYING) => Applying,
            Err(_) => Complete,
        }
    }

    /// Seal an unapplied device insert after a witnessed fault of its output.
    /// An admitted callback still owns its exact terminal.
    pub(super) fn close_device_insert_on_fault(&self) {
        if self.0.device_insert.is_some() {
            self.fence_unapplied_insert(OUTPUT_CLOSED);
        }
    }

    fn expire_if_due(&self) {
        // The control owner initializes the monotonic clock when creating the
        // deadline. Only a device insert consults it on the callback.
        if self
            .0
            .device_insert
            .as_ref()
            .is_some_and(|basis| Instant::now() >= basis.install_deadline)
        {
            self.fence_unapplied_insert(EXPIRED);
        }
    }

    fn fence_unapplied_insert(&self, terminal: u32) {
        let _ = self.0.phase.fetch_update(AcqRel, Acquire, |phase| {
            matches!(phase, PENDING | FADING).then_some(terminal)
        });
    }
}

/// The queued graph retains the writer. Control destruction closes its
/// unfinished receipt even if no further callback can run.
pub(crate) struct PublicationReceiptOwner(pub(super) CancellablePublicationReceipt);

impl PublicationReceiptOwner {
    pub(super) fn device_insert_deadline(&self) -> Option<Instant> {
        self.0
            .device_insert_basis()
            .map(|basis| basis.install_deadline)
    }

    pub(super) fn record_retained_route_generation(&self, generation: u64) {
        self.0
            .0
            .retained_route_generation
            .store(generation, Release);
    }

    pub(super) fn fenced_before_admission(&self) -> bool {
        self.0.expire_if_due();
        matches!(
            self.phase().load(Acquire),
            CANCEL_REQUESTED | EXPIRED | OUTPUT_CLOSED
        )
    }

    pub(crate) fn begin_apply(&self) -> bool {
        self.0.expire_if_due();
        self.phase()
            .compare_exchange(PENDING, APPLYING, AcqRel, Acquire)
            .is_ok()
    }

    pub(super) fn begin_confirmed_fade(&self) -> bool {
        self.0.expire_if_due();
        self.phase()
            .compare_exchange(PENDING, FADING, AcqRel, Acquire)
            .is_ok()
    }

    pub(crate) fn begin_confirmed_apply(&self) -> bool {
        self.0.expire_if_due();
        self.phase()
            .compare_exchange(FADING, APPLYING, AcqRel, Acquire)
            .is_ok()
    }

    pub(crate) fn installed(&self) {
        let _ = self
            .phase()
            .compare_exchange(APPLYING, INSTALLED, AcqRel, Acquire);
    }

    /// Returns whether cancellation won, including against retirement.
    pub(crate) fn returned(&self, rejection: Option<PlaybackHandoffRejection>) -> bool {
        self.finish(rejection.map_or(OUTPUT_CLOSED, |reason| REFUSED + reason as u32))
    }

    /// A pure handoff refusal left both graphs untouched. Make a deferred
    /// seamless insert cancellable again before the next callback attempt.
    pub(crate) fn defer_apply(&self) {
        let _ = self
            .phase()
            .compare_exchange(APPLYING, PENDING, AcqRel, Acquire);
    }

    fn phase(&self) -> &AtomicU32 {
        &self.0.0.phase
    }

    fn finish(&self, terminal: u32) -> bool {
        // Cancellation or expiry can win Pending only once.
        matches!(
            self.phase()
                .fetch_update(AcqRel, Acquire, |phase| match phase {
                    PENDING | FADING | APPLYING => Some(terminal),
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
