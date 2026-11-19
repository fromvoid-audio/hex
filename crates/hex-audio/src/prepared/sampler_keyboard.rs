//! Fixed scalar output mailbox: control is the sole binding writer, UI the sole
//! key writer. Cancellation is an independent monotonic atomic high-water mark.
use super::*;
use hex_engine::computer_keyboard::{
    COMPUTER_KEY_COUNT, CurrentSamplerKeyboardFrame, valid_key_word,
};
#[derive(Default)]
struct Key {
    sequence: AtomicU64,
    epoch: AtomicU64,
    word: AtomicU64,
}
impl Key {
    fn publish(&self, epoch: u64, word: u64) -> Result<(), &'static str> {
        let before = self.sequence.load(SeqCst);
        let after = before
            .checked_add(2)
            .filter(|_| before & 1 == 0)
            .ok_or("sampler keyboard key sequence exhausted")?;
        self.sequence.store(before + 1, SeqCst);
        self.epoch.store(epoch, SeqCst);
        self.word.store(word, SeqCst);
        self.sequence.store(after, SeqCst);
        Ok(())
    }
}
#[derive(Default)]
pub(super) struct Mailbox {
    reserved: AtomicU64,
    revoked: AtomicU64,
    callback_revoked: AtomicU64,
    rejected: AtomicU64,
    sequence: AtomicU64,
    epoch: AtomicU64,
    output: AtomicU64,
    generation: AtomicU64,
    editor: AtomicU64,
    audio: AtomicU64,
    track: AtomicU64,
    device: AtomicU64,
    keys: [Key; COMPUTER_KEY_COUNT],
}
pub(super) enum Read {
    Deferred,
    Frame(Option<CurrentSamplerKeyboardFrame>),
}
impl Mailbox {
    fn reserve(&self) -> Result<u64, &'static str> {
        self.reserved
            .fetch_update(SeqCst, SeqCst, |n| {
                n.checked_add(1).filter(|next| *next != u64::MAX)
            })
            .map(|old| old + 1)
            .map_err(|_| "sampler keyboard activation identity exhausted")
    }

    pub(super) fn revoke_all(&self) {
        self.revoked.fetch_max(self.reserved.load(SeqCst), SeqCst);
    }
    fn cancel(&self, epoch: u64) {
        if epoch != 0 && epoch <= self.reserved.load(SeqCst) {
            self.revoked.fetch_max(epoch, SeqCst);
        }
    }
    // The callback is the sole writer of these two words. Plain atomic
    // load/store avoids an atomic-max CAS retry path on the audio thread.
    fn cancel_on_callback(&self, epoch: u64) {
        self.callback_revoked
            .store(self.callback_revoked.load(SeqCst).max(epoch), SeqCst);
    }
    pub(super) fn revoke_on_callback(&self) {
        self.cancel_on_callback(self.reserved.load(SeqCst));
    }
    fn reject_on_callback(&self, epoch: u64) {
        self.rejected
            .store(self.rejected.load(SeqCst).max(epoch), SeqCst);
        self.cancel_on_callback(epoch);
    }
    fn revoked_through(&self) -> u64 {
        self.revoked
            .load(SeqCst)
            .max(self.callback_revoked.load(SeqCst))
    }
    fn valid_epoch(&self, epoch: u64) -> bool {
        epoch != 0 && epoch <= self.reserved.load(SeqCst) && epoch > self.revoked_through()
    }
    fn bind(
        &self,
        applied: AppliedAudioSource,
        epoch: u64,
        track: u64,
        device: u64,
    ) -> Result<(), &'static str> {
        if !self.valid_epoch(epoch) || epoch <= self.epoch.load(SeqCst) || track == 0 || device == 0
        {
            return Err("sampler keyboard activation is stale");
        }
        let before = self.sequence.load(SeqCst);
        let after = before
            .checked_add(2)
            .filter(|_| before & 1 == 0)
            .ok_or("sampler keyboard binding sequence exhausted")?;
        self.sequence.store(before + 1, SeqCst);
        self.output.store(applied.output_identity, SeqCst);
        self.generation
            .store(applied.context.project_generation, SeqCst);
        self.editor.store(applied.context.editor_revision, SeqCst);
        self.audio.store(applied.audio_revision, SeqCst);
        self.track.store(track, SeqCst);
        self.device.store(device, SeqCst);
        self.epoch.store(epoch, SeqCst);
        self.sequence.store(after, SeqCst);
        if !self.valid_epoch(epoch) {
            return Err("sampler keyboard activation was cancelled");
        }
        Ok(())
    }
    pub(super) fn read(&self, applied: AppliedAudioSource) -> Read {
        // Cancellation must win even while control is writing the binding.
        let epoch = self.epoch.load(SeqCst);
        if epoch != 0 && epoch <= self.revoked_through() {
            return Read::Frame(None);
        }
        for _ in 0..3 {
            let before = self.sequence.load(SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let epoch = self.epoch.load(SeqCst);
            let track = self.track.load(SeqCst);
            let device = self.device.load(SeqCst);
            let matches = self.output.load(SeqCst) == applied.output_identity
                && self.generation.load(SeqCst) == applied.context.project_generation
                && self.editor.load(SeqCst) == applied.context.editor_revision
                && self.audio.load(SeqCst) == applied.audio_revision;
            if self.sequence.load(SeqCst) != before {
                continue;
            }
            if !matches || !self.valid_epoch(epoch) {
                return Read::Frame(None);
            }
            let mut words = [0; COMPUTER_KEY_COUNT];
            for (word, key) in words.iter_mut().zip(&self.keys) {
                let mut stable = false;
                for _ in 0..3 {
                    let sequence = key.sequence.load(SeqCst);
                    if sequence & 1 != 0 {
                        continue;
                    }
                    let key_epoch = key.epoch.load(SeqCst);
                    let value = key.word.load(SeqCst);
                    if sequence == key.sequence.load(SeqCst) {
                        *word = if key_epoch == epoch { value } else { 0 };
                        stable = true;
                        break;
                    }
                }
                if !stable {
                    return if self.valid_epoch(epoch) {
                        Read::Deferred
                    } else {
                        Read::Frame(None)
                    };
                }
            }
            if self.sequence.load(SeqCst) != before {
                continue;
            }
            if !self.valid_epoch(epoch) {
                return Read::Frame(None);
            }
            return Read::Frame(Some(CurrentSamplerKeyboardFrame {
                epoch,
                track,
                device,
                words,
            }));
        }
        Read::Deferred
    }
}
impl Client {
    pub fn sampler_keyboard_epoch_rejected(&self, output: u64, epoch: u64) -> bool {
        self.owner_ref().is_some_and(|e| {
            e.output_identity == output && e.status.sampler_keyboard.rejected.load(Acquire) == epoch
        })
    }

