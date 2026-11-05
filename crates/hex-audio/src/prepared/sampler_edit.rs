//! One reversible control edit fence shared by stopped sampler, native epochs and Arrangement edits. Scalar transport exclusion,
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
    native_epoch: AtomicBool,
    ordinary_native_epoch: AtomicBool,
    allow_playing: AtomicBool,
    playing: AtomicBool,
    position: AtomicU64,
    loop_start: AtomicU64,
    loop_end: AtomicU64,
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
enum NativeEpoch {
    None,
    Current,
    Ordinary,
}
pub(super) fn is_stop(action: PlaybackAction) -> bool {
    matches!(
        action,
        PlaybackAction::Transport(PlaybackCommand::Stop)
            | PlaybackAction::Session(hex_engine::SessionPlaybackCommand::Stop)
    )
}
impl Fence {
    /// Match the original acknowledged source again at a later callback
    /// frontier; readiness alone may describe an older active publication.
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        expect(
            dead_code,
            reason = "ordinary native epochs are admitted only on macos arm64"
        )
    )]
    pub(super) fn matches_source(
        &self,
        token: StoppedEditFence,
        source: AppliedAudioSource,
    ) -> bool {
        self.state(token) == StoppedEditFenceState::Ready
            && source.output_identity == self.output.load(SeqCst)
            && source.audio_revision == self.audio.load(SeqCst)
            && source.context.project_generation == self.generation.load(SeqCst)
            && source.context.editor_revision == self.editor.load(SeqCst)
            && self.gate() == token.operation
    }
    pub(super) fn native_epoch_active(&self) -> bool {
        self.active() && self.native_epoch.load(SeqCst)
    }
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        expect(
            dead_code,
            reason = "ordinary native epochs are admitted only on macos arm64"
        )
    )]
    pub(super) fn ordinary_native_epoch_active(&self) -> bool {
        self.native_epoch_active() && self.ordinary_native_epoch.load(SeqCst)
    }
    pub(super) fn native_epoch_context(
        &self,
        token: StoppedEditFence,
    ) -> Option<hex_engine::AudioSourceContext> {
        (self.native_epoch_active() && self.state(token) == StoppedEditFenceState::Ready).then(
            || hex_engine::AudioSourceContext {
                project_generation: self.generation.load(SeqCst),
                editor_revision: self.editor.load(SeqCst),
            },
        )
    }
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
        self.begin_source(source, cut, false, false, false)
    }
    pub(super) fn begin_source(
        &self,
        source: AppliedAudioSource,
        cut: u64,
        resident: bool,
        allow_playing: bool,
        native_epoch: bool,
    ) -> Result<StoppedEditFence, &'static str> {
        self.begin_source_with_epoch(
            source,
            cut,
            resident,
            allow_playing,
            if native_epoch {
                NativeEpoch::Current
            } else {
                NativeEpoch::None
            },
        )
    }
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        expect(
            dead_code,
            reason = "ordinary native epochs are admitted only on macos arm64"
        )
    )]
    pub(super) fn begin_ordinary_native_epoch_source(
        &self,
        source: AppliedAudioSource,
        cut: u64,
    ) -> Result<StoppedEditFence, &'static str> {
        self.begin_source_with_epoch(source, cut, false, false, NativeEpoch::Ordinary)
    }
    fn begin_source_with_epoch(
        &self,
        source: AppliedAudioSource,
        cut: u64,
        resident: bool,
        allow_playing: bool,
        native_epoch: NativeEpoch,
    ) -> Result<StoppedEditFence, &'static str> {
        self.retire_abort_on_control();
        let prior = self.gate();
        let operation = prior
            .checked_add(1)
            .filter(|n| *n != u64::MAX && prior & 1 == 0)
            .ok_or("sampler edit is busy or its identity is exhausted")?;
        self.allow_playing.store(allow_playing, SeqCst);
        self.resident.store(resident, SeqCst);
        self.native_epoch
            .store(!matches!(native_epoch, NativeEpoch::None), SeqCst);
        self.ordinary_native_epoch
            .store(matches!(native_epoch, NativeEpoch::Ordinary), SeqCst);
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
        operation: u64,
        source: Option<AppliedAudioSource>,
        playing: bool,
        keyboard_epoch: u64,
        faulted: bool,
        consumed: u64,
    ) {
        if operation & 1 == 0 || self.gate() != operation || consumed < self.cut.load(SeqCst) {
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
        self.playing.store(playing, SeqCst);
        self.accepted.store(
            !faulted && (!playing || self.allow_playing.load(SeqCst)) && source == Some(expected),
            SeqCst,
        );
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
    pub(super) fn retire_stopped_edit_on_control(&self) {
        self.sampler_edit.retire_abort_on_control();
        if !self.sampler_edit.active() {
            self.publication_pending.release_native_epoch();
        }
    }
    /// Scalar transport exclusion during a stopped edit, including accepted
    /// publication awaiting exact installation retirement. This grants no source custody.
    pub fn stopped_edit_pending(&self) -> bool {
        self.sampler_edit.active()
    }
}
impl PublicationControl {
    /// Fence a resident structural edit while allowing the existing runtime to keep playing.
    /// The ready witness selects live inheritance instead of a prepared stopped cursor.
    pub fn request_live_arrangement_edit_fence(
        &self,
        generation: u64,
        revision: u64,
    ) -> Result<StoppedEditFence, &'static str> {
        self.request_arrangement_edit_fence_impl(generation, revision, true)
    }
    /// Fence an ordinary resident runtime after its admitted command prefix. The
    /// callback, rather than UI telemetry, proves source identity and stopped state.
    pub fn request_arrangement_edit_fence(
        &self,
        generation: u64,
        revision: u64,
    ) -> Result<StoppedEditFence, &'static str> {
        self.request_arrangement_edit_fence_impl(generation, revision, false)
    }
    fn request_arrangement_edit_fence_impl(
        &self,
        generation: u64,
        revision: u64,
        allow_playing: bool,
    ) -> Result<StoppedEditFence, &'static str> {
        self.status.retire_stopped_edit_on_control();
        if self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
            || self.status.publication_pending.recording_active()
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
            allow_playing,
            false,
        )
    }
    /// Actual callback-observed frame, available only for this ready arrangement gate.
    pub fn arrangement_edit_position(&self, token: StoppedEditFence) -> Option<u64> {
        (self.stopped_edit_fence_state(token) == StoppedEditFenceState::Ready
            && self.status.sampler_edit.resident.load(Acquire))
        .then(|| self.status.sampler_edit.position.load(Acquire))
    }
    /// Playback state from the same callback witness as `arrangement_edit_position`.
    pub fn arrangement_edit_playing(&self, token: StoppedEditFence) -> Option<bool> {
        self.arrangement_edit_position(token)
            .map(|_| self.status.sampler_edit.playing.load(Acquire))
    }
    /// Transient launched loop from the exact ready witness, rather than saved markers.
    pub fn arrangement_edit_section_loop(&self, token: StoppedEditFence) -> Option<(u64, u64)> {
        self.arrangement_edit_position(token)?;
        let start = self.status.sampler_edit.loop_start.load(Acquire);
        let end = self.status.sampler_edit.loop_end.load(Acquire);
        (start < end).then_some((start, end))
    }
    pub fn request_sampler_edit_fence(
        &self,
        expected: AppliedAudioSource,
    ) -> Result<StoppedEditFence, &'static str> {
        self.status.retire_stopped_edit_on_control();
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
        self.status.retire_stopped_edit_on_control();
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
        // Pin the operation before selecting its runtime witness. An ACK from
        // one kind of edit must not acknowledge a later gate on the same source.
        let operation = self.status.sampler_edit.gate();
        if operation & 1 == 0 {
            return;
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if self.status.sampler_edit.native_epoch.load(Acquire) {
            let source = self.current_native_epoch_fence_source();
            let playing = match self.active.as_ref() {
                Some(ActivePublication::Eligible(active)) => active
                    .runtime()
                    .and_then(|runtime| runtime.status())
                    .map(|status| status.playing),
                Some(ActivePublication::Legacy(active)) => Some(active.playback.playing()),
                _ => None,
            };
            self.status.sampler_edit.acknowledge(
                operation,
                source,
                playing.unwrap_or(true),
                0,
                source.is_none()
                    || playing.is_none()
                    || self.status.closing.load(Acquire)
                    || self.status.device_fault.load(Acquire)
                    || self.status.render_fault.load(Acquire),
                self.status.consumed_command.load(Acquire),
            );
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
            if self.status.sampler_edit.ack.load(SeqCst) != operation {
                let region = active.and_then(|a| a.playback.section_loop());
                self.status
                    .sampler_edit
                    .loop_start
                    .store(region.map_or(0, |r| r.start()), SeqCst);
                self.status
                    .sampler_edit
                    .loop_end
                    .store(region.map_or(0, |r| r.end()), SeqCst);
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
                operation,
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
        let status = active.and_then(|a| a.runtime()).and_then(|r| {
            if self.status.sampler_edit.native_epoch.load(Acquire) {
                // Native effects need the real runtime Stop witness; they do
                // not require a sampler source collection to exist.
                r.status().map(|status| (status, 0))
            } else {
                r.current_sampler_edit_status()
            }
        });
        self.status.sampler_edit.acknowledge(
            operation,
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
            transport_epoch: 1,
            action: PlaybackAction::Transport(PlaybackCommand::Play),
        }
    }
    #[test]
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn aborted_native_gate_releases_recording_and_publication_admission_on_control() {
        let (_control, callback) = channel(48_000).unwrap();
        let status = &callback.status;
        assert!(status.publication_pending.reserve_native_epoch());
        let fence = &status.sampler_edit;
        let token = fence.begin_source(source(), 0, false, false, true).unwrap();
        fence.release(token);
        status.retire_stopped_edit_on_control();
        assert!(!status.publication_pending.reserve_publication());
        fence.acknowledge(token.operation, Some(source()), false, 0, false, 0);
        status.retire_stopped_edit_on_control();
        assert!(status.publication_pending.reserve_midi());
        assert!(!status.publication_pending.reserve_native_epoch());
        status.publication_pending.midi_finished();
        let ordinary = fence.begin(source(), 0).unwrap();
        assert!(status.publication_pending.reserve_publication());
        status.publication_pending.store(false, Release);
        fence.release(ordinary);
    }
    #[test]
    fn earlier_witness_cannot_acknowledge_a_later_gate_on_the_same_source() {
        let fence = Fence::default();
        let first = fence.begin(source(), 0).unwrap();
        fence.acknowledge(first.operation, Some(source()), false, 0, false, 0);
        fence.release(first);
        let next = fence.begin_source(source(), 0, false, false, true).unwrap();
        fence.acknowledge(first.operation, Some(source()), false, 0, false, 0);
        assert_eq!(fence.state(next), StoppedEditFenceState::Pending);
        fence.acknowledge(next.operation, Some(source()), false, 0, false, 0);
        assert_eq!(fence.native_epoch_context(next), Some(source().context));
    }
    #[test]
    fn earlier_play_is_consumed_before_stopped_ack_and_later_play_never_replays() {
        let fence = Fence::default();
        let older = play(1, fence.gate());
        let token = fence.begin(source(), 1).unwrap();
        assert!(fence.permits(older));
        // A request can arrive after this callback took its consumption cut.
        // It must wait for the following callback to consume that earlier Play.
        fence.acknowledge(fence.gate(), Some(source()), false, 9, false, 0);
        assert_eq!(fence.state(token), StoppedEditFenceState::Pending);
        fence.acknowledge(fence.gate(), Some(source()), true, 9, false, 1);
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
        fence.acknowledge(fence.gate(), Some(source()), false, 9, false, 0);
        assert_eq!(fence.state(token), StoppedEditFenceState::Ready);
        assert!(fence.permits_keyboard(9, 9));
        assert!(!fence.permits_keyboard(10, 9));
        fence.release(token);
        assert!(fence.permits_keyboard(10, 9));
        let next = fence.begin(source(), 0).unwrap();
        let mut changed = source();
        changed.audio_revision += 1;
        fence.acknowledge(fence.gate(), Some(changed), false, 9, false, 0);
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
        fence.acknowledge(fence.gate(), Some(source()), true, 0, false, 0);
        fence.retire_abort_on_control();
        assert!(fence.active());
        fence.acknowledge(fence.gate(), Some(source()), true, 0, false, 1);
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
        fence.acknowledge(fence.gate(), Some(source()), false, 0, true, 0);
        assert_eq!(fence.state(token), StoppedEditFenceState::Refused);
        fence.release(token);
        fence.serial.store(u64::MAX - 1, SeqCst);
        assert!(fence.begin(source(), 0).is_err());
        assert_eq!(fence.gate(), u64::MAX - 1);
    }
}
