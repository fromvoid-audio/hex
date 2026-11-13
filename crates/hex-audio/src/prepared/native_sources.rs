//! Native source control entry points; actual owners stay inside CandidateRoot.
use super::*;
use crate::{
    NativeSourceDataReservation, NativeSourcePreparationProgress, NativeSourcePreparationReturn,
};
use hex_lattice::ProcessorInstanceIdentity;
use hex_lattice_contract::{ForeignProcessorBinding as Binding, NativeDeviceState};
use hex_plugin_host::native;
use hex_plugin_scan::macos::build_identity::InstanceBuild;

mod parameter_extension;

/// A copied callback observation correlated with a particular stopped fence.
/// It proves neither graph membership nor continued validity after that fence
/// changes, and grants no ownership or publication authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FencedNativeQuantum {
    source: AppliedAudioSource,
    fence: StoppedEditFence,
    quantum: crate::plugin_worker::CommittedQuantum,
}

impl FencedNativeQuantum {
    pub fn observation(&self) -> crate::plugin_worker::CommittedQuantum {
        self.quantum
    }
}

impl PublicationControl {
    fn stopped_native_quantum_fence_matches(
        &self,
        source: AppliedAudioSource,
        fence: StoppedEditFence,
    ) -> bool {
        self.native_custody
            && self
                .status
                .stopped_native_source_fence_matches(source, fence)
    }

    /// Read only while the exact stopped native fence remains ready. The
    /// observation may advance as the callback runs even while the graph is
    /// fenced; callers must independently prove graph membership and the
    /// lifetime of any history selected from it.
    pub fn try_fenced_native_quantum_on_control(
        &self,
        source: AppliedAudioSource,
        fence: StoppedEditFence,
        binding: Binding,
    ) -> Result<Option<FencedNativeQuantum>, &'static str> {
        if binding.root != source.output_identity
            || !self.stopped_native_quantum_fence_matches(source, fence)
        {
            return Err("native quantum fence is not current");
        }
        let quantum = self.try_native_source_committed_quantum_on_control(binding)?;
        if !self.stopped_native_quantum_fence_matches(source, fence) {
            return Err("native quantum fence is not current");
        }
        Ok(quantum.map(|quantum| FencedNativeQuantum {
            source,
            fence,
            quantum,
        }))
    }

    /// Recheck the correlated fence only; this does not assert the callback
    /// observation is still the most recent quantum.
    pub fn fenced_native_quantum_fence_is_ready(&self, checkpoint: FencedNativeQuantum) -> bool {
        self.stopped_native_quantum_fence_matches(checkpoint.source, checkpoint.fence)
    }

    /// Definite correspondence or a specific unavailable/busy observation.
    /// A control caller must not turn contention into a negative approval or
    /// use this observation as graph/native admission authority.
    pub fn try_native_source_approves_captured_state_on_control(
        &self,
        binding: Binding,
        state: &NativeDeviceState,
    ) -> Result<bool, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native captured-state output is closing");
        }
        self.candidate_root
            .try_native_source_approves_captured_state(binding, state)
    }
    /// Mark the exact original worker's completed capture as source-corresponding.
    /// This is not a processing-profile approval or graph publication permit.
    pub fn approve_native_sequenced_capture_on_control(
        &self,
        binding: Binding,
        fact: &crate::plugin_worker::NativeWorkerStateAfterParameter,
    ) -> Result<NativeDeviceState, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native captured-state output is closing");
        }
        let state = self
            .candidate_root
            .approve_native_sequenced_capture(binding, fact)?;
        if self.status.closing.load(Acquire) {
            return Err("native captured-state output is closing");
        }
        Ok(state)
    }

    pub(crate) fn approve_native_canonical_processing_on_control(
        &self,
        facts: &crate::plugin_effects::CapturedCanonicalBases,
    ) -> Result<NativeDeviceState, &'static str> {
        let matches = || facts.matches_output_on_control(self);
        if !self.native_custody || !matches() {
            return Err("native processing approval source changed");
        }
        let state = self
            .candidate_root
            .approve_native_canonical_processing(facts)?;
        if !matches() {
            return Err("native processing approval source changed");
        }
        Ok(state)
    }

    /// Processing-profile correspondence only; native graph admission and
    /// actual parameter/Main delivery remain separate owner checks.
    pub fn native_source_approves_processing_state_on_control(
        &self,
        binding: Binding,
        state: &NativeDeviceState,
    ) -> Option<bool> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return None;
        }
        self.candidate_root
            .native_source_approves_processing_state(binding, state)
    }
    pub(crate) fn approve_native_manual_base_on_control(
        &self,
        captured: &crate::plugin_effects::manual_base_capture::CompletedManualBase,
    ) -> Result<NativeDeviceState, &'static str> {
        if !self.native_custody
            || self.status.closing.load(Acquire)
            || !captured.matches_source(&self.status)
        {
            return Err("native manual base source changed");
        }
        let state = self.candidate_root.approve_native_manual_base(captured)?;
        if !captured.matches_source(&self.status) || self.status.closing.load(Acquire) {
            return Err("native manual base source changed");
        }
        Ok(state)
    }

    /// Read source correspondence only. This does not admit a graph, restore
    /// SDK state, or prove that the captured parameter reached final Main.
    pub fn native_source_approves_captured_state_on_control(
        &self,
        binding: Binding,
        state: &NativeDeviceState,
    ) -> Option<bool> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return None;
        }
        self.candidate_root
            .native_source_approves_captured_state(binding, state)
    }
}

pub(super) enum NativeEpochRuntimeTarget {
    Constructor(crate::candidate_custody::NativeEpochTarget),
    Ordinary(AppliedAudioSource),
}

impl Status {
    /// The ordinary source and epoch eligible for new live parameter input.
    /// Priority Stop invalidates admission before the callback consumes it.
    pub fn live_native_parameter_source(&self) -> Option<(AppliedAudioSource, u64)> {
        self.native_parameter_source(true)
    }

    /// Current ordinary source at the requested transport state. Idle uses the
    /// installed canonical graph observation. After ordinary graph return,
    /// canonical preparation instead requires its held restore fence. Root
    /// independently matches the exact native binding and state in either case.
    pub fn native_parameter_source(&self, playing: bool) -> Option<(AppliedAudioSource, u64)> {
        let epoch = self.transport_epoch.load(Acquire);
        let source = if playing {
            self.native_effect_source()?
        } else {
            self.recording_source()?
        };
        (self.playing.load(Acquire) == playing
            && self.pending_stop_marker.load(Acquire) == 0
            && self.applied_audio_source().is_none()
            && !self.closing.load(Acquire)
            && self.transport_epoch.load(Acquire) == epoch)
            .then_some((source, epoch))
    }
}