    /// Reserve before submitting worker activation so cancellation also defeats
    /// a delayed activation. This scalar never grants source/runtime admission.
    pub fn reserve_sampler_keyboard_epoch(&self) -> Result<(u64, u64), &'static str> {
        let endpoints = self
            .owner_ref()
            .ok_or("sampler keyboard output is unavailable")?;
        let status = &endpoints.status;
        if status.sampler_edit.active()
            || status.closing.load(Acquire)
            || status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || status
                .applied_audio_source()
                .is_none_or(|a| a.output_identity != endpoints.output_identity)
        {
            return Err("sampler keyboard requires an applied audio output");
        }
        let epoch = status.sampler_keyboard.reserve()?;
        Ok((endpoints.output_identity, epoch))
    }
    pub fn sampler_keyboard_epoch_revoked(&self, output: u64, epoch: u64) -> bool {
        self.owner_ref().is_none_or(|e| {
            e.output_identity != output
                || e.status.closing.load(Acquire)
                || !e.status.sampler_keyboard.valid_epoch(epoch)
        })
    }
    pub fn cancel_sampler_keyboard_epoch(&self, output: u64, epoch: u64) {
        if let Some(endpoints) = self.owner_ref().filter(|e| e.output_identity == output) {
            endpoints.status.sampler_keyboard.cancel(epoch);
        }
    }
    /// One UI writer publishes the existing physical-key word law. Binding has
    /// its own control-writer sequence; no shared multiwriter seqlock exists.
    pub fn sampler_keyboard_key(
        &self,
        output: u64,
        epoch: u64,
        index: usize,
        word: u64,
    ) -> Result<(), &'static str> {
        let endpoints = self
            .owner_ref()
            .filter(|e| e.output_identity == output)
            .ok_or("sampler keyboard output changed")?;
        let status = &endpoints.status;
        let mailbox = &status.sampler_keyboard;
        if status.closing.load(Acquire)
            || status.device_fault.load(Acquire)
            || status.render_fault.load(Acquire)
            || !mailbox.valid_epoch(epoch)
            || mailbox.epoch.load(SeqCst) != epoch
            || status
                .applied_audio_source()
                .is_none_or(|a| a.output_identity != output)
        {
            return Err("sampler keyboard activation is not current");
        }
        if !valid_key_word(word) {
            return Err("sampler keyboard key is invalid");
        }
        let key = mailbox
            .keys
            .get(index)
            .ok_or("sampler keyboard key is out of range")?;
        key.publish(epoch, word)
    }
}
impl PublicationControl {
    /// Called only on the existing publication worker, against actual installed
    /// CURRENT identity. No UI object or source owner enters the callback.
    pub fn activate_sampler_keyboard(
        &self,
        expected: AppliedAudioSource,
        epoch: u64,
        track: u64,
        device: u64,
    ) -> Result<(), &'static str> {
        if self.status.sampler_edit.active()
            || self.status.closing.load(Acquire)
            || self.status.applied_audio_source() != Some(expected)
        {
            return Err("sampler keyboard output is not current");
        }
        self.status
            .sampler_keyboard
            .bind(expected, epoch, track, device)?;
        if self.status.sampler_edit.active() {
            return Err("sampler edit is preparing");
        }
        Ok(())
    }
}
impl Callback {
    pub(super) fn revoke_current_sampler_keyboard(&self) {
        if let Some(ActivePublication::Eligible(active)) = self.active.as_ref()
            && let Some(epoch) = active
                .runtime()
                .and_then(|r| r.current_sampler_keyboard_epoch())
        {
            self.status.sampler_keyboard.cancel_on_callback(epoch);
        }
    }

