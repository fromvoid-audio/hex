//! Control-side transport lease, revocation, parameter fencing, and editor scope.
use super::*;

/// Queue-independent revocation. Retain this control lease through worker join
/// and callback terminal return; neither endpoint may be destroyed on callback.
pub struct Control {
    pub(super) revoked: Arc<Revocation>,
}

/// Actual pickup of one accepted original transport request. This is neither
/// successful SDK processing, final Main delivery nor publication authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceivedProgram {
    pub binding: Binding,
    pub sequence: u64,
    pub program: Option<NonZeroU64>,
}
/// Control-side custody of the original pair's opt-in. This retains only
/// copied transport signals, not a worker, endpoint, SDK or output lease.
/// Drop on control after the last wanted marker is drained.
pub struct EditorScope {
    pub(super) signal: Arc<Revocation>,
    pub(super) serial: NonZeroU64,
}

impl Drop for EditorScope {
    fn drop(&mut self) {
        let ended = self.serial.get() << 1;
        let _ = self
            .signal
            .editor_scope
            .compare_exchange(ended | 1, ended, AcqRel, Acquire);
    }
}

impl Control {
    /// Enable observation on new physical quantum starts for this exact pair.
    /// Serial numbers must increase per pair. Ending discards unconsumed
    /// markers; drain wanted observations before ending an orderly scope.
    pub fn arm_editor_pcm(&mut self, serial: NonZeroU64) -> Result<EditorScope, Fault> {
        self.arm_editor_pcm_after(serial, || {})
    }

    /// The unique control owner installs its exact worker plan before the
    /// release publication can make this serial visible to the callback.
    pub fn arm_editor_pcm_after(
        &mut self,
        serial: NonZeroU64,
        prepare: impl FnOnce(),
    ) -> Result<EditorScope, Fault> {
        if self.is_revoked() {
            return Err(Fault::Revoked);
        }
        let value = serial.get().checked_mul(2).ok_or(Fault::Exhausted)? | 1;
        let previous = self.revoked.editor_scope.load(Acquire);
        if previous & 1 != 0 || serial.get() <= previous >> 1 {
            return Err(Fault::Protocol);
        }
        prepare();
        self.revoked.editor_scope.store(value, Release);
        Ok(EditorScope {
            signal: Arc::clone(&self.revoked),
            serial,
        })
    }

    /// Distinguish an untouched paired callback from a committed quantum.
    /// None includes a concurrent first acceptance, contention, revocation,
    /// and exhausted observation serial. Neither result stays valid by itself.
    pub fn quantum_observation(&self) -> Option<QuantumObservation> {
        use std::sync::atomic::Ordering::SeqCst;
        if self.revoked.is_revoked() {
            return None;
        }
        let serial = self.revoked.quantum_serial.load(SeqCst);
        if serial == 0 {
            return (self.revoked.quantum_serial.load(SeqCst) == 0 && !self.revoked.is_revoked())
                .then_some(QuantumObservation::Untouched(self.revoked.binding));
        }
        self.committed_quantum().map(QuantumObservation::Committed)
    }

    /// Read one bounded snapshot of the exact paired callback's most recently
    /// accepted quantum. None means no observation, contention, exhaustion, or
    /// revocation. A copied binding is not proof of any graph installation.
    pub fn committed_quantum(&self) -> Option<CommittedQuantum> {
        use std::sync::atomic::Ordering::SeqCst;
        if self.revoked.is_revoked() {
            return None;
        }
        let before = self.revoked.quantum_serial.load(SeqCst);
        if before == 0 || before & 1 != 0 {
            return None;
        }
        let key = self.revoked.quantum_program.load(SeqCst);
        let state = match self.revoked.quantum_state.load(SeqCst) {
            1 => CommittedQuantumState::Closed,
            2 => CommittedQuantumState::OpenUnprogrammed,
            3 => CommittedQuantumState::OpenProgram(NonZeroU64::new(key)?),
            _ => return None,
        };
        (before == self.revoked.quantum_serial.load(SeqCst) && !self.revoked.is_revoked())
            .then_some(CommittedQuantum {
                binding: self.revoked.binding,
                state,
            })
    }
    pub fn binding(&self) -> Binding {
        self.revoked.binding
    }
    pub fn revoke(&self) {
        self.revoked.revoke();
    }
    /// Wake the parked transport worker without allocation or mutexes.
    pub fn wake_worker(&self) {
        self.revoked.wake_worker();
    }
    pub fn is_revoked(&self) -> bool {
        self.revoked.is_revoked()
    }
    /// One bounded coherent observation from this constructor-paired worker.
    /// A concurrent publication or revocation returns no observation; retry on
    /// a later control poll. No input/current-graph key is substituted.
    pub fn received_program(&self) -> Option<ReceivedProgram> {
        if self.is_revoked() {
            return None;
        }
        let (sequence, program) = self.revoked.received_program()?;
        (!self.is_revoked()).then_some(ReceivedProgram {
            binding: self.revoked.binding,
            sequence,
            program,
        })
    }