impl PublicationControl {
    pub(crate) fn canonical_source_matches_on_control(
        &self,
        (source, epoch, playing): (AppliedAudioSource, u64, bool),
        fence: Option<StoppedEditFence>,
    ) -> bool {
        if !self.native_custody
            || self.output_identity() != Some(source.output_identity)
            || self.status.closing.load(Acquire)
        {
            return false;
        }
        match fence {
            None => self.status.native_parameter_source(playing) == Some((source, epoch)),
            Some(fence) => {
                !playing
                    && !self.status.playing.load(Acquire)
                    && self.status.pending_stop_marker.load(Acquire) == 0
                    && self.status.transport_epoch.load(Acquire) == epoch
                    && self.status.sampler_edit.ready_transport_epoch.load(Acquire) == epoch
                    && self.status.applied_revision.load(Acquire) == source.audio_revision
                    && self.status.sampler_edit.ordinary_native_epoch_active()
                    && self.status.sampler_edit.matches_source(fence, source)
                    && self.stopped_edit_fence_state(fence) == StoppedEditFenceState::Ready
            }
        }
    }

    pub(crate) fn native_fenced_base_capture_source(
        &self,
        source: AppliedAudioSource,
        fence: StoppedEditFence,
        binding: Binding,
        canonical: &NativeDeviceState,
    ) -> Result<(u32, u64, crate::plugin_effects::BaseCaptureSource), String> {
        let epoch = self.status.transport_epoch.load(Acquire);
        if binding.root != source.output_identity
            || !self.canonical_source_matches_on_control((source, epoch, false), Some(fence))
        {
            return Err("native base capture requires its held ordinary restore fence".into());
        }
        let original = self
            .candidate_root
            .native_source_for_base_capture(binding, canonical, false)?;
        Ok((self.rate, epoch, original))
    }

    pub(crate) fn native_base_capture_source(
        &self,
        expected: AppliedAudioSource,
        binding: Binding,
        canonical: &NativeDeviceState,
        playing: bool,
    ) -> Result<(u32, u64, crate::plugin_effects::BaseCaptureSource), String> {
        let (_, epoch) = self
            .status
            .native_parameter_source(playing)
            .filter(|(source, _)| *source == expected)
            .filter(|_| {
                self.native_custody && self.output_identity() == Some(expected.output_identity)
            })
            .filter(|_| binding.root == expected.output_identity)
            .ok_or("native base capture requires the current ordinary processor")?;
        let source = self
            .candidate_root
            .native_source_for_base_capture(binding, canonical, playing)?;
        Ok((self.rate, epoch, source))
    }

    pub(crate) fn native_saved_base_capture_source(
        &self,
        binding: Binding,
        canonical: &NativeDeviceState,
    ) -> Result<(u32, crate::plugin_effects::BaseCaptureSource), String> {
        if !self.native_custody
            || self.status.closing.load(Acquire)
            || self.output_identity() != Some(binding.root)
        {
            return Err("native saved base capture output differs or is closing".into());
        }
        // Save may follow a pending ordinary publication or stopped playback.
        // Its caller retains exact captured data and canonical source identity.
        // This private scratch never needs or claims a live callback witness.
        let source = self
            .candidate_root
            .native_source_for_base_capture(binding, canonical, false)?;
        Ok((self.rate, source))
    }

    /// Admit one actual PCM point and reserve its final Main delivery witness
    /// before the worker request becomes visible. Failed reservation admits no
    /// point. Both returned handles retain exact terminal custody through late
    /// close. Drain the witness before another observation or publication.
    /// Compensated pre-roll keeps this input queued until the accepted quantum
    /// starts on a known Arrangement origin.
    pub fn begin_native_source_parameter_change_with_delivery_on_control(
        &mut self,
        source: AppliedAudioSource,
        binding: Binding,
        id: u32,
        normalized: f64,
    ) -> Result<
        (
            crate::plugin_worker::NativeWorkerParameterChange,
            crate::foreign_parameter_delivery::Target,
        ),
        &'static str,
    > {
        self.require_parameter_delivery_source(source, binding)?;
        let status = &self.status;
        let delivery = self
            .foreign_parameter_delivery
            .as_mut()
            .ok_or("vst3 delivery reader is retired")?;
        self.candidate_root.native_parameter_change_with_before(
            binding,
            id,
            normalized,
            |request| {
                let (binding, operation, parameter) = request
                    .delivery_identity()
                    .ok_or("vst3 delivery requires a PCM parameter operation")?;
                delivery.begin(status, source, binding, operation, parameter)
            },
        )
    }

    /// Atomically reserve final Main delivery for a held target's Begin/Change
    /// or its exclusive Release. The release receipt carries the actual origin
    /// of the first successful SDK quantum with authored delivery restored.
    /// Observed input remains queued during compensated pre-roll until that
    /// quantum starts on a known Arrangement origin. Stop and queued withdrawal
    /// retain their ordinary independent terminal paths.
    pub fn native_source_parameter_hold_with_delivery_on_control(
        &mut self,
        source: AppliedAudioSource,
        binding: Binding,
        command: crate::plugin_worker::NativeParameterHold<'_>,
    ) -> Result<
        (
            crate::plugin_worker::NativeWorkerParameterChange,
            crate::foreign_parameter_delivery::Target,
        ),
        &'static str,
    > {
        self.require_parameter_delivery_source(source, binding)?;
        let status = &self.status;
        let delivery = self
            .foreign_parameter_delivery
            .as_mut()
            .ok_or("vst3 delivery reader is retired")?;
        self.candidate_root
            .native_parameter_hold_with_before(binding, command, |request| {
                let (binding, operation, parameter) = request
                    .delivery_identity()
                    .ok_or("vst3 delivery requires a PCM parameter operation")?;
                delivery.begin(status, source, binding, operation, parameter)
            })
    }

