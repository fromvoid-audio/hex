//! Display handoff for ordinary edits released by a pointer interaction.
//!
//! Mouse-up ends input capture, not display custody. Keep the owner's immutable
//! final preview until its own terminal and canonical state are received in the
//! same synchronous `EditorView::receive`; do not use a timer, an arbitrary
//! revision change or an idle queue as a completion witness. Repaint and hit/
//! overwrite geometry must use that same preview while it is pending.
//!
//! Owners retain their typed payload, exact request matcher and source identity.
//! Create a receipt before enqueueing; stamp it only AFTER successful Service
//! submission (including retries/barriers), using the resulting pending count.
//! Full queues keep the queued receipt; admission refusal, closed service and
//! source replacement retire it. Selection/blur/Escape can cancel held input,
//! but cannot undo a released edit already admitted to the worker.
//!
//! This follows the Service's ordered ordinary request completions. Every
//! `request_complete` counts, including typed gesture completions counted by the
//! controller. Existing keyed device GestureClient acknowledgements remain their
//! own stronger protocol; never replace those with an ordinary FIFO receipt.
use std::num::NonZeroUsize;

#[derive(Debug)]
pub struct Receipt {
    generation: u64,
    state: State,
}

#[derive(Debug)]
enum State {
    Queued,
    Submitted(NonZeroUsize),
    Finished,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Disposition {
    Waiting,
    Terminal,
    Invalidated,
}

impl Receipt {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            state: State::Queued,
        }
    }

    pub fn is_queued(&self) -> bool {
        matches!(self.state, State::Queued)
    }

    /// Returns false for an invalid or duplicate stamp; never rebinds custody.
    pub fn submitted(&mut self, pending: usize) -> bool {
        let Some(pending) = NonZeroUsize::new(pending) else {
            return false;
        };
        if !self.is_queued() {
            return false;
        }
        self.state = State::Submitted(pending);
        true
    }

    pub fn observe(
        &mut self,
        generation: u64,
        project_replaced: bool,
        request_complete: bool,
    ) -> Disposition {
        if matches!(self.state, State::Finished) {
            return Disposition::Waiting;
        }
        if project_replaced || generation != self.generation {
            self.state = State::Finished;
            return Disposition::Invalidated;
        }
        if request_complete && let State::Submitted(remaining) = self.state {
            if let Some(next) = NonZeroUsize::new(remaining.get() - 1) {
                self.state = State::Submitted(next);
            } else {
                self.state = State::Finished;
                return Disposition::Terminal;
            }
        }
        Disposition::Waiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_and_earlier_events_cannot_retire_released_display() {
        let mut receipt = Receipt::new(7);
        assert_eq!(receipt.observe(7, false, true), Disposition::Waiting);
        assert!(!receipt.submitted(0));
        assert!(receipt.submitted(2));
        assert!(!receipt.submitted(1));
        assert_eq!(receipt.observe(7, false, false), Disposition::Waiting);
        assert_eq!(receipt.observe(7, false, true), Disposition::Waiting);
        // Success/refusal and revision equality are deliberately not inputs:
        // the same exact terminal releases paint for either canonical outcome.
        assert_eq!(receipt.observe(7, false, true), Disposition::Terminal);
        assert_eq!(receipt.observe(7, false, true), Disposition::Waiting);
        assert!(!receipt.submitted(1));
    }

    #[test]
    fn project_replacement_invalidates_both_queued_and_submitted_receipts() {
        for submitted in [false, true] {
            for replaced in [false, true] {
                let mut receipt = Receipt::new(7);
                if submitted {
                    assert!(receipt.submitted(3));
                }
                assert_eq!(
                    receipt.observe(if replaced { 7 } else { 8 }, replaced, false),
                    Disposition::Invalidated
                );
                assert!(!receipt.submitted(1));
            }
        }
    }
}

#[cfg(test)]
mod macro_gesture_tests {
    use super::*;

    #[test]
    fn macro_gesture_receipt_holds_final_preview_across_repaints_until_exact_ack_or_invalidation() {
        // Released macro knob drag submits a typed SetMacroValue edit at generation 7
        // while 2 earlier edits are still in flight (pending count = 3).
        let mut receipt = Receipt::new(7);
        assert!(receipt.is_queued());
        assert!(receipt.submitted(3));
        assert!(
            !receipt.submitted(1),
            "duplicate submission stamp must be refused"
        );

        // Intermediate repaints before ACK (no completion yet) keep frozen preview active.
        assert_eq!(receipt.observe(7, false, false), Disposition::Waiting);
        // Earlier requests 1 & 2 complete; macro edit still awaits its own exact ACK.
        assert_eq!(receipt.observe(7, false, true), Disposition::Waiting);
        assert_eq!(receipt.observe(7, false, true), Disposition::Waiting);
        // Exact terminal ACK arrives alongside canonical state -> retire preview atomically!
        assert_eq!(receipt.observe(7, false, true), Disposition::Terminal);
        // Post-terminal observations remain settled without re-firing.
        assert_eq!(receipt.observe(7, false, true), Disposition::Waiting);

        // Source/project replacement invalidates a pending macro preview immediately.
        let mut replaced_receipt = Receipt::new(7);
        assert!(replaced_receipt.submitted(2));
        assert_eq!(
            replaced_receipt.observe(7, true, false),
            Disposition::Invalidated
        );

        // Stale generation (refusal/reload) invalidates without mutating canonical state.
        let mut stale_receipt = Receipt::new(7);
        assert!(stale_receipt.submitted(1));
        assert_eq!(
            stale_receipt.observe(8, false, false),
            Disposition::Invalidated
        );
    }
}

#[cfg(test)]
mod macro_variation_gesture_tests {
    use super::*;

    #[test]
    fn macro_variation_recall_receipt_handles_delayed_ack_cancellation_and_project_replacement() {
        // Triggering one atomic variation recall enqueues a single receipt at generation 12.
        let mut recall = Receipt::new(12);
        assert!(recall.is_queued());
        // Before worker admission, a project replacement cancels custody immediately.
        let mut cancelled_before_submit = Receipt::new(12);
        assert_eq!(
            cancelled_before_submit.observe(12, true, false),
            Disposition::Invalidated
        );

        // Once admitted with 1 prior request ahead (pending = 2), repaints hold the
        // recalled variation preview steady until exact terminal ACK arrives.
        assert!(recall.submitted(2));
        assert_eq!(recall.observe(12, false, false), Disposition::Waiting);
        assert_eq!(recall.observe(12, false, true), Disposition::Waiting);
        assert_eq!(recall.observe(12, false, true), Disposition::Terminal);

        // Prepare refusal or reload bumps generation -> invalidates without leaving stale preview.
        let mut refused = Receipt::new(12);
        assert!(refused.submitted(1));
        assert_eq!(refused.observe(13, false, false), Disposition::Invalidated);
    }
}
