//! Fixed control/callback action-outcome custody. Storage is prepared on control.
//!
//! This ledger reserves one slot per admitted ticket and publishes terminals in
//! callback completion order. It cannot free or overwrite an unobserved slot.
//! Queue admission and output lifecycle remain with the existing endpoint owners.
use hex_engine::PlaybackActionRejection;
use std::sync::{
    Arc,
    atomic::{
        AtomicBool, AtomicU8, AtomicU32, AtomicU64,
        Ordering::{Acquire, Relaxed, Release},
    },
};

pub const ACTION_OUTCOME_CAPACITY: usize = 32;
const CAPACITY: usize = ACTION_OUTCOME_CAPACITY;
const FREE: u8 = 0;
const RESERVED: u8 = 1;
const COMPLETE: u8 = 2;
const HEALTHY: u8 = 0;
const INVARIANT_FAILED: u8 = 1;
const RETIREMENT_UNAVAILABLE: u8 = 2;
const ENDPOINT_ABANDONED: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    InvariantFailed,
    RetirementUnavailable,
    EndpointAbandoned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The callback completed the action at its owning boundary.
    Applied,
    /// Priority Stop discarded an older action before it was applied.
    Superseded,
    /// The callback refused the action with this typed reason.
    Rejected(PlaybackActionRejection),
    /// The exact callback became quiescent without a terminal; application is unknown.
    Interrupted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub ticket: u64,
    pub outcome: Outcome,
}
struct Slot {
    state: AtomicU8,
    ticket: AtomicU64,
    sequence: AtomicU64,
    outcome: AtomicU32,
}
impl Slot {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(FREE),
            ticket: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            outcome: AtomicU32::new(0),
        }
    }
}
struct Ledger {
    slots: [Slot; CAPACITY],
    failure: AtomicU8,
    retired: AtomicBool,
}
impl Ledger {
    fn slot(&self, ticket: u64) -> &Slot {
        &self.slots[(ticket % CAPACITY as u64) as usize]
    }
    fn fail(&self, reason: u8) {
        let _ = self
            .failure
            .compare_exchange(HEALTHY, reason, Release, Relaxed);
    }
}
/// Unique command-admission side. Reserve before publishing a queue entry.
pub(super) struct Admission {
    ledger: Arc<Ledger>,
    retired: bool,
}
impl Admission {
    pub(super) fn identity(&self) -> usize {
        Arc::as_ptr(&self.ledger) as usize
    }
    pub(super) fn failure_signal(&self) -> FailureSignal {
        FailureSignal(Arc::clone(&self.ledger))
    }
}
pub(crate) struct FailureSignal(Arc<Ledger>);
impl FailureSignal {
    pub(crate) fn identity(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
    pub(crate) fn retirement_unavailable(&self) {
        self.0.fail(RETIREMENT_UNAVAILABLE);
    }
}
/// Unique callback-side writer. It does not allocate or wait when completing.
pub(super) struct Writer {
    ledger: Arc<Ledger>,
    sequence: u64,
    retired: bool,
}
impl Writer {
    pub(super) fn identity(&self) -> usize {
        Arc::as_ptr(&self.ledger) as usize
    }
    pub(super) fn fail_invariant(&self) {
        self.ledger.fail(INVARIANT_FAILED);
    }
}
pub(super) fn storage_bytes() -> usize {
    std::mem::size_of::<Ledger>()
}
/// Unique control-side observer. It can outlive the client endpoint. Its owner
/// accounts for one fixed ledger payload until this reader is dropped; retained
/// readers from replaced outputs are separate obligations. Poll and drop this
/// reader on control, and check `failure` when an outcome remains pending. The
/// callback publishes its block's transport and consumed status before these
/// receipts; that status can include later actions in the same block. The
/// native path reserves one shared group and up to four handles (admission,
/// callback, observer and retirement signal); an unsplit pair uses three. The
/// Arc header and allocator overhead are not included in `retained_data_bytes`.
pub struct Reader {
    ledger: Arc<Ledger>,
    observed: u64,
    output_identity: u64,
}

pub(super) fn channel(output_identity: u64) -> (Admission, Writer, Reader) {
    let ledger = Arc::new(Ledger {
        slots: std::array::from_fn(|_| Slot::new()),
        failure: AtomicU8::new(HEALTHY),
        retired: AtomicBool::new(false),
    });
    (
        Admission {
            ledger: Arc::clone(&ledger),
            retired: false,
        },
        Writer {
            ledger: Arc::clone(&ledger),
            sequence: 0,
            retired: false,
        },
        Reader {
            ledger,
            observed: 0,
            output_identity,
        },
    )
}
impl Admission {
    /// Reserve only the next actual command ticket; queue refusal must roll it back.
    pub(super) fn reserve(&mut self, ticket: u64) -> bool {
        if ticket == 0 || self.ledger.failure.load(Acquire) != HEALTHY {
            return false;
        }
        let slot = self.ledger.slot(ticket);
        if slot.state.load(Acquire) != FREE {
            return false;
        }
        slot.ticket.store(ticket, Relaxed);
        slot.state.store(RESERVED, Release);
        true
    }
    /// Only the admission owner may cancel a reservation not published to the queue.
    pub(super) fn rollback(&mut self, ticket: u64) -> bool {
        let slot = self.ledger.slot(ticket);
        if slot.state.load(Acquire) != RESERVED || slot.ticket.load(Relaxed) != ticket {
            self.ledger.fail(INVARIANT_FAILED);
            return false;
        }
        slot.state.store(FREE, Release);
        true
    }
}
impl Writer {
    /// A failed completion leaves the original reservation in custody.
    pub(super) fn finish(&mut self, ticket: u64, outcome: Outcome) -> bool {
        // At most CAPACITY completions can be unobserved. Consequently the
        // next sequence cannot alias another live completion, even across wrap.
        let sequence = self.sequence.wrapping_add(1);
        let slot = self.ledger.slot(ticket);
        if slot.state.load(Acquire) != RESERVED || slot.ticket.load(Relaxed) != ticket {
            self.ledger.fail(INVARIANT_FAILED);
            return false;
        }
        let encoded = match outcome {
            Outcome::Applied => 1,
            Outcome::Superseded => 2,
            Outcome::Interrupted => 3,
            Outcome::Rejected(reason) => 4 + reason as u32,
        };
        slot.outcome.store(encoded, Relaxed);
        slot.sequence.store(sequence, Relaxed);
        slot.state.store(COMPLETE, Release);
        self.sequence = sequence;
        true
    }