    fn require_parameter_delivery_source(
        &self,
        source: AppliedAudioSource,
        binding: Binding,
    ) -> Result<(), &'static str> {
        if self.foreign_parameter_delivery.is_none() {
            return Err("vst3 delivery reader is retired");
        }
        if !self.native_custody
            || self.status.closing.load(Acquire)
            || self.output_identity() != Some(source.output_identity)
            || binding.root != source.output_identity
            || self.native_source_endpoint_installed(binding) != Some(true)
        {
            return Err("vst3 delivery source differs or is closing");
        }
        Ok(())
    }

    /// Passive observation of a verified playing worker. This does not
    /// authorize a value change, retain the Root, or consume controller edits.
    pub fn arm_native_source_editor_delivery_on_control(
        &mut self,
        source: AppliedAudioSource,
        binding: Binding,
    ) -> Result<crate::foreign_parameter_delivery::editor::Target, &'static str> {
        self.require_parameter_delivery_source(source, binding)?;
        self.foreign_editor_delivery
            .as_mut()
            .ok_or("vst3 editor delivery reader is retired")?
            .begin(&self.status, source, binding, None, None, |scope| {
                self.candidate_root
                    .arm_native_editor_pcm(binding, scope, None)
            })
    }

    /// Arm one exclusive original SDK editor target for compensated delivery.
    /// While live, the existing native parameter input reservation excludes
    /// another host gesture from taking the same output.
    pub fn arm_native_source_editor_touch_delivery_on_control(
        &mut self,
        source: AppliedAudioSource,
        transport_epoch: u64,
        binding: Binding,
        touch: crate::foreign_parameter_delivery::editor::TouchPlan,
    ) -> Result<crate::foreign_parameter_delivery::editor::Target, &'static str> {
        self.require_parameter_delivery_source(source, binding)?;
        self.foreign_editor_delivery
            .as_mut()
            .ok_or("vst3 editor delivery reader is retired")?
            .begin(
                &self.status,
                source,
                binding,
                Some(transport_epoch),
                Some(Arc::clone(&self.status)),
                |scope| {
                    self.candidate_root.arm_native_editor_pcm(
                        binding,
                        scope,
                        Some(
                            crate::foreign_parameter_delivery::editor::TouchAdmission::Single(
                                touch,
                            ),
                        ),
                    )
                },
            )
    }

    /// Arm the exact prepared group on its original source. Targets must
    /// already have owned read/Base anchors, including unlaned Base values.
    pub fn arm_native_source_editor_group_delivery_on_control(
        &mut self,
        source: AppliedAudioSource,
        transport_epoch: u64,
        binding: Binding,
        targets: &[hex_plugin_host::protocol::EditorTouchTarget],
    ) -> Result<crate::foreign_parameter_delivery::editor::Target, &'static str> {
        self.require_parameter_delivery_source(source, binding)?;
        self.foreign_editor_delivery
            .as_mut()
            .ok_or("vst3 editor delivery reader is retired")?
            .begin(
                &self.status,
                source,
                binding,
                Some(transport_epoch),
                Some(Arc::clone(&self.status)),
                |scope| {
                    self.candidate_root.arm_native_editor_pcm(
                        binding,
                        scope,
                        Some(
                            crate::foreign_parameter_delivery::editor::TouchAdmission::Group(
                                targets,
                            ),
                        ),
                    )
                },
            )
    }

    pub fn cancel_native_source_editor_delivery_on_control(
        &mut self,
        target: crate::foreign_parameter_delivery::editor::Target,
    ) -> bool {
        self.foreign_editor_delivery
            .as_ref()
            .is_some_and(|delivery| delivery.cancel(target))
    }

    pub fn try_native_source_editor_delivery_on_control(
        &mut self,
    ) -> Option<crate::foreign_parameter_delivery::editor::Receipt> {
        self.foreign_editor_delivery.as_mut()?.try_receipt()
    }

    pub fn native_source_editor_confirmed_sequence_on_control(
        &self,
        target: crate::foreign_parameter_delivery::editor::Target,
    ) -> Option<u64> {
        self.foreign_editor_delivery
            .as_ref()?
            .confirmed_sequence(target)
    }

    /// Detach the exact editor scope's observation before output relinquish.
    /// This publication cannot admit another editor scope after transfer.
    pub fn retire_native_source_editor_delivery_on_control(
        &mut self,
    ) -> Option<crate::foreign_parameter_delivery::editor::Retired> {
        self.foreign_editor_delivery.as_ref()?.target()?;
        self.foreign_editor_delivery.take()?.retire()
    }

    pub fn cancel_native_source_parameter_delivery_on_control(
        &mut self,
        target: crate::foreign_parameter_delivery::Target,
    ) -> bool {
        self.foreign_parameter_delivery
            .as_mut()
            .is_some_and(|delivery| delivery.cancel(target))
    }

    pub fn try_native_source_parameter_delivery_on_control(
        &mut self,
    ) -> Option<crate::foreign_parameter_delivery::Receipt> {
        self.foreign_parameter_delivery
            .as_mut()?
            .try_receipt(&self.status)
    }

    /// Detach the one outstanding witness before relinquishing this output.
    /// New confirmed inputs on this publication are permanently refused. The
    /// non-Clone reader retains bounded Copy receipt channels and status only,
    /// never Root, SDK workers, PCM or a native output lease. Its poll/drop and
    /// final channel destruction belong on control, after callback activity.
    pub fn retire_native_source_parameter_delivery_on_control(
        &mut self,
    ) -> Option<crate::foreign_parameter_delivery::Retired> {
        self.foreign_parameter_delivery.as_ref()?.target()?;
        let delivery = self.foreign_parameter_delivery.take()?;
        delivery.retire(Arc::clone(&self.status))
    }

    /// Queue a live hold on the selected original worker. A begin/change
    /// requires the exact applied ordinary source and a running transport.
    /// Release stays with the original worker even if its editor revision
    /// changed or playback is paused. A release queued while paused is
    /// consumed before authored delivery in the next actual PCM quantum.
    /// A returned token keeps its ordinary progress/drain custody
    /// if the output starts closing after admission.
    pub fn native_source_parameter_hold_on_control(
        &mut self,
        expected: AppliedAudioSource,
        binding: Binding,
        command: crate::plugin_worker::NativeParameterHold<'_>,
    ) -> Result<crate::plugin_worker::NativeWorkerParameterChange, &'static str> {
        use crate::plugin_worker::NativeParameterHold;
        if !self.native_custody
            || self.status.closing.load(Acquire)
            || self.output_identity() != Some(expected.output_identity)
            || binding.root != expected.output_identity
        {
            return Err("native parameter hold output differs or is closing");
        }
        if !matches!(command, NativeParameterHold::Release { .. }) {
            if !self.status.playing.load(Acquire) {
                return Err("native parameter hold requires running playback");
            }
            if self
                .status
                .live_native_parameter_source()
                .map(|(source, _)| source)
                != Some(expected)
                || self.native_source_endpoint_installed(binding) != Some(true)
            {
                return Err("native parameter hold source is not applied");
            }
        }
        self.candidate_root.native_parameter_hold(binding, command)
    }

    /// Queue one validated raw SDK parameter input on the original live source.
    /// While playing, an actual PCM quantum consumes it. While idle, the same
    /// serialized source flushes the point and acknowledges controller readback
    /// without advancing PCM. After revocation, admission observes its healthy
    /// worker join and the exact stopped-input drain performs that SDK exchange.
    /// Validate current metadata/writability at the caller. Once admitted, the
    /// caller receives the exact operation token even if output closure races
    /// admission. Retain it through its actual terminal or original worker
    /// release; a returned token does not establish continued source liveness.
    /// Processing progress below grants no canonical/state installation.
    pub fn begin_native_source_parameter_change_on_control(
        &mut self,
        binding: Binding,
        id: u32,
        normalized: f64,
    ) -> Result<crate::plugin_worker::NativeWorkerParameterChange, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native parameter output is closing");
        }
        self.candidate_root.begin_native_parameter_change(
            binding,
            id,
            normalized,
            !self.status.playing.load(Acquire),
        )
    }
    /// True only if the exact queued input was withdrawn before processing.
    /// An in-flight operation must reach its real terminal; this does not roll
    /// back third-party DSP or acknowledge canonical gesture cancellation.
    pub fn withdraw_native_source_parameter_change_on_control(
        &mut self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerParameterChange,
    ) -> Result<bool, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native parameter output is closing");
        }
        let result = self
            .candidate_root
            .withdraw_native_parameter_change(binding, request)?;
        if self.status.closing.load(Acquire) {
            return Err("native parameter output is closing");
        }
        Ok(result)
    }
    /// Actual SDK input exchange progress, not controller readback, final host
    /// delivery or canonical authority. Exact source/token/context checks and
    /// captured-state validation remain required before a project/history edit.
    pub fn poll_native_source_parameter_change_on_control(
        &mut self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerParameterChange,
    ) -> Result<crate::plugin_worker::NativeWorkerParameterProgress, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native parameter output is closing");
        }
        let result = self
            .candidate_root
            .poll_native_parameter_change(binding, request)?;
        if self.status.closing.load(Acquire) {
            return Err("native parameter output is closing");
        }
        Ok(result)
    }
    /// Consume an exact stopped input's real SDK terminal after the original
    /// worker joins. Pending never fabricates a result; Processed does not
    /// imply callback delivery or canonical acceptance. Root must drain before
    /// quiescing this source, and an old token cannot address a new epoch.
    pub fn drain_stopped_native_source_parameter_change_on_control(
        &mut self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerParameterChange,
    ) -> Result<crate::plugin_worker::NativeWorkerParameterProgress, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native parameter output is closing");
        }
        let result = self
            .candidate_root
            .drain_stopped_native_parameter_change(binding, request)?;
        if self.status.closing.load(Acquire) {
            return Err("native parameter output is closing");
        }
        Ok(result)
    }
    /// Query actual SDK controller metadata through the original Root effect
    /// or loan worker. Busy/preparing/quiescent/closing sources refuse without
    /// replacing them. Normally stopped transport uses its joined original
    /// worker. An admitted token is returned even if close races admission;
    /// the caller retains it until its exact terminal or worker release. A
    /// token does not imply that the source remains open. Neither admission nor
    /// copied metadata grants edit authority.
    pub fn begin_native_source_controller_query_on_control(
        &mut self,
        binding: Binding,
        query: impl Into<hex_plugin_host::protocol::ControllerRequest>,
    ) -> Result<crate::plugin_worker::NativeWorkerControllerQuery, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native controller output is closing");
        }
        self.candidate_root
            .begin_native_controller_query(binding, query.into())
    }
    /// Consume only the exact source/worker query terminal. Recheck the owning
    /// document/device context before using these copied observations in UI.
    /// Source revocation wins over a late result; close suppresses delivery.
    pub fn poll_native_source_controller_query_on_control(
        &mut self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerControllerQuery,
    ) -> Result<crate::plugin_worker::NativeWorkerControllerProgress, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native controller output is closing");
        }
        let result = self
            .candidate_root
            .poll_native_controller_query(binding, request)?;
        if self.status.closing.load(Acquire) {
            return Err("native controller output is closing");
        }
        Ok(result)
    }
    /// Request/read ordered native controller callbacks through the actual
    /// original effect or loan control. None is pending; Some consumes at most
    /// eight receipts. Call regularly while observing an editor: bounded full
    /// queues backpressure, never coalesce gesture boundaries. Wrong/closed/
    /// revoked sources refuse. Native receipts grant no canonical automation or
    /// future-liveness authority; source terminal cancels pending observations.
    pub fn poll_native_source_parameter_edits_on_control(
        &mut self,
        binding: Binding,
    ) -> Result<Option<hex_plugin_host::protocol::ParameterEdits>, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native edit output is closing");
        }
        let result = self.candidate_root.native_source_parameter_edits(binding)?;
        if self.status.closing.load(Acquire) {
            return Err("native edit output is closing");
        }
        Ok(result)
    }
    /// Observe the exact live worker's edit receipts and uncovered latency
    /// restart. Opt in only when the consumer can restore the stopped source.
    /// While the transport is idle, the same worker applies one pending batch
    /// without advancing its PCM prefix.
    pub fn poll_native_source_recovery_edits_on_control(
        &mut self,
        binding: Binding,
    ) -> Result<Option<hex_plugin_host::protocol::RecoveryEdits>, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native edit output is closing");
        }
        self.candidate_root
            .native_source_recovery_edits(binding, !self.status.playing.load(Acquire))
    }
    /// Inspect latest processor parameter values through the actual original
    /// Root-held effect or its unique loan control. Wrong/closed/revoked sources
    /// refuse; busy is a read refusal, not consumption or transport cancellation.
    /// Values and their worker prefix are processing observations, not authored
    /// edits, final callback delivery or graph/canonical installation authority.
    pub fn native_source_parameter_values_on_control(
        &self,
        binding: Binding,
    ) -> Result<crate::plugin_worker::NativeParameterValues, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native parameter output is closing");
        }
        let result = self
            .candidate_root
            .native_source_parameter_values(binding)?;
        if self.status.closing.load(Acquire) {
            return Err("native parameter output is closing");
        }
        Ok(result)
    }
    /// Admit one capture through the original Root-held effect/control,
    /// including an active callback loan or normally ended transport. Stopped
    /// capture requires the original worker's joined healthy terminal and leaves
    /// transport revoked. No graph/endpoint is returned or
    /// replaced. The caller owns/accounting for its correlation token and must
    /// retire it on the exact terminal, including source close/refusal.
    pub fn begin_native_source_state_capture_on_control(
        &mut self,
        binding: Binding,
    ) -> Result<crate::plugin_worker::NativeWorkerCapture, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native capture output is closing");
        }
        self.candidate_root.begin_live_native_capture(binding)
    }
    /// Current state is copied data only, never canonical/source installation.
    /// Root binding/token checks do not prove the latest document/editor or
    /// applied graph context when a source is reused. Recheck that owning
    /// context before using data in a save; preserve confirmed state on error.
    pub fn poll_native_source_state_capture_on_control(
        &mut self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerCapture,
    ) -> Result<crate::plugin_worker::NativeWorkerCaptureProgress, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native capture output is closing");
        }
        let result = self
            .candidate_root
            .poll_live_native_capture(binding, request)?;
        if self.status.closing.load(Acquire) {
            return Err("native capture output is closing");
        }
        Ok(result)
    }
    /// Admit a capture ordered after the exact original worker's processed
    /// parameter input. This fact does not grant graph or canonical authority.
    pub fn begin_native_capture_after_parameter_on_control(
        &mut self,
        binding: Binding,
        input: &crate::plugin_worker::NativeWorkerParameterChange,
    ) -> Result<crate::plugin_worker::NativeWorkerCapture, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native capture output is closing");
        }
        self.candidate_root
            .begin_capture_after_parameter(binding, input)
    }
    pub fn poll_native_capture_after_parameter_on_control(
        &mut self,
        binding: Binding,
        request: &crate::plugin_worker::NativeWorkerCapture,
    ) -> Result<crate::plugin_worker::NativeWorkerSequencedCaptureProgress, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native capture output is closing");
        }
        let result = self
            .candidate_root
            .poll_capture_after_parameter(binding, request)?;
        if self.status.closing.load(Acquire) {
            return Err("native capture output is closing");
        }
        Ok(result)
    }
    pub fn native_sequenced_capture_is_current_on_control(
        &mut self,
        binding: Binding,
        fact: &crate::plugin_worker::NativeWorkerStateAfterParameter,
    ) -> Result<bool, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native capture output is closing");
        }
        let result = self
            .candidate_root
            .sequenced_capture_is_current(binding, fact)?;
        if self.status.closing.load(Acquire) {
            return Err("native capture output is closing");
        }
        Ok(result)
    }
    /// Observe the original endpoint of an exact processed input and capture.
    /// This weak control-owned observation can become stale immediately; it
    /// does not admit a candidate, identify its state, or authorize a handoff.
    /// Return and destroy it on control, outside the audio callback.
    pub fn observe_native_sequenced_capture_input_on_control(
        &mut self,
        binding: Binding,
        fact: &crate::plugin_worker::NativeWorkerStateAfterParameter,
    ) -> Result<Option<hex_plugin_host::transport::ParameterInputSource>, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native capture output is closing");
        }
        let result = self
            .candidate_root
            .observe_sequenced_capture_input(binding, fact)?;
        if self.status.closing.load(Acquire) {
            return Err("native capture output is closing");
        }
        Ok(result)
    }
    /// Nondefault deterministic fixture control: original worker's observed
    /// completion prefix. Neither queueing, readiness nor custody authority.
    #[cfg(feature = "allocation-probe")]
    pub fn native_source_completed_blocks(&self, binding: Binding) -> Option<u64> {
        self.candidate_root.native_source_completed_blocks(binding)
    }
    /// Nonblocking actual source-pair observation: (endpoint loan outstanding,
    /// transport live). A queued loan stays outstanding until actual control
    /// ingestion; a returned/revoked pair is not made live by querying it.
    /// None includes contention/stale/retiring/closed sources. This is not
    /// endpoint installation, graph admission, or callback readiness authority.
    pub fn native_source_endpoint_status(&self, binding: Binding) -> Option<(bool, bool)> {
        if self.status.closing.load(Acquire) {
            return None;
        }
        self.candidate_root.native_source_endpoint_status(binding)
    }
    /// Read the exact accepted endpoint's terminal diagnostic once, without
    /// joining its worker or exposing any callback state. Contention can defer it.
    pub fn take_native_source_failure_diagnostic(&self, binding: Binding) -> Option<String> {
        if self.status.closing.load(Acquire) {
            return None;
        }
        self.candidate_root
            .take_native_source_failure_diagnostic(binding)
    }
    /// Nonblocking observation of this original endpoint's actual callback
    /// installation, independent of queue admission. None includes a stale,
    /// retiring/closed source or control contention. True grants neither graph
    /// application nor transport readiness: normal publication still validates
    /// the actual source/pair, including any later Stop or refusal.
    pub fn native_source_endpoint_installed(&self, binding: Binding) -> Option<bool> {
        if self.status.closing.load(Acquire) {
            return None;
        }
        self.candidate_root
            .native_source_endpoint_installed(binding)
    }
    /// Exclude transport/publication while the callback acknowledges the exact
    /// current native runtime after the admitted command prefix. This witness
    /// grants no source ownership and does not acknowledge graph/endpoint return.
    pub fn request_native_epoch_fence(
        &self,
        expected: AppliedAudioSource,
    ) -> Result<StoppedEditFence, &'static str> {
        self.request_native_epoch_source_fence(expected, false)
    }
    /// Fence the exact current ordinary graph while renewing original loaded
    /// sources, including sources removed by Undo whose tail has already ended.
    /// Stop may remove the last foreign runtime before acknowledgement; the
    /// actual ordinary graph/source must still match. Constructor/sampler and
    /// verified-streaming paths retain their existing fences. Readiness grants
    /// no return or renewal: original graph and endpoint ingestion remain required.
    pub fn request_ordinary_native_epoch_fence(
        &self,
        expected: AppliedAudioSource,
    ) -> Result<StoppedEditFence, &'static str> {
        self.request_native_epoch_source_fence(expected, true)
    }
    fn request_native_epoch_source_fence(
        &self,
        expected: AppliedAudioSource,
        ordinary: bool,
    ) -> Result<StoppedEditFence, &'static str> {
        self.status.retire_stopped_edit_on_control();
        // Compensated stream windows currently reject the native bridge's
        // positive latency. Do not admit renewal in a mode that cannot install
        // its compensated successor; that window contract must be extended first.
        if self.status.verified_streaming_mode.load(Acquire) {
            return Err("native epoch renewal requires resident playback");
        }
        if !self.native_custody
            || self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
            || self.status.publication_pending.recording_active()
            || if ordinary {
                self.status.applied_audio_source().is_some()
                    || !self.candidate_root.ordinary_streams_configured()
                    || self.status.recording_source() != Some(expected)
            } else {
                self.status.native_effect_source() != Some(expected)
            }
        {
            return Err("native epoch output is not current or publication is pending");
        }
        if !self.status.publication_pending.reserve_native_epoch() {
            return Err("native epoch publication or recording is pending");
        }
        let result = if ordinary {
            self.status.sampler_edit.begin_ordinary_native_epoch_source(
                expected,
                self.status.admitted_ticket.load(Acquire),
            )
        } else {
            self.status.sampler_edit.begin_source(
                expected,
                self.status.admitted_ticket.load(Acquire),
                false,
                false,
                true,
            )
        };
        match result {
            Ok(token) => Ok(token),
            Err(reason) => {
                self.status.publication_pending.release_native_epoch();
                Err(reason)
            }
        }
    }

    /// Queue return of exactly this stopped runtime and its original native
    /// endpoints through the existing carriers. Keep the stopped fence held
    /// through renewal/successor installation. Enqueue success is not return:
    /// source renewal still requires control ingestion and all graph refs gone.
    pub fn return_stopped_native_epoch_runtime(
        &mut self,
        ticket: &crate::AudioCandidateTicket,
        fence: StoppedEditFence,
    ) -> Result<(), &'static str> {
        if !self.native_custody
            || self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
            || self
                .status
                .sampler_edit
                .native_epoch_context(fence)
                .is_none()
            || self.stopped_edit_fence_state(fence) != StoppedEditFenceState::Ready
        {
            return Err("native epoch requires an observed stopped source fence");
        }
        let target = ticket
            .native_epoch_target(self.candidate_root.root_id())
            .ok_or("native epoch runtime belongs to another output")?;
        let chunk = self
            .publications
            .write_chunk_uninit(1)
            .map_err(|_| "native epoch publication is busy")?;
        chunk.fill_from_iter(std::iter::once(QueuedPublication::NativeEpoch {
            target: NativeEpochRuntimeTarget::Constructor(target),
            fence,
        }));
        Ok(())
    }
    /// Return exactly the callback-observed ordinary native graph under its
    /// ready fence. Success queues only a scalar request; Root still requires
    /// actual graph destruction and original endpoint ingestion before renewal.
    pub fn return_stopped_ordinary_native_epoch_runtime(
        &mut self,
        source: AppliedAudioSource,
        fence: StoppedEditFence,
    ) -> Result<(), &'static str> {
        // The same still-held operation may need another fresh epoch when Stop
        // revokes a newly prepared cohort before successor installation. There
        // is no current graph in that case, but actual endpoint return is still
        // required; the callback independently proves this vacant frontier.
        let vacant = self.status.native_effect_source().is_none()
            && self.status.recording_source().is_none()
            && self.status.applied_revision.load(Acquire) == source.audio_revision;
        if !self.native_custody
            || !self.candidate_root.ordinary_streams_configured()
            || self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
            || (self.status.native_effect_source() != Some(source)
                && !(self.status.sampler_edit.ordinary_native_epoch_active()
                    && self.status.applied_audio_source().is_none()
                    && self.status.recording_source() == Some(source))
                && !vacant)
            || source.output_identity != self.candidate_root.root_id()
            || self.status.sampler_edit.native_epoch_context(fence) != Some(source.context)
        {
            return Err("ordinary native epoch requires its observed stopped source");
        }
        let chunk = self
            .publications
            .write_chunk_uninit(1)
            .map_err(|_| "native epoch publication is busy")?;
        chunk.fill_from_iter(std::iter::once(QueuedPublication::NativeEpoch {
            target: NativeEpochRuntimeTarget::Ordinary(source),
            fence,
        }));
        Ok(())
    }
    /// Renew custody of this exact loaded source after its original callback
    /// endpoint and every old graph reference returned. The new binding is a
    /// pending operation: query facts, complete with actual presentation, then
    /// construct/loan fresh paired endpoints before proposing a successor graph.
    pub fn quiesce_native_source_preparation(
        &mut self,
        previous: Binding,
        graph: u64,
    ) -> Result<Binding, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native epoch output is closing");
        }
        self.candidate_root
            .quiesce_native_source_preparation(previous, graph)
    }
    /// Reactivate this exact renewed reservation's stopped component and
    /// replace its facts before planning presentation and completing the epoch.
    /// May reset effect history. Some returns fresh facts, None retains a
    /// failed native owner for disposal, and Err refuses admission or close.
    /// The caller must rebuild compensation before publishing the successor.
    /// This operation runs only on control after actual graph and endpoint return.
    pub fn reactivate_native_source_on_control(
        &mut self,
        binding: Binding,
    ) -> Result<Option<hex_plugin_host::protocol::Facts>, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native reactivation output is closing");
        }
        let result = self.candidate_root.reactivate_native_source(binding)?;
        if self.status.closing.load(Acquire) {
            return Err("native reactivation output is closing");
        }
        Ok(result)
    }
    /// Settle latency restarts on this exact stopped source reservation
    /// before completing its new epoch and preparing compensation.
    pub fn settle_native_latency_on_control(
        &mut self,
        binding: Binding,
    ) -> Result<Option<hex_plugin_host::protocol::Facts>, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native reactivation output is closing");
        }
        self.candidate_root.settle_native_latency(binding)
    }
    /// Capture the original quiescent owner after actual graph/endpoint return.
    /// `binding` names Root's renewed reservation; the helper still acknowledges
    /// its old physical binding until the normal completion/resume transaction.
    /// Some is fresh bounded data only; None means native failure/close retained
    /// for normal progress/error/disposal recovery. Err is synchronous refusal.
    /// Keep last confirmed state on either refusal. Caller owns/accounting for
    /// the copied envelope; this does not install it or grant publication.
    pub fn capture_native_source_state_on_control(
        &mut self,
        binding: Binding,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Option<NativeDeviceState>, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native capture output is closing");
        }
        self.candidate_root
            .capture_native_source_state(binding, is_cancelled)
    }
    /// Apply parameter-only changes to Root's actual quiescent source after
    /// graph and endpoint return. Binding names the renewed reservation; the
    /// helper still validates its old physical epoch until normal resume.
    /// Some is stopped DSP/controller readback, not a canonical edit or resume.
    /// None is native failure/close retained for ordinary disposal; Err refuses
    /// admission. Keep confirmed state on either refusal and recheck document
    /// context before consuming the result. Never call on the audio callback.
    pub fn flush_native_source_parameters_on_control(
        &mut self,
        binding: Binding,
        parameters: &hex_plugin_host::transport::Parameters,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Option<hex_plugin_host::protocol::FlushedParameters>, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native parameter output is closing");
        }
        let result = self.candidate_root.flush_native_source_parameters(
            binding,
            parameters,
            is_cancelled,
        )?;
        if self.status.closing.load(Acquire) {
            return Err("native parameter output is closing");
        }
        Ok(result)
    }
    /// Reserve fixed cells and per-cell variable DATA allowances before exposing
    /// the output. The whole pool's allowances remain charged, including vacant
    /// cells, so other admissions cannot spend that capacity. This is not actual
    /// producer/resource sealing and cannot grant graph/publication eligibility.
    pub fn configure_native_source_preparation(
        &mut self,
        capacity: usize,
        limits: NativeSourceDataReservation,
    ) -> Result<(), &'static str> {
        if !self.native_custody
            || self.revision != 0
            || self.status.client_exposed.load(Acquire)
            || self.status.closing.load(Acquire)
        {
            return Err("native preparation requires a vacant native output");
        }
        if self.device_resources.is_some() {
            let graphs = self
                .publications
                .buffer()
                .capacity()
                .checked_add(RETIRED)
                .and_then(|n| n.checked_add(5))
                .ok_or("ordinary native graph capacity overflow")?;
            self.candidate_root
                .configure_ordinary_native_references(graphs)?;
        }
        self.candidate_root
            .configure_native_source_preparation(capacity, limits)
    }
    /// Check the existing native source reservation on control before staging
    /// a bounded cohort. Unavailable or contended custody refuses this check.
    /// Subsequent preparation must still pass each original admission.
    pub fn has_vacant_native_source_slots_on_control(&self, needed: usize) -> bool {
        self.native_custody
            && !self.status.closing.load(Acquire)
            && self.candidate_root.has_vacant_native_source_slots(needed)
    }
    /// Run source preparation on the existing control worker, never UI/audio.
    /// Success parks the actual owner in Root even after refusal/close. The
    /// initial binding and cancellation pair are created by Root before spawn.
    /// Callers cancel by exact binding; no descriptor-owning token escapes.
    #[allow(clippy::too_many_arguments)] // Complete typed native request; no separate caller cancellation owner.
    pub fn begin_native_source_preparation(
        &mut self,
        identity: ProcessorInstanceIdentity,
        graph: u64,
        build: &InstanceBuild,
        source: native::Source,
        preparation: native::Preparation,
        restore: Option<NativeDeviceState>,
        trust: native::TrustUserCode,
    ) -> Result<Binding, &'static str> {
        if !self.native_custody
            || self.status.closing.load(Acquire)
            || preparation.sample_rate != f64::from(self.rate)
            || self.plugin_callback_quantum != Some(preparation.quantum)
        {
            return Err("native preparation output profile differs or is closing");
        }
        self.candidate_root.begin_native_source_preparation(
            identity,
            graph,
            build,
            source,
            preparation,
            restore,
            trust,
        )
    }
    /// Supply the presentation plan. The exact Pending becomes a Root-held Prepared or retained
    /// refusal. Ok acknowledges custody only; inspect progress/error separately.
    /// Run on the existing control worker. This does not publish a graph.
    pub fn complete_native_source_preparation(
        &mut self,
        binding: Binding,
        presentation: impl Into<hex_plugin_host::protocol::PresentationPlan>,
    ) -> Result<(), &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native preparation output is closing");
        }
        self.candidate_root
            .complete_native_source_preparation(binding, presentation)
    }
    /// Construct the actual copied-transport worker/callback pair inside Root.
    /// Ok acknowledges retained custody, including constructor refusal. Neither
    /// the pair nor its facts grant graph/publication admission yet.
    pub fn start_native_effect(&mut self, binding: Binding) -> Result<(), &'static str> {
        self.start_native_effect_with_authored_selection(binding, None)
    }

    /// Arm the saved targets on the actual source before transferring the
    /// prepared lease to its original worker. Existing Root refusal custody
    /// applies to both SDK metadata validation and worker construction.
    pub fn start_native_effect_with_authored_selection(
        &mut self,
        binding: Binding,
        authored: Option<&crate::plugin_effects::AuthoredSelection>,
    ) -> Result<(), &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native preparation output is closing");
        }
        self.candidate_root
            .start_native_effect_with_authored_selection(binding, authored)
    }
    /// Run pure graph compilation with definitions borrowed from these exact
    /// Root-owned effect pairs. This does not transfer endpoints or authorize
    /// publication. The visitor runs on the existing control worker.
    pub fn with_native_effect_definition<R>(
        &self,
        bindings: &[Binding],
        prepare: impl FnOnce(&crate::plugin_effects::NativeEffectDefinition<'_>) -> Result<R, String>,
    ) -> Result<R, String> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native effect output is closing".into());
        }
        self.candidate_root
            .with_native_effect_definition(bindings, prepare)
    }
    /// Borrow selected actual Root source definitions while preparing inside
    /// this same Root's reserved cell. One Work guard covers both sides; no
    /// nested acquisition, source extraction or independent lifecycle exists.
    pub fn prepare_audio_candidate_with_native_sources_on_control(
        &mut self,
        token: crate::AudioCandidatePreparationToken,
        bindings: &[Binding],
        prepare: impl FnOnce(
            &mut hex_engine::AudioCandidatePreparation,
            &hex_engine::AudioCandidateEffects<'_, '_>,
        ) -> Result<(), hex_engine::AudioStorageError>,
    ) -> Result<crate::AudioCandidatePreparationProgress, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native effect output is closing");
        }
        self.candidate_root
            .with_native_runtime_preparation(token, bindings, prepare)
    }
    /// Copy the original restored envelope, or actual initial capture for a
    /// fresh source. Read-only state is never Root/graph admission authority.
    pub fn native_source_prepared_state(&self, binding: Binding) -> Option<NativeDeviceState> {
        self.candidate_root.native_source_prepared_state(binding)
    }
    /// Copy bounded original source metadata on control. This borrows the exact
    /// constructed worker owner even while its endpoint is loaned; no native
    /// resource, state capture or permission is returned. New preparation still
    /// revalidates files and constructs an independent source through this Root.
    pub fn native_source_metadata(
        &self,
        binding: Binding,
    ) -> Result<crate::plugin_effects::EffectSourceMetadata, String> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native source control is closing".into());
        }
        self.candidate_root.native_source_metadata(binding)
    }
    /// Reserve a fresh graph-program key on the actual original source. This
    /// does not alter DSP, grant a new endpoint or authorize canonical state.
    /// Unused or concurrently cancelled reservations may leave monotonic gaps.
    pub fn reserve_native_source_program_revision_on_control(
        &self,
        binding: Binding,
    ) -> Result<std::num::NonZeroU64, String> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native source control is closing".into());
        }
        self.candidate_root
            .reserve_native_program_revision(binding)
            .map_err(str::to_owned)
    }
    /// Exact original worker pickup, not successful SDK processing or final
    /// Main delivery. A busy, revoked or replaced source has no observation.
    pub fn native_source_received_program_on_control(
        &self,
        binding: Binding,
    ) -> Option<crate::plugin_worker::ReceivedProgram> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return None;
        }
        self.candidate_root.native_source_received_program(binding)
    }
    /// Retryable observation on the exact original worker. An empty frontier
    /// requires either an unprogrammed pickup or an observed zero pickup
    /// prefix on the live pair. Unknown grants no graph or pruning authority.
    pub fn try_native_source_program_frontier_on_control(
        &self,
        binding: Binding,
    ) -> Result<hex_engine::ForeignProgramFrontier, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native automation output is closing");
        }
        self.candidate_root
            .try_native_source_program_frontier(binding)
    }
    /// Observe the last accepted quantum of the exact live loaned endpoint.
    /// None means no coherent callback observation; busy, stale, retired and
    /// closing sources are errors. This grants no graph or pruning authority.
    pub fn try_native_source_committed_quantum_on_control(
        &self,
        binding: Binding,
    ) -> Result<Option<crate::plugin_worker::CommittedQuantum>, &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native automation output is closing");
        }
        self.candidate_root
            .try_native_source_committed_quantum(binding)
    }
    /// Exact worker reply publication, still distinct from final Main delivery.
    pub fn native_source_processed_program_on_control(
        &self,
        binding: Binding,
    ) -> Option<crate::plugin_worker::ReceivedProgram> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return None;
        }
        self.candidate_root.native_source_processed_program(binding)
    }
    pub fn native_source_preparation_progress(
        &self,
        binding: Binding,
    ) -> Option<NativeSourcePreparationProgress> {
        self.candidate_root
            .native_source_preparation_progress(binding)
    }
    /// Record retirement of this exact source. Original graph references defer
    /// endpoint return and worker disposal; a true result needs no retry.
    /// False records no request: inspect progress and retry a still-live source
    /// when the control executor is busy. Unknown or terminal bindings refuse.
    pub fn retire_native_source_preparation(&mut self, binding: Binding) -> bool {
        self.candidate_root
            .retire_native_source_preparation(binding)
    }
    /// Observe the original construction/cleanup failure without taking its
    /// owner or treating an unresolved child as a recoverable return.
    pub fn native_source_preparation_error(&self, binding: Binding) -> Option<String> {
        self.candidate_root.native_source_preparation_error(binding)
    }
    /// Observe the same original diagnostic without confusing control contention
    /// with a successfully observed absence of error. Busy is nondestructive;
    /// stale bindings refuse. This read grants no source/terminal custody.
    pub fn try_native_source_preparation_error(
        &self,
        binding: Binding,
    ) -> Result<Option<String>, &'static str> {
        self.candidate_root
            .try_native_source_preparation_error(binding)
    }
    pub fn recover_native_source_preparation(
        &mut self,
        binding: Binding,
    ) -> Option<NativeSourcePreparationReturn> {
        self.candidate_root
            .recover_native_source_preparation(binding)
    }
}