    pub(super) fn report_sampler_keyboard_rejection(&self) {
        if let Some(ActivePublication::Eligible(active)) = self.active.as_ref()
            && let Some(epoch) = active
                .runtime()
                .and_then(|r| r.current_sampler_keyboard_rejected_epoch())
        {
            self.status.sampler_keyboard.reject_on_callback(epoch);
        }
    }

    pub(super) fn read_sampler_keyboard(&mut self) {
        let Some(ActivePublication::Eligible(active)) = self.active.as_mut() else {
            return;
        };
        let Some(applied) = active.applied_audio_source() else {
            return;
        };
        let actual_epoch = active
            .runtime()
            .and_then(|r| r.current_sampler_keyboard_epoch())
            .unwrap_or(0);
        // Terminal cancellation belongs to the actual sounding epoch, not the
        // latest mailbox binding. A newer bound epoch may be fenced or torn;
        // neither may defer cancellation of the older runtime voice.
        let terminal_cancelled =
            actual_epoch != 0 && !self.status.sampler_keyboard.valid_epoch(actual_epoch);
        if terminal_cancelled
            && let Some(runtime) = active.runtime_mut()
            && runtime.apply_current_sampler_keyboard(None).is_err()
        {
            self.status
                .sampler_keyboard
                .reject_on_callback(actual_epoch);
            return;
        }
        let Read::Frame(frame) = self.status.sampler_keyboard.read(applied) else {
            return;
        };
        if terminal_cancelled && frame.is_none() {
            return; // the same terminal reset already completed above
        }
        if frame.is_some_and(|f| {
            !self
                .status
                .sampler_edit
                .permits_keyboard(f.epoch, actual_epoch)
        }) {
            return;
        }
        if let Some(runtime) = active.runtime_mut()
            && runtime.apply_current_sampler_keyboard(frame).is_err()
        {
            self.status
                .sampler_keyboard
                .reject_on_callback(frame.map_or(0, |f| f.epoch));
        }
    }
}

