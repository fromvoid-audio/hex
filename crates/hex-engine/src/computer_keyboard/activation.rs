//! Scalar output-local activation order. No runtime, source or publication authority.
use super::*;
use std::sync::atomic::Ordering::{AcqRel, SeqCst};

/// One output's monotonic keyboard activation clock. Control reserves epochs; priority Stop
/// revokes the reserved prefix synchronously before publishing its callback marker. The callback
/// never revokes a newer epoch while consuming an older marker. Exhaustion refuses activation.
#[derive(Debug, Default)]
pub struct KeyboardActivationClock {
    reserved: AtomicU64,
    revoked: AtomicU64,
}
impl KeyboardActivationClock {
    fn reserve_on_control(&self) -> Result<u64, Invalid> {
        self.reserved
            .fetch_update(AcqRel, Acquire, |epoch| epoch.checked_add(1))
            .map(|epoch| epoch + 1)
            .map_err(|_| Invalid("computer keyboard activation identity exhausted"))
    }
    pub fn revoke_reserved_prefix(&self) {
        self.revoked.fetch_max(self.reserved.load(Acquire), AcqRel);
    }
    pub(super) fn permits(&self, epoch: u64) -> bool {
        epoch != 0 && epoch > self.revoked.load(Acquire) && epoch <= self.reserved.load(Acquire)
    }
}

#[derive(Debug)]
pub(super) struct OutputActivation {
    pub(super) clock: Arc<KeyboardActivationClock>,
    pub(super) epoch: AtomicU64,
}
impl ComputerKeyboardInput {
    /// Allocate the input on control, using only the output's scalar clock. This retains no
    /// Status, callback, graph or source owner. One UI/control writer owns all desired-note writes.
    pub fn with_output_activation(clock: Arc<KeyboardActivationClock>) -> Result<Self, Invalid> {
        let epoch = clock.reserve_on_control()?;
        Ok(Self {
            output_activation: Some(OutputActivation {
                clock,
                epoch: AtomicU64::new(epoch),
            }),
            ..Self::default()
        })
    }
    pub fn is_output_bound(&self) -> bool {
        self.output_activation.is_some()
    }
    /// Scalar endpoint correlation only, never graph or resource admission.
    pub fn bound_to_clock(&self, clock: &KeyboardActivationClock) -> bool {
        self.output_activation
            .as_ref()
            .is_some_and(|activation| std::ptr::eq(activation.clock.as_ref(), clock))
    }

    /// Actual retained scalar allocation charge, including both Arc headers. Output and input
    /// owners may conservatively charge the same small clock; no native resource is represented.
    pub fn control_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.ordered_keys.as_ref().map_or(0, |_| {
                std::mem::size_of::<super::ordered_keys::OrderedKeys>()
            })
            + 2 * std::mem::size_of::<usize>()
            + self.output_activation.as_ref().map_or(0, |_| {
                std::mem::size_of::<KeyboardActivationClock>() + 2 * std::mem::size_of::<usize>()
            })
    }
    pub(super) fn begin_write(&self) -> Result<u64, Invalid> {
        let before = self.sequence.load(SeqCst);
        let after = before
            .checked_add(2)
            .filter(|_| before & 1 == 0)
            .ok_or(Invalid("computer keyboard input sequence exhausted"))?;
        self.sequence.store(before + 1, SeqCst);
        Ok(after)
    }
    /// Clear physical words on control without ending the rooted input. Client Stop separately
    /// revokes its activation prefix. Exhaustion terminally cancels, so Stop always remains effective.
    pub fn clear_keys_on_control(&self) {
        let Ok(after) = self.begin_write() else {
            self.cancel();
            return;
        };
        for (key, word) in self.notes.iter().enumerate() {
            let before = word.swap(0, SeqCst);
            if let Some(queue) = &self.ordered_keys {
                queue.publish_change(key, before, 0);
            }
        }
        self.sequence.store(after, SeqCst);
    }
    /// Share a still-permitted output epoch between physical and hardware input.
    /// Only a revoked epoch requires a fresh reservation and cleared old words.
    pub fn ensure_output_activation_on_control(&self) -> Result<(), Invalid> {
        if self.is_note_audition() {
            return Err(Invalid("note audition requires a fresh input"));
        }
        if self.is_cancelled() {
            return Err(Invalid("computer keyboard input is cancelled"));
        }
        let activation = self
            .output_activation
            .as_ref()
            .ok_or(Invalid("computer keyboard input is not output bound"))?;
        if activation.clock.permits(activation.epoch.load(Acquire)) {
            Ok(())
        } else {
            self.reactivate_on_control()
        }
    }
    /// A fresh physical press after priority Stop reuses the admitted root. Terminal lifecycle
    /// cancellation is irreversible. Clear old words before publishing the new activation epoch.
    pub fn reactivate_on_control(&self) -> Result<(), Invalid> {
        if self.is_note_audition() {
            return Err(Invalid("note audition requires a fresh input"));
        }
        if self.is_cancelled() {
            return Err(Invalid("computer keyboard input is cancelled"));
        }
        let activation = self
            .output_activation
            .as_ref()
            .ok_or(Invalid("computer keyboard input is not output bound"))?;
        let epoch = activation.clock.reserve_on_control()?;
        let after = self.begin_write()?;
        for word in &self.notes {
            word.store(0, SeqCst);
        }
        self.rejected.store(false, Release);
        activation.epoch.store(epoch, SeqCst);
        self.sequence.store(after, SeqCst);
        if self.is_cancelled() {
            return Err(Invalid("computer keyboard input is cancelled"));
        }
        Ok(())
    }
    pub(crate) fn note_snapshot(&self) -> NoteSnapshot {
        for _ in 0..2 {
            let before = self.sequence.load(SeqCst);
            if before & 1 != 0 || self.is_cancelled() {
                continue;
            }
            let epoch = self
                .output_activation
                .as_ref()
                .map_or(0, |a| a.epoch.load(SeqCst));
            let words = if self.note_audition_finished() {
                [0; COMPUTER_KEY_COUNT]
            } else {
                std::array::from_fn(|index| self.notes[index].load(SeqCst))
            };
            if before == self.sequence.load(SeqCst)
                && !self.is_cancelled()
                && self
                    .output_activation
                    .as_ref()
                    .is_none_or(|a| a.clock.permits(epoch))
            {
                return NoteSnapshot::Active(epoch, words);
            }
        }
        let epoch = self
            .output_activation
            .as_ref()
            .map_or(0, |a| a.epoch.load(SeqCst));
        if self.is_cancelled()
            || self
                .output_activation
                .as_ref()
                .is_some_and(|a| !a.clock.permits(epoch))
        {
            NoteSnapshot::Inactive
        } else {
            // A note write in the same activation defers this block's update. A new epoch
            // cannot reuse old desired words; the runtime silences until a coherent snapshot.
            NoteSnapshot::Deferred(epoch)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exhausted_activation_and_note_sequences_refuse_without_wrapping_and_stop_still_cancels() {
        let clock = Arc::new(KeyboardActivationClock::default());
        let input = ComputerKeyboardInput::with_output_activation(Arc::clone(&clock)).unwrap();
        clock.reserved.store(u64::MAX, Release);
        assert!(input.reactivate_on_control().is_err());
        input.sequence.store(u64::MAX - 1, SeqCst);
        assert!(input.set_key(0, Some((60, 100, 1))).is_err());
        input.clear_keys_on_control();
        assert!(input.is_cancelled());
        assert!(input.reactivate_on_control().is_err());
        assert!(matches!(input.note_snapshot(), NoteSnapshot::Inactive));
    }
}