impl PublicationControl {
    /// Queue the unique actual endpoint; the existing Root keeps its worker.
    /// Queue acceptance grants no graph, canonical edit or disposal authority.
    /// A full queue refuses before extracting the original constructor pair.
    pub fn publish_native_source_endpoint(&mut self, binding: Binding) -> Result<(), &'static str> {
        if !self.native_custody || self.status.closing.load(Acquire) {
            return Err("native preparation output is closing");
        }
        let chunk = self
            .publications
            .write_chunk_uninit(1)
            .map_err(|_| "native endpoint publication is busy")?;
        let endpoint = self.candidate_root.loan_native_endpoint(binding)?;
        chunk.fill_from_iter(std::iter::once(QueuedPublication::Native(endpoint)));
        Ok(())
    }
}
impl Callback {
    /// Borrow the installed original endpoint's input observation without
    /// waiting, allocation, or ownership transfer. This is not graph, source,
    /// transport, state, or publication authority; callers must validate each.
    pub fn reserve_native_parameter_input_observation(
        &self,
        binding: Binding,
        source: &hex_plugin_host::transport::ParameterInputSource,
    ) -> Option<hex_plugin_host::transport::ParameterInputFence<'_>> {
        if self.status.closing.load(Acquire) {
            return None;
        }
        self.native_sources
            .reserve_observed_parameter_input(binding, source)
    }

    #[cfg(feature = "allocation-probe")]
    pub(crate) fn native_root_for_control_fixture(
        &self,
    ) -> crate::candidate_custody::CandidateRoot {
        self.candidate_root.clone()
    }
    /// Read-only headless control query, outside rendering/allocation probes.
    /// This grants no readiness, endpoint loan, graph admission or native ACK.
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn native_workers_idle_on_control(&self) -> Option<bool> {
        self.native_sources
            .workers_idle_on_control(&self.candidate_root)
    }
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn native_worker_phase_for_test(&self) -> Option<usize> {
        self.native_sources.single_worker_phase_for_test()
    }
    /// Source-specific diagnostic only, outside callback measurement. Absence
    /// means healthy or unknown; it grants no readiness or source authority.
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn native_source_fault_for_test(
        &self,
        binding: Binding,
    ) -> Option<hex_plugin_host::transport::Fault> {
        self.native_sources.test_transport_fault(binding)
    }
    pub(super) fn current_native_epoch_fence_source(&self) -> Option<AppliedAudioSource> {
        if !self.status.sampler_edit.ordinary_native_epoch_active() {
            return self.current_native_epoch_source();
        }
        let active = self.active.as_ref().and_then(ActivePublication::legacy)?;
        active.source_context.map(|context| AppliedAudioSource {
            context,
            audio_revision: active.revision,
            output_identity: self.candidate_root.root_id(),
        })
    }
    pub(super) fn current_native_epoch_source(&self) -> Option<AppliedAudioSource> {
        match self.active.as_ref()? {
            ActivePublication::Eligible(active) => active.applied_audio_source(),
            ActivePublication::Legacy(active)
                if self.native_sources.ordinary_runtime_requires_sources() =>
            {
                active.source_context.map(|context| AppliedAudioSource {
                    context,
                    audio_revision: active.revision,
                    output_identity: self.candidate_root.root_id(),
                })
            }
            _ => None,
        }
    }
    pub(super) fn return_native_epoch_runtime(
        &mut self,
        target: NativeEpochRuntimeTarget,
        fence: StoppedEditFence,
    ) {
        let target = match target {
            NativeEpochRuntimeTarget::Constructor(target) => target,
            NativeEpochRuntimeTarget::Ordinary(source) => {
                self.return_ordinary_native_epoch_runtime(source, fence);
                return;
            }
        };
        let Some(ActivePublication::Eligible(active)) = self.active.as_ref() else {
            return;
        };
        let Some(source) = active.applied_audio_source() else {
            return;
        };
        if self
            .status
            .sampler_edit
            .native_epoch_context(fence)
            .is_none()
            || !active.matches_native_epoch_target(&target)
            || !self.status.sampler_edit.matches_source(fence, source)
            || active
                .runtime()
                .and_then(|runtime| runtime.status())
                .is_none_or(|s| s.playing)
            || !self.native_sources.has_endpoints()
        {
            return;
        }
        // begin_block reserved return capacity before popping this scalar
        // request. Only fixed moves occur; all original destruction is control.
        self.cancel_active_stream_action();
        if let Some(ActivePublication::Eligible(mut active)) = self.active.take() {
            active.clear_stream_roles(&mut self.domain_pages);
            self.native_sources.request_epoch_return();
            self.return_publication(ReturnedPublication::Eligible(
                active.return_stopped_native_epoch(),
            ));
            self.status.applied_source.publish(None);
            self.publish_active_routes(true);
            self.publish_transport();
        }
    }
    fn return_ordinary_native_epoch_runtime(
        &mut self,
        source: AppliedAudioSource,
        fence: StoppedEditFence,
    ) {
        if !self.status.sampler_edit.matches_source(fence, source)
            || self
                .status
                .sampler_edit
                .native_epoch_context(fence)
                .is_none()
            || !self.native_sources.has_endpoints()
        {
            return;
        }
        if self.active.is_none() {
            if self.status.applied_revision.load(Acquire) == source.audio_revision
                && !self.status.publication_pending.load(Acquire)
            {
                self.native_sources.refresh_runtime_requirements(None);
                self.native_sources.request_epoch_return();
            }
            return;
        }
        if self.current_native_epoch_fence_source() != Some(source)
            || self
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .is_none_or(|active| active.playback.playing())
        {
            return;
        }
        self.cancel_active_stream_action();
        if let Some(ActivePublication::Legacy(mut active)) = self.active.take() {
            if let Some(mut streams) = active.stream_view() {
                streams.clear_stream_roles(&mut self.domain_pages);
            }
            self.native_sources.refresh_runtime_requirements(None);
            self.native_sources.request_epoch_return();
            self.return_graph(active);
            self.status.applied_source.publish(None);
            self.publish_active_routes(true);
            self.publish_transport();
        }
    }
    pub(super) fn return_requested_native_sources(&mut self) {
        for index in 0..hex_lattice::MAX_STATEFUL_PROCESSORS {
            if self.held_retirement.is_some() || self.retired.is_full() {
                return;
            }
            let Some(endpoint) = self.native_sources.take_requested(index) else {
                continue;
            };
            self.return_publication(ReturnedPublication::Native(endpoint));
        }
    }
    pub(crate) fn has_native_source_endpoints(&self) -> bool {
        self.native_sources.has_endpoints()
    }
    pub(crate) fn extract_native_source_endpoints_on_control(
        &mut self,
        destination: &mut [Option<crate::candidate_custody::NativeEndpointEnvelope>;
                 hex_lattice::MAX_STATEFUL_PROCESSORS],
    ) -> bool {
        self.native_sources
            .extract(self.candidate_root.root_id(), destination)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod parameter_source_tests {
    use super::*;

    #[test]
    fn parameter_source_requires_exact_transport_mode_and_current_ordinary_graph() {
        let status = Status::default();
        let source = AppliedAudioSource {
            context: hex_engine::AudioSourceContext {
                project_generation: 3,
                editor_revision: 7,
            },
            audio_revision: 9,
            output_identity: 11,
        };
        status.recording_source.publish(Some(source));
        status.applied_revision.store(9, Release);
        status.transport_epoch.store(5, Release);
        assert_eq!(status.native_parameter_source(false), Some((source, 5)));
        assert_eq!(status.live_native_parameter_source(), None);
        status.pending_stop_marker.store(1, Release);
        assert_eq!(status.native_parameter_source(false), None);
        status.pending_stop_marker.store(0, Release);
        status.playing.store(true, Release);
        status.ordinary_native_revision.store(9, Release);
        assert_eq!(status.native_parameter_source(false), None);
        assert_eq!(status.live_native_parameter_source(), Some((source, 5)));
        status.closing.store(true, Release);
        assert_eq!(status.live_native_parameter_source(), None);
        status.closing.store(false, Release);
        status.playing.store(false, Release);
        status.applied_source.publish(Some(source));
        assert_eq!(status.native_parameter_source(false), None);
        status.applied_source.publish(None);
        status.applied_revision.store(10, Release);
        assert_eq!(status.native_parameter_source(false), None);
    }
}

#[cfg(test)]
mod retired_delivery_tests;