impl Status {
    /// Scalar refusal telemetry, not a source or installation capability.
    pub fn sampler_keyboard_rejected_epoch(&self) -> u64 {
        self.sampler_keyboard.rejected.load(Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn applied() -> AppliedAudioSource {
        AppliedAudioSource {
            context: hex_engine::AudioSourceContext {
                project_generation: 2,
                editor_revision: 7,
            },
            audio_revision: 3,
            output_identity: 11,
        }
    }
    fn frame(mailbox: &Mailbox) -> Option<CurrentSamplerKeyboardFrame> {
        match mailbox.read(applied()) {
            Read::Frame(frame) => frame,
            Read::Deferred => panic!("stable mailbox was deferred"),
        }
    }
    #[test]
    fn cancelled_late_bindings_and_old_keys_cannot_reactivate_or_retarget_a_new_epoch() {
        let mailbox = Mailbox::default();
        let first = mailbox.reserve().unwrap();
        mailbox.cancel(first);
        assert!(mailbox.bind(applied(), first, 1, 2).is_err());
        let second = mailbox.reserve().unwrap();
        mailbox.bind(applied(), second, 1, 2).unwrap();
        mailbox.keys[0]
            .publish(first, (1 << 16) | (127 << 8) | 61)
            .unwrap();
        assert_eq!(frame(&mailbox).unwrap().words, [0; COMPUTER_KEY_COUNT]);
        mailbox.cancel(first);
        mailbox.cancel_on_callback(first);
        mailbox.keys[0]
            .publish(second, (2 << 16) | (127 << 8) | 61)
            .unwrap();
        assert_ne!(frame(&mailbox).unwrap().words[0], 0);
        assert!(mailbox.bind(applied(), first, 3, 4).is_err());
        assert!(mailbox.bind(applied(), second, 3, 4).is_err());
        assert_eq!(frame(&mailbox).unwrap().track, 1);
        let mut stale = applied();
        stale.context.editor_revision += 1;
        assert!(matches!(mailbox.read(stale), Read::Frame(None)));
        stale = applied();
        stale.output_identity += 1;
        assert!(matches!(mailbox.read(stale), Read::Frame(None)));
        mailbox.cancel(second);
        assert!(frame(&mailbox).is_none());
    }
    #[test]
    fn contended_snapshots_are_bounded_and_cancellation_wins_over_both_writer_sequences() {
        let mailbox = Mailbox::default();
        let epoch = mailbox.reserve().unwrap();
        mailbox.bind(applied(), epoch, 1, 2).unwrap();
        mailbox.keys[0].sequence.store(1, SeqCst);
        assert!(matches!(mailbox.read(applied()), Read::Deferred));
        mailbox.cancel(epoch);
        assert!(matches!(mailbox.read(applied()), Read::Frame(None)));
        mailbox.sequence.store(3, SeqCst);
        assert!(matches!(mailbox.read(applied()), Read::Frame(None)));
        let other = Mailbox::default();
        other.sequence.store(1, SeqCst);
        assert!(matches!(other.read(applied()), Read::Deferred));
    }
    #[test]
    fn epoch_binding_and_key_sequences_refuse_exhaustion_without_wrapping_or_partial_write() {
        let mailbox = Mailbox::default();
        mailbox.reserved.store(u64::MAX - 1, SeqCst);
        assert!(mailbox.reserve().is_err());
        assert_eq!(mailbox.reserved.load(SeqCst), u64::MAX - 1);
        mailbox.sequence.store(u64::MAX - 1, SeqCst);
        assert!(mailbox.bind(applied(), 1, 1, 2).is_err());
        assert_eq!(mailbox.epoch.load(SeqCst), 0);
        let key = Key::default();
        key.sequence.store(u64::MAX - 1, SeqCst);
        assert!(key.publish(1, (1 << 16) | (127 << 8) | 61).is_err());
        assert_eq!(key.epoch.load(SeqCst), 0);
        assert_eq!(key.word.load(SeqCst), 0);
    }
}
