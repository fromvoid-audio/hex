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
pub(super) struct Receipt {
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
pub(super) enum Disposition {
    Waiting,
    Terminal,
    Invalidated,
}

impl Receipt {
    pub(super) fn new(generation: u64) -> Self {
        Self {
            generation,
            state: State::Queued,
        }
    }

    pub(super) fn is_queued(&self) -> bool {
        matches!(self.state, State::Queued)
    }

    /// Returns false for an invalid or duplicate stamp; never rebinds custody.
    pub(super) fn submitted(&mut self, pending: usize) -> bool {
        let Some(pending) = NonZeroUsize::new(pending) else {
            return false;
        };
        if !self.is_queued() {
            return false;
        }
        self.state = State::Submitted(pending);
        true
    }

    pub(super) fn observe(
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
