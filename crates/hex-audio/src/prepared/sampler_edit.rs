//! One reversible stopped-only control edit fence shared by sampler and Arrangement edits. Scalar transport exclusion,
//! not a source lease, graph admission or installation receipt.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoppedEditFence {
    output: u64,
    operation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoppedEditFenceState {
    Pending,
    Ready,
    Refused,
}
#[derive(Default)]
pub(super) struct Fence {
    // One control writer: even idle, odd requested. Never wraps. Queue entries
    // capture this serial before admission, so a suspended producer cannot
    // introduce a pre-fence command after the fence has been released.
    serial: AtomicU64,
    resident: AtomicBool,
    position: AtomicU64,
    pub(super) ready_transport_epoch: AtomicU64,
    output: AtomicU64,
    generation: AtomicU64,
    editor: AtomicU64,
    audio: AtomicU64,
    cut: AtomicU64,
    // Callback is the sole writer of acknowledgement and allowed keyboard epoch.
    ack: AtomicU64,
    accepted: AtomicBool,
    keyboard_epoch: AtomicU64,
    abort: AtomicU64,
    retired: AtomicU64,
}
pub(super) fn is_stop(action: PlaybackAction) -> bool {
    matches!(
        action,
        PlaybackAction::Transport(PlaybackCommand::Stop)
            | PlaybackAction::Session(hex_engine::SessionPlaybackCommand::Stop)
    )
}
impl Fence {
    pub(super) fn gate(&self) -> u64 {
        self.serial.load(SeqCst)
    }
    pub(super) fn active(&self) -> bool {
        self.gate() & 1 != 0
    }
    fn begin(
        &self,
        source: AppliedAudioSource,
        cut: u64,
    ) -> Result<StoppedEditFence, &'static str> {
        self.begin_source(source, cut, false)
    }
    fn begin_source(
        &self,
        source: AppliedAudioSource,
        cut: u64,
        resident: bool,
    ) -> Result<StoppedEditFence, &'static str> {
        self.retire_abort_on_control();
        let prior = self.gate();
        let operation = prior
            .checked_add(1)
            .filter(|n| *n != u64::MAX && prior & 1 == 0)
            .ok_or("sampler edit is busy or its identity is exhausted")?;
        self.resident.store(resident, SeqCst);
        self.output.store(source.output_identity, SeqCst);
        self.generation
            .store(source.context.project_generation, SeqCst);
        self.editor.store(source.context.editor_revision, SeqCst);
        self.audio.store(source.audio_revision, SeqCst);
        self.cut.store(cut, SeqCst);
        self.serial.store(operation, SeqCst);
        Ok(StoppedEditFence {
            output: source.output_identity,
            operation,
        })
    }
    fn release(&self, token: StoppedEditFence) {
        if self.output.load(SeqCst) == token.output && self.gate() == token.operation {
            if self.ack.load(SeqCst) == token.operation {
                self.serial.store(token.operation + 1, SeqCst);
            } else {
                // Preserve the already admitted prefix if no callback has yet
                // observed the fence. The gain request may finish immediately;
                // this one scalar retirement remains until callback or teardown.
                self.abort.store(token.operation, SeqCst);
                self.retire_abort_on_control();
            }
        }
    }
    pub(super) fn retire_abort_on_control(&self) {
        let operation = self.gate();
        if operation & 1 != 0
            && self.abort.load(SeqCst) == operation
            && self.retired.load(SeqCst) == operation
        {
            self.serial.store(operation + 1, SeqCst);
        }
    }
    pub(super) fn permits(&self, queued: QueuedCommand) -> bool {
        if is_stop(queued.action) {
            return true;
        }
        let now = self.gate();
        if now & 1 == 0 {
            return queued.sampler_edit_gate == now;
        }
        queued.sampler_edit_gate == now - 1 && queued.ticket <= self.cut.load(SeqCst)
    }
    fn acknowledge(
        &self,
        source: Option<AppliedAudioSource>,
        playing: bool,
        keyboard_epoch: u64,
        faulted: bool,
        consumed: u64,
    ) {
        let operation = self.gate();
        if operation & 1 == 0 || consumed < self.cut.load(SeqCst) {
            return;
        }
        if self.abort.load(SeqCst) == operation {
            self.retired.store(operation, SeqCst);
            return;
        }
        if self.ack.load(SeqCst) == operation {
            return;
        }
        let expected = AppliedAudioSource {
            output_identity: self.output.load(SeqCst),
            context: hex_engine::AudioSourceContext {
                project_generation: self.generation.load(SeqCst),
                editor_revision: self.editor.load(SeqCst),
            },
            audio_revision: self.audio.load(SeqCst),
        };
        if self.gate() != operation {
            return;
        }
        self.keyboard_epoch.store(keyboard_epoch, SeqCst);
        self.accepted
            .store(!faulted && !playing && source == Some(expected), SeqCst);
        self.ack.store(operation, SeqCst);
    }
    pub(super) fn state(&self, token: StoppedEditFence) -> StoppedEditFenceState {
        if self.gate() != token.operation
            || self.output.load(SeqCst) != token.output
            || self.abort.load(SeqCst) == token.operation
        {
            return StoppedEditFenceState::Refused;
        }
        if self.ack.load(SeqCst) != token.operation {
            return StoppedEditFenceState::Pending;
        }
        let accepted = self.accepted.load(SeqCst);
        if self.gate() != token.operation || !accepted {
            StoppedEditFenceState::Refused
        } else {
            StoppedEditFenceState::Ready
        }
    }
    pub(super) fn permits_keyboard(&self, epoch: u64, actual_epoch: u64) -> bool {
        let operation = self.gate();
        operation & 1 == 0
            || epoch
                == if self.ack.load(SeqCst) == operation {
                    self.keyboard_epoch.load(SeqCst)
                } else {
                    actual_epoch
                }
    }
}
impl Status {
    /// Scalar transport exclusion during a stopped edit, including accepted
    /// publication awaiting exact installation retirement. This grants no source custody.
    pub fn stopped_edit_pending(&self) -> bool {
        self.sampler_edit.active()
    }
}
impl PublicationControl {
    /// Fence an ordinary resident runtime after its admitted command prefix. The
    /// callback, rather than UI telemetry, proves source identity and stopped state.
    pub fn request_arrangement_edit_fence(
        &self,
        generation: u64,
        revision: u64,
    ) -> Result<StoppedEditFence, &'static str> {
        if self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
            || self.status.publication_pending.gain_active()
            || self.status.applied_audio_source().is_some()
            || revision == 0
            || self.status.applied_revision.load(Acquire) != revision
        {
            return Err("arrangement audio is not current or publication is pending");
        }
        self.status.sampler_edit.begin_source(
            AppliedAudioSource {
                output_identity: self.candidate_root.root_id(),
                audio_revision: revision,
                context: hex_engine::AudioSourceContext {
                    project_generation: generation,
                    editor_revision: 0,
                },
            },
            self.status.admitted_ticket.load(Acquire),
            true,
        )
    }
    /// Actual callback-observed stopped frame, available only for this ready gate.
    pub fn arrangement_edit_position(&self, token: StoppedEditFence) -> Option<u64> {
        (self.stopped_edit_fence_state(token) == StoppedEditFenceState::Ready
            && self.status.sampler_edit.resident.load(Acquire))
        .then(|| self.status.sampler_edit.position.load(Acquire))
    }
    pub fn request_sampler_edit_fence(
        &self,
        expected: AppliedAudioSource,
    ) -> Result<StoppedEditFence, &'static str> {
        if self.status.closing.load(Acquire) || self.status.applied_audio_source() != Some(expected)
        {
            return Err("sampler edit output is not current");
        }
        self.status
            .sampler_edit
            .begin(expected, self.status.admitted_ticket.load(Acquire))
    }
    pub fn stopped_edit_fence_state(&self, token: StoppedEditFence) -> StoppedEditFenceState {
        if self.status.closing.load(Acquire)
            || self.status.device_fault.load(Acquire)
            || self.status.render_fault.load(Acquire)
        {
            return StoppedEditFenceState::Refused;
        }
        self.status.sampler_edit.state(token)
    }
    pub fn release_stopped_edit_fence(&self, token: StoppedEditFence) {
        self.status.sampler_edit.release(token);
    }
    /// Terminal cancellation after authored commit, before old output drain.
    pub fn commit_sampler_edit_fence(&self, token: StoppedEditFence) -> Result<(), &'static str> {
        if self.stopped_edit_fence_state(token) != StoppedEditFenceState::Ready {
            return Err("sampler edit fence is no longer ready");
        }
        self.status.sampler_keyboard.revoke_all();
        Ok(())
    }
}
impl Callback {
    pub(super) fn acknowledge_sampler_edit(&self) {
        if !self.status.sampler_edit.active() {
            return;
        }
        if self.status.sampler_edit.resident.load(Acquire) {
            let active = self.active.as_ref().and_then(ActivePublication::legacy);
            let source =
                active
                    .filter(|a| a.explicit_control_context)
                    .map(|a| AppliedAudioSource {
                        output_identity: self.candidate_root.root_id(),
                        audio_revision: a.revision,
                        context: hex_engine::AudioSourceContext {
                            project_generation: a.control_context_generation,
                            editor_revision: 0,
                        },
                    });
            let operation = self.status.sampler_edit.gate();
            if self.status.sampler_edit.ack.load(SeqCst) != operation {
                self.status
                    .sampler_edit
                    .position
                    .store(active.map_or(0, |a| a.playback.position()), SeqCst);
                self.status
                    .sampler_edit
                    .ready_transport_epoch
                    .store(self.status.transport_epoch.load(Acquire), SeqCst);
            }
            self.status.sampler_edit.acknowledge(
                source,
                active.is_none_or(|a| a.playback.playing()),
                0,
                active.is_none()
                    || self.status.closing.load(Acquire)
                    || self.status.device_fault.load(Acquire)
                    || self.status.render_fault.load(Acquire),
                self.status.consumed_command.load(Acquire),
            );
            return;
        }
        let active = match self.active.as_ref() {
            Some(ActivePublication::Eligible(a)) => Some(a),
            _ => None,
        };
        let source = active.and_then(|a| a.applied_audio_source());
        let status = active
            .and_then(|a| a.runtime())
            .and_then(|r| r.current_sampler_edit_status());
        self.status.sampler_edit.acknowledge(
            source,
            status.is_none_or(|s| s.0.playing),
            status.map_or(0, |s| s.1),
            status.is_none()
                || self.status.closing.load(Acquire)
                || self.status.device_fault.load(Acquire)
                || self.status.render_fault.load(Acquire),
            self.status.consumed_command.load(Acquire),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> AppliedAudioSource {
        AppliedAudioSource {
            output_identity: 11,
            audio_revision: 7,
            context: hex_engine::AudioSourceContext {
                project_generation: 2,
                editor_revision: 3,
            },
        }
    }
    fn play(ticket: u64, sampler_edit_gate: u64) -> QueuedCommand {
        QueuedCommand {
            ticket,
            sampler_edit_gate,
            action: PlaybackAction::Transport(PlaybackCommand::Play),
        }
    }
    #[test]
    fn earlier_play_is_consumed_before_stopped_ack_and_later_play_never_replays() {
        let fence = Fence::default();
        let older = play(1, fence.gate());
        let token = fence.begin(source(), 1).unwrap();
        assert!(fence.permits(older));
        // A request can arrive after this callback took its consumption cut.
        // It must wait for the following callback to consume that earlier Play.
        fence.acknowledge(Some(source()), false, 9, false, 0);
        assert_eq!(fence.state(token), StoppedEditFenceState::Pending);
        fence.acknowledge(Some(source()), true, 9, false, 1);
        assert_eq!(fence.state(token), StoppedEditFenceState::Refused);
        let raced = play(2, 0);
        assert!(!fence.permits(raced));
        fence.release(token);
        assert!(!fence.permits(raced));
        assert!(fence.permits(play(3, fence.gate())));
        let next = fence.begin(source(), 2).unwrap();
        // A delayed old producer must not gain permission just because a later
        // request's admitted prefix now includes its ticket.
        assert!(!fence.permits(raced));
        fence.release(token);
        assert_eq!(fence.state(next), StoppedEditFenceState::Pending);
    }
    #[test]
    fn exact_source_stopped_ack_pins_existing_keyboard_and_keeps_stop_priority() {
        let fence = Fence::default();
        let token = fence.begin(source(), 0).unwrap();
        assert!(!fence.permits(play(1, 0)));
        assert!(fence.permits(QueuedCommand {
            action: PlaybackAction::Transport(PlaybackCommand::Stop),
            ..play(1, 0)
        }));
        fence.acknowledge(Some(source()), false, 9, false, 0);
        assert_eq!(fence.state(token), StoppedEditFenceState::Ready);
        assert!(fence.permits_keyboard(9, 9));
        assert!(!fence.permits_keyboard(10, 9));
        fence.release(token);
        assert!(fence.permits_keyboard(10, 9));
        let next = fence.begin(source(), 0).unwrap();
        let mut changed = source();
        changed.audio_revision += 1;
        fence.acknowledge(Some(changed), false, 9, false, 0);
        assert_eq!(fence.state(next), StoppedEditFenceState::Refused);
        assert_eq!(fence.state(token), StoppedEditFenceState::Refused);
    }
    #[test]
    fn timeout_abort_preserves_admitted_prefix_until_callback_retirement_without_replaying_later_play()
     {
        let fence = Fence::default();
        let earlier = play(1, fence.gate());
        let token = fence.begin(source(), 1).unwrap();
        let raced = play(2, 0);
        fence.release(token); // lost-callback timeout, no ACK yet
        assert_eq!(fence.state(token), StoppedEditFenceState::Refused);
        assert!(fence.active());
        assert!(fence.begin(source(), 2).is_err());
        assert!(fence.permits(earlier));
        assert!(!fence.permits(raced));
        fence.acknowledge(Some(source()), true, 0, false, 0);
        fence.retire_abort_on_control();
        assert!(fence.active());
        fence.acknowledge(Some(source()), true, 0, false, 1);
        fence.retire_abort_on_control();
        assert!(!fence.active());
        assert!(!fence.permits(raced));
        let next = fence.begin(source(), 2).unwrap();
        fence.release(token);
        fence.retire_abort_on_control();
        assert_eq!(fence.state(next), StoppedEditFenceState::Pending);
        assert!(!fence.permits(raced));
        assert!(fence.permits(QueuedCommand {
            action: PlaybackAction::Transport(PlaybackCommand::Stop),
            ..raced
        }));
    }
    #[test]
    fn fault_and_identity_exhaustion_refuse_without_wrapping_or_losing_release() {
        let fence = Fence::default();
        let token = fence.begin(source(), 0).unwrap();
        assert!(fence.begin(source(), 0).is_err());
        fence.acknowledge(Some(source()), false, 0, true, 0);
        assert_eq!(fence.state(token), StoppedEditFenceState::Refused);
        fence.release(token);
        fence.serial.store(u64::MAX - 1, SeqCst);
        assert!(fence.begin(source(), 0).is_err());
        assert_eq!(fence.gate(), u64::MAX - 1);
    }
}