    /// This live pair has published no worker pickup yet. A failed coherent
    /// read of a nonzero prefix is a different observation. The callback may
    /// still be assembling or sending a quantum.
    pub fn has_no_pcm_pickup(&self) -> bool {
        self.revoked.received_prefix.load(Acquire) == 0 && !self.is_revoked()
    }

    /// Retained observation for the terminal MissingOutput of this exact pair.
    pub fn deadline_miss(&self) -> Option<TerminalDeadlineMiss> {
        let worker_phase = match self.revoked.missed_phase.load(Acquire) {
            1 => DeadlineWorkerPhase::AwaitingPickup,
            2 => DeadlineWorkerPhase::Processing,
            3 => DeadlineWorkerPhase::Publishing,
            _ => return None,
        };
        Some(TerminalDeadlineMiss {
            miss: DeadlineMiss {
                reply_sequence: self.revoked.missed_reply.load(Relaxed),
                worker_phase,
            },
            last_received_sequence: self
                .revoked
                .missed_received_prefix
                .load(Relaxed)
                .checked_sub(1),
            last_processed_sequence: self
                .revoked
                .missed_processed_prefix
                .load(Relaxed)
                .checked_sub(1),
        })
    }
    /// Drain at most one accepted concealment from this exact pair. Contention
    /// defers the read; an absent value is not evidence that no gap occurred.
    pub fn take_concealed_output(&self) -> Option<ConcealedOutput> {
        self.revoked
            .concealed_slot
            .compare_exchange(2, 1, Acquire, Relaxed)
            .ok()?;
        let sequence = self.revoked.concealed_reply.load(Relaxed);
        let phase = self.revoked.concealed_phase.load(Relaxed);
        let overflowed = self.revoked.concealed_overflow.swap(false, Relaxed);
        self.revoked.concealed_slot.store(0, Release);
        let worker_phase = match phase {
            1 => DeadlineWorkerPhase::AwaitingPickup,
            2 => DeadlineWorkerPhase::Processing,
            3 => DeadlineWorkerPhase::Publishing,
            _ => return None,
        };
        Some(ConcealedOutput {
            binding: self.revoked.binding,
            miss: DeadlineMiss {
                reply_sequence: sequence,
                worker_phase,
            },
            overflowed,
        })
    }
    /// Whether this transport was revoked cleanly by an intentional PriorityStop
    /// while still live and unfaulted.
    pub fn was_revoked_for_stop(&self) -> bool {
        self.revoked.is_stop_revoked()
    }
    /// The exact callback's first actionable fault, once. A control/worker
    /// revocation alone and a later callback observing it have no new cause.
    pub fn take_callback_fault(&self) -> Option<Fault> {
        let code = self.revoked.callback_fault.load(Acquire);
        if code == 0 || code == Revocation::STOP_REVOKED_CODE || code & Revocation::REPORTED != 0 {
            return None;
        }
        let previous = self
            .revoked
            .callback_fault
            .fetch_or(Revocation::REPORTED, AcqRel);
        if previous & Revocation::REPORTED != 0 {
            return None;
        }
        Revocation::decode_fault(previous).filter(|&fault| fault != Fault::Revoked)
    }
}
impl Drop for Control {
    fn drop(&mut self) {
        self.revoke();
    }
}