    /// Resolve all still-reserved tickets after both exact endpoints have been
    /// returned to control and the callback is quiescent. Consuming both ends
    /// prevents either from resuming; foreign admission is returned untouched.
    /// An unresolved command is indeterminate, not rejected.
    pub(super) fn interrupt_after_quiescence(
        mut self,
        mut admission: Admission,
    ) -> Result<(), (Self, Admission)> {
        if !Arc::ptr_eq(&self.ledger, &admission.ledger) {
            return Err((self, admission));
        }
        for slot in &self.ledger.slots {
            if slot.state.load(Acquire) == RESERVED {
                self.sequence = self.sequence.wrapping_add(1);
                slot.outcome.store(3, Relaxed);
                slot.sequence.store(self.sequence, Relaxed);
                slot.state.store(COMPLETE, Release);
            }
        }
        self.retired = true;
        admission.retired = true;
        // No matching endpoint can publish again. Release makes every final
        // terminal visible before the reader observes this frontier.
        self.ledger.retired.store(true, Release);
        Ok(())
    }
}
impl Reader {
    /// True only after both matching endpoints have become quiescent and all
    /// remaining reservations have terminals. Unread receipts remain available;
    /// drain them after observing this frontier. Check failure independently.
    pub fn is_retired(&self) -> bool {
        self.ledger.retired.load(Acquire)
    }
    /// Exact output incarnation for correlating tickets across replacement.
    pub fn output_identity(&self) -> u64 {
        self.output_identity
    }
    pub fn retained_data_bytes(&self) -> usize {
        storage_bytes()
    }
    /// An unavailable stream is not proof that any pending action did or did
    /// not execute. The owner must stop relying on its receipts and recover.
    pub fn failure(&self) -> Option<Failure> {
        match self.ledger.failure.load(Acquire) {
            INVARIANT_FAILED => Some(Failure::InvariantFailed),
            RETIREMENT_UNAVAILABLE => Some(Failure::RetirementUnavailable),
            ENDPOINT_ABANDONED => Some(Failure::EndpointAbandoned),
            _ => None,
        }
    }
    /// Read only the next completion, even if a later slot became visible during a scan.
    pub fn try_take(&mut self) -> Option<Receipt> {
        let next = self.observed.wrapping_add(1);
        let slot = self.ledger.slots.iter().find(|slot| {
            slot.state.load(Acquire) == COMPLETE && slot.sequence.load(Relaxed) == next
        })?;
        let outcome = match slot.outcome.load(Relaxed) {
            1 => Outcome::Applied,
            2 => Outcome::Superseded,
            3 => Outcome::Interrupted,
            code => Outcome::Rejected(PlaybackActionRejection::from_code(code.checked_sub(4)?)?),
        };
        let receipt = Receipt {
            ticket: slot.ticket.load(Relaxed),
            outcome,
        };
        slot.state.store(FREE, Release);
        self.observed = next;
        Some(receipt)
    }
}
impl Drop for Admission {
    fn drop(&mut self) {
        if !self.retired {
            self.ledger.fail(ENDPOINT_ABANDONED);
        }
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        if !self.retired {
            self.ledger.fail(ENDPOINT_ABANDONED);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_every_terminal_in_completion_order() {
        let (mut admission, mut writer, mut reader) = channel(1);
        assert!(admission.reserve(1));
        assert!(admission.reserve(2));
        assert!(writer.finish(2, Outcome::Rejected(PlaybackActionRejection::OutputFailed)));
        assert!(writer.finish(1, Outcome::Applied));
        assert_eq!(
            reader.try_take(),
            Some(Receipt {
                ticket: 2,
                outcome: Outcome::Rejected(PlaybackActionRejection::OutputFailed)
            })
        );
        assert_eq!(
            reader.try_take(),
            Some(Receipt {
                ticket: 1,
                outcome: Outcome::Applied
            })
        );
        assert_eq!(reader.try_take(), None);
    }
    #[test]
    fn full_slots_refuse_without_overwriting_and_rollback_restores_capacity() {
        let (mut admission, mut writer, mut reader) = channel(1);
        for ticket in 1..=CAPACITY as u64 {
            assert!(admission.reserve(ticket));
            assert!(writer.finish(ticket, Outcome::Superseded));
        }
        assert!(!admission.reserve(CAPACITY as u64 + 1));
        assert_eq!(reader.try_take().map(|r| r.ticket), Some(1));
        assert!(admission.reserve(CAPACITY as u64 + 1));
        assert!(admission.rollback(CAPACITY as u64 + 1));
        assert!(admission.reserve(CAPACITY as u64 + 1));
    }
    #[test]
    fn duplicate_completion_and_sequence_wrap_preserve_order() {
        let (mut admission, mut writer, mut reader) = channel(1);
        assert!(admission.reserve(1));
        assert!(writer.finish(1, Outcome::Interrupted));
        assert!(!writer.finish(1, Outcome::Applied));
        assert_eq!(reader.failure(), Some(Failure::InvariantFailed));
        assert_eq!(
            reader.try_take(),
            Some(Receipt {
                ticket: 1,
                outcome: Outcome::Interrupted
            })
        );
        assert!(!admission.reserve(2));
        let (mut admission, mut writer, mut reader) = channel(1);
        writer.sequence = u64::MAX - 1;
        reader.observed = u64::MAX - 1;
        assert!(admission.reserve(2));
        assert!(admission.reserve(3));
        assert!(writer.finish(3, Outcome::Applied));
        assert!(writer.finish(2, Outcome::Superseded));
        assert_eq!(reader.try_take().map(|r| r.ticket), Some(3));
        assert_eq!(reader.try_take().map(|r| r.ticket), Some(2));
        assert_eq!(reader.try_take(), None);
    }
    #[test]
    fn callback_and_control_exchange_out_of_order_terminals_across_slot_reuse() {
        use std::{sync::mpsc::channel as message_channel, time::Duration};

        let (mut admission, mut writer, mut reader) = channel(1);
        let (commands, callback_commands) = message_channel::<(u64, u64)>();
        let (published, control_published) = message_channel::<()>();
        let (observed, callback_observed) = message_channel::<()>();
        let timeout = Duration::from_secs(2);
        std::thread::scope(|scope| {
            scope.spawn(move || {
                while let Ok((first, second)) = callback_commands.recv_timeout(timeout) {
                    assert!(writer.finish(second, Outcome::Superseded));
                    published.send(()).unwrap();
                    callback_observed.recv_timeout(timeout).unwrap();
                    assert!(writer.finish(first, Outcome::Applied));
                    published.send(()).unwrap();
                }
            });
            for pair in 0..100 {
                let first = pair * 2 + 1;
                let second = first + 1;
                assert!(admission.reserve(first));
                assert!(admission.reserve(second));
                commands.send((first, second)).unwrap();
                control_published.recv_timeout(timeout).unwrap();
                assert_eq!(
                    reader.try_take(),
                    Some(Receipt {
                        ticket: second,
                        outcome: Outcome::Superseded
                    })
                );
                observed.send(()).unwrap();
                control_published.recv_timeout(timeout).unwrap();
                assert_eq!(
                    reader.try_take(),
                    Some(Receipt {
                        ticket: first,
                        outcome: Outcome::Applied
                    })
                );
            }
            drop(commands);
        });
    }
    #[test]
    fn quiescent_interruption_preserves_actual_terminals_and_finishes_pending_tickets() {
        let (mut admission, mut writer, mut reader) = channel(1);
        assert!(admission.reserve(1));
        assert!(admission.reserve(2));
        assert!(admission.reserve(3));
        assert!(writer.finish(2, Outcome::Rejected(PlaybackActionRejection::OutputFailed)));
        assert!(!reader.is_retired());
        // No callback is assumed to run again after the endpoints are returned.
        assert!(writer.interrupt_after_quiescence(admission).is_ok());
        assert!(
            reader.is_retired(),
            "unread terminals do not delay quiescence"
        );
        assert_eq!(
            reader.try_take(),
            Some(Receipt {
                ticket: 2,
                outcome: Outcome::Rejected(PlaybackActionRejection::OutputFailed),
            })
        );
        assert_eq!(
            reader.try_take(),
            Some(Receipt {
                ticket: 1,
                outcome: Outcome::Interrupted
            })
        );
        assert_eq!(
            reader.try_take(),
            Some(Receipt {
                ticket: 3,
                outcome: Outcome::Interrupted
            })
        );
        assert_eq!(reader.try_take(), None);
    }
    #[test]
    fn quiescent_interruption_refuses_foreign_admission() {
        let (mut admission, mut writer, mut reader) = channel(1);
        let (foreign, _foreign_writer, foreign_reader) = channel(1);
        assert!(admission.reserve(1));
        let (returned_writer, _returned_foreign) = match writer.interrupt_after_quiescence(foreign)
        {
            Ok(()) => panic!("foreign admission must not terminate this output"),
            Err(endpoints) => endpoints,
        };
        writer = returned_writer;
        assert!(
            !reader.is_retired(),
            "a foreign pair cannot retire this reader"
        );
        assert!(!foreign_reader.is_retired());
        assert_eq!(reader.try_take(), None);
        assert!(writer.finish(1, Outcome::Applied));
        assert_eq!(
            reader.try_take(),
            Some(Receipt {
                ticket: 1,
                outcome: Outcome::Applied
            })
        );
    }
    #[test]
    fn abandoned_endpoint_is_failure_not_successful_retirement() {
        let (admission, writer, reader) = channel(1);
        drop(writer);
        assert!(!reader.is_retired());
        assert_eq!(reader.failure(), Some(Failure::EndpointAbandoned));
        drop(admission);
        assert!(!reader.is_retired());
        assert_eq!(reader.failure(), Some(Failure::EndpointAbandoned));
    }
    #[test]
    fn retirement_frontier_publishes_all_terminals_across_threads() {
        let (mut admission, mut writer, mut reader) = channel(1);
        assert!(admission.reserve(1));
        assert!(admission.reserve(2));
        assert!(writer.finish(2, Outcome::Applied));
        assert!(!reader.is_retired());
        std::thread::scope(|scope| {
            scope.spawn(move || assert!(writer.interrupt_after_quiescence(admission).is_ok()));
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            while !reader.is_retired() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
            assert_eq!(
                reader.try_take(),
                Some(Receipt {
                    ticket: 2,
                    outcome: Outcome::Applied
                })
            );
            assert_eq!(
                reader.try_take(),
                Some(Receipt {
                    ticket: 1,
                    outcome: Outcome::Interrupted
                })
            );
            assert_eq!(reader.try_take(), None);
        });
    }
    #[cfg(feature = "allocation-probe")]
    #[test]
    fn completion_and_observation_do_not_allocate_or_free() {
        let (mut admission, mut writer, mut reader) = channel(1);
        assert!(admission.reserve(1));
        let mut receipt = None;
        let allocations = allocation_counter::measure(|| {
            assert!(writer.finish(1, Outcome::Rejected(PlaybackActionRejection::OutputFailed)));
            assert!(!reader.is_retired());
            receipt = reader.try_take();
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            receipt,
            Some(Receipt {
                ticket: 1,
                outcome: Outcome::Rejected(PlaybackActionRejection::OutputFailed)
            })
        );
        assert!(writer.interrupt_after_quiescence(admission).is_ok());
        let allocations = allocation_counter::measure(|| assert!(reader.is_retired()));
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
}
