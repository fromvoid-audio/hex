//! The original source's metadata-bound gesture admission. No parameter input
//! or canonical edit is inferred from this observation.
use super::*;
use hex_audio::plugin_worker::{
    NativeParameterPoll, NativeParameterTerminalReceipt, NativeSnapshotAction, NativeWorkerCapture,
    NativeWorkerParameterChange, NativeWorkerParameterProgress, NativeWorkerStateSnapshot,
    NativeWorkerStateSnapshotProgress,
};
use hex_audio::prepared::AudioSourceObservation;
use hex_session::device_gesture::{
    GestureKey, GestureRequest, GestureSnapshot, GestureTarget, PluginGesturePreparation,
    PluginParameterOrigin, PluginParameterValue,
};
use hex_session::service::{
    PluginControllerPageRequest, PluginControllerPageResult, PluginParameterGestureError as Error,
    PluginParameterGestureResult as ResultState, StalePluginGestureDisposition,
};

fn parameter_admission_busy(reason: &str) -> bool {
    matches!(
        reason,
        "native preparation is busy"
            | "native parameter mailbox is busy"
            | "native state approval is pending"
    )
}

#[derive(Clone, Copy)]
pub(super) struct Gesture {
    key: GestureKey,
    pub(super) origin: PluginParameterOrigin,
    request: PluginControllerPageRequest,
    admitted: bool,
    pub(super) binding: Binding,
    output: u64,
    pub(super) last_value: PluginParameterValue,
    last_sequence: u64,
}

pub(super) struct PendingPoint {
    snapshot: GestureSnapshot,
    value: PluginParameterValue,
    restore: bool,
    binding: Binding,
    output: u64,
    context: AudioSourceContext,
    token: Option<NativeWorkerParameterChange>,
    // Only held before native admission; dropped as soon as the token exists.
    waiting_state: Option<NativeDeviceState>,
    admission_error: Option<&'static str>,
    terminal: Option<NativeParameterTerminalReceipt>,
    current: bool,
    cancel_requested: bool,
}

/// One exact stopped point while its original source is fenced and renewed.
/// Neither a flush nor graph admission completes this gesture.
pub(super) struct StoppedPoint {
    pub(super) snapshot: GestureSnapshot,
    pub(super) value: PluginParameterValue,
    pub(super) restore: bool,
    pub(super) binding: Binding,
    pub(super) output: u64,
    pub(super) context: AudioSourceContext,
    pub(super) cancel_requested: bool,
    pub(super) graph_refused: bool,
    // Set only by fresh_output after its new exclusive lease proves the old
    // callback and native Root have actually retired.
    pub(super) output_retired: bool,
    pub(super) retired_without_submission: bool,
    pub(super) fence_requested: bool,
    pub(super) wait_stream_cut: Option<u64>,
    pub(super) flushed: bool,
    pub(super) rollback: bool,
    pub(super) graph_installed: bool,
}
impl StoppedPoint {
    fn matches(&self, request: GestureRequest) -> bool {
        match request {
            GestureRequest::ChangePlugin {
                key,
                sequence,
                value,
            }
            | GestureRequest::CommitPlugin {
                key,
                sequence,
                value,
            } => {
                key == self.snapshot.key
                    && sequence == self.snapshot.sequence
                    && (value == self.value
                        || (self.restore && Some(value) == self.snapshot.plugin_value))
            }
            GestureRequest::Cancel { key } => self.restore && key == self.snapshot.key,
            _ => false,
        }
    }
}

pub(super) struct PendingCommit {
    snapshot: GestureSnapshot,
    context: AudioSourceContext,
    binding: Binding,
    output: u64,
    token: Option<NativeWorkerCapture>,
    waiting_state: Option<NativeDeviceState>,
    captured: Option<Box<NativeWorkerStateSnapshot>>,
    staged: bool,
    decision: Option<bool>,
    failure: Option<String>,
    current: bool,
}
impl PendingPoint {
    fn matches(&self, request: GestureRequest) -> bool {
        match request {
            GestureRequest::ChangePlugin {
                key,
                sequence,
                value,
            }
            | GestureRequest::CommitPlugin {
                key,
                sequence,
                value,
            } => {
                key == self.snapshot.key
                    && sequence == self.snapshot.sequence
                    && (value == self.value
                        || (self.restore && Some(value) == self.snapshot.plugin_value))
            }
            GestureRequest::Cancel { key } => self.restore && key == self.snapshot.key,
            _ => false,
        }
    }
}

impl Plugins {
    pub(in crate::native_audio) fn matches_retired_stopped_gesture(
        &self,
        context: AudioSourceContext,
        key: GestureKey,
        sequence: u64,
        output: u64,
    ) -> bool {
        self.gesture.is_some_and(|gesture| {
            gesture.key == key && gesture.origin.context == context && gesture.output == output
        }) && self.stopped_gesture_point.as_ref().is_some_and(|point| {
            point.output_retired
                && point.context == context
                && point.output == output
                && point.snapshot.key == key
                && point.snapshot.sequence == sequence
        })
    }
    pub(in crate::native_audio) fn retire_unsubmitted_stopped_gesture(
        &mut self,
        context: AudioSourceContext,
        key: GestureKey,
        sequence: u64,
        old_output: u64,
    ) -> bool {
        if self.restore.is_some()
            || self.gesture_point.is_some()
            || self.gesture_capture.is_some()
            || !self.gesture.is_some_and(|gesture| {
                gesture.key == key
                    && gesture.origin.context == context
                    && gesture.output == old_output
            })
            || !self.stopped_gesture_point.as_ref().is_some_and(|point| {
                point.output_retired
                    && point.retired_without_submission
                    && point.context == context
                    && point.output == old_output
                    && point.snapshot.key == key
                    && point.snapshot.sequence == sequence
            })
        {
            return false;
        }
        self.stopped_gesture_point = None;
        self.gesture = None;
        true
    }
    pub(in crate::native_audio) fn retire_closed_stopped_gesture(
        &mut self,
        context: AudioSourceContext,
        key: GestureKey,
        sequence: u64,
        old_output: u64,
    ) -> bool {
        if self.restore.is_some()
            || self.gesture_point.is_some()
            || self.gesture_capture.is_some()
            || !self.gesture.is_some_and(|gesture| {
                gesture.key == key
                    && gesture.origin.context == context
                    && gesture.output == old_output
            })
            || !self.stopped_gesture_point.as_ref().is_some_and(|point| {
                point.context == context
                    && point.output == old_output
                    && point.snapshot.key == key
                    && point.snapshot.sequence == sequence
            })
        {
            return false;
        }
        self.stopped_gesture_point = None;
        self.gesture = None;
        true
    }
    pub(in crate::native_audio) fn finish_stopped_gesture(
        &mut self,
        context: AudioSourceContext,
        key: GestureKey,
        sequence: u64,
        receipt: hex_session::service::ResidentPublicationReceipt,
        accepted: bool,
        control: Option<&mut hex_audio::prepared::PublicationControl>,
    ) -> hex_session::service::StoppedGestureCompletion {
        use hex_session::service::StoppedGestureCompletion as Completion;
        let Some(control) = control else {
            return Completion::Conflict;
        };
        let Some(point) = self.stopped_gesture_point.as_mut() else {
            return Completion::Conflict;
        };
        if point.context != context
            || point.snapshot.key != key
            || point.snapshot.sequence != sequence
            || receipt.context != context
            || receipt.output_identity != point.output
        {
            return Completion::Conflict;
        }
        if !accepted {
            let Some(restore) = self.restore.as_mut() else {
                return Completion::Conflict;
            };
            if !restore.begin_refused_gesture_rollback(receipt, control) {
                return Completion::Conflict;
            }
            point.cancel_requested = true;
            point.graph_refused = true;
            point.rollback = point.flushed;
            return Completion::Completed;
        }
        let source = match control.status.recording_source_observation() {
            AudioSourceObservation::Pending => return Completion::Pending,
            AudioSourceObservation::Applied(source) => source,
            AudioSourceObservation::Absent => return Completion::Conflict,
        };
        if point.cancel_requested && point.flushed {
            let Some(restore) = self.restore.as_mut() else {
                return Completion::Conflict;
            };
            if !restore.begin_gesture_rollback(context, source, control) {
                return Completion::Conflict;
            }
            point.rollback = true;
            return Completion::Completed;
        }
        let Some(restore) = self.restore.as_ref() else {
            return Completion::Conflict;
        };
        if !restore.finish_gesture_graph(context, source, control) {
            return Completion::Conflict;
        }
        self.restore = None;
        point.graph_installed = true;
        Completion::Completed
    }
    pub(in crate::native_audio) fn retire_closed_gesture(
        &mut self,
        context: AudioSourceContext,
        key: GestureKey,
        sequence: u64,
        old_output: u64,
    ) -> bool {
        let matching_gesture = self.gesture.is_some_and(|gesture| {
            gesture.key == key && gesture.origin.context == context && gesture.output == old_output
        });
        let matching_capture = self.gesture_capture.as_ref().is_some_and(|capture| {
            capture.context == context
                && capture.snapshot.key == key
                && capture.snapshot.sequence == sequence
                && capture.output == old_output
                && capture.staged
        });
        if !matching_gesture || !matching_capture || self.gesture_point.is_some() {
            return false;
        }
        // A replacement exclusive lease and installed canonical graph have
        // already proved the old callback and Root are fully retired.
        self.gesture_capture = None;
        self.gesture = None;
        true
    }
    pub(in crate::native_audio) fn prepare_plugin_gesture(
        &mut self,
        output: Option<&mut WorkerPreparedOutput>,
        current: Option<AudioSourceContext>,
        input: hex_session::service::PluginParameterGestureInput<'_>,
    ) -> Result<ResultState, Error> {
        let context = input.context();
        match input.operation() {
            PluginGesturePreparation::Begin { origin, snapshot } => {
                if origin.context != context || origin.metadata_revision == 0 {
                    return Err(Error::Refused("vst3 parameter context changed".into()));
                }
                let request = PluginControllerPageRequest {
                    context,
                    device_id: origin.device.value(),
                    first: origin.page_first,
                    revision: origin.metadata_revision,
                };
                if self.gesture.is_none() {
                    if current != Some(context) {
                        return Err(Error::Refused("vst3 parameter context changed".into()));
                    }
                    let output = output
                        .ok_or_else(|| Error::Refused("vst3 output is unavailable".into()))?;
                    self.begin_controller_page(output, current, request, input.canonical_state())
                        .map_err(Error::Refused)?;
                    let (binding, output) =
                        self.controller_page_identity(request).ok_or_else(|| {
                            Error::Refused("vst3 controller identity is unavailable".into())
                        })?;
                    self.gesture = Some(Gesture {
                        key: snapshot.key,
                        origin,
                        request,
                        admitted: false,
                        binding,
                        output,
                        last_value: origin.initial,
                        last_sequence: 0,
                    });
                    return Ok(ResultState::Pending);
                }
                let gesture = self
                    .gesture
                    .ok_or_else(|| Error::Refused("vst3 gesture disappeared".into()))?;
                if gesture.key != snapshot.key
                    || gesture.origin != origin
                    || gesture.request != request
                    || gesture.admitted
                {
                    return Err(Error::Refused("vst3 gesture identity differs".into()));
                }
                match self.take_controller_page(request, current) {
                    Ok(PluginControllerPageResult::Pending) => Ok(ResultState::Pending),
                    Ok(PluginControllerPageResult::Page(page)) => {
                        let valid = page.revision == origin.metadata_revision
                            && page.parameters.iter().any(|parameter| {
                                parameter.id == origin.parameter
                                    && parameter.editable
                                    && parameter.normalized == origin.initial.normalized()
                            });
                        if !valid {
                            self.gesture = None;
                            return Err(Error::Refused("vst3 parameter metadata changed".into()));
                        }
                        if let Some(gesture) = self.gesture.as_mut() {
                            gesture.admitted = true;
                        }
                        Ok(ResultState::Applied)
                    }
                    Ok(PluginControllerPageResult::Changed { .. }) => {
                        self.gesture = None;
                        Err(Error::Refused("vst3 parameter metadata changed".into()))
                    }
                    Err(reason) => {
                        self.gesture = None;
                        Err(Error::Refused(reason))
                    }
                }
            }
            PluginGesturePreparation::Restore { snapshot, initial } => {
                let gesture = self
                    .gesture
                    .ok_or_else(|| Error::Refused("vst3 gesture is unavailable".into()))?;
                if !gesture.admitted
                    || gesture.key != snapshot.key
                    || gesture.origin.initial != initial
                    || snapshot.target
                        != (GestureTarget::PluginParameter {
                            device: gesture.origin.device,
                            parameter: gesture.origin.parameter,
                        })
                {
                    return Err(Error::Refused("vst3 gesture restore differs".into()));
                }
                if gesture.last_value == initial && self.gesture_point.is_none() {
                    self.gesture = None;
                    return Ok(ResultState::Applied);
                }
                self.prepare_point(
                    output,
                    current,
                    input.canonical_state(),
                    snapshot,
                    initial,
                    true,
                )
            }
            PluginGesturePreparation::Preview(snapshot) => {
                let Some(gesture) = self.gesture else {
                    return Err(Error::Refused("vst3 gesture is unavailable".into()));
                };
                if gesture.admitted
                    && gesture.key == snapshot.key
                    && current == Some(gesture.origin.context)
                    && snapshot.target
                        == (GestureTarget::PluginParameter {
                            device: gesture.origin.device,
                            parameter: gesture.origin.parameter,
                        })
                    && snapshot.plugin_value == Some(gesture.last_value)
                    && self.gesture_point.is_none()
                {
                    if let Some(gesture) = self.gesture.as_mut() {
                        gesture.last_sequence = snapshot.sequence;
                    }
                    Ok(ResultState::Applied)
                } else {
                    let value = snapshot.plugin_value.ok_or_else(|| {
                        Error::Refused("vst3 parameter value is unavailable".into())
                    })?;
                    self.prepare_point(
                        output,
                        current,
                        input.canonical_state(),
                        snapshot,
                        value,
                        false,
                    )
                }
            }
            PluginGesturePreparation::Commit(snapshot) => {
                self.prepare_commit(output, current, input.canonical_state(), snapshot)
            }
        }
    }

    pub(in crate::native_audio) fn plugin_gesture_ready(
        &self,
        context: AudioSourceContext,
        request: GestureRequest,
    ) -> bool {
        let Some(gesture) = self.gesture else {
            return false;
        };
        if let Some(point) = self.stopped_gesture_point.as_ref() {
            return point.context == context
                && point.matches(request)
                && (point.output_retired
                    && point.retired_without_submission
                    && self.reopen_ready() == Some(context)
                    || !point.output_retired
                        && (point.wait_stream_cut.is_none() || point.cancel_requested)
                        && (!point.fence_requested
                            || point.graph_installed
                            || self.restore.as_ref().is_some_and(|restore| {
                                restore.ready_for_gesture(context)
                                    || restore.gesture_fence_refusal(context).is_some()
                            })));
        }
        if let Some(point) = self.gesture_point.as_ref() {
            return point.context == context
                && point.matches(request)
                && point.current
                && (point.admission_error.is_some()
                    || point
                        .terminal
                        .is_some_and(|terminal| terminal.source_current()));
        }
        if let Some(commit) = self.gesture_capture.as_ref() {
            return context == commit.context
                && commit.current
                && commit.decision.is_none()
                && matches!(request, GestureRequest::CommitPlugin { key, sequence, value } if key == commit.snapshot.key && sequence == commit.snapshot.sequence && Some(value) == commit.snapshot.plugin_value)
                && (commit.captured.is_some() || commit.failure.is_some());
        }
        matches!(request, GestureRequest::BeginPlugin { key, origin } if key == gesture.key && origin == gesture.origin)
            && context == gesture.origin.context
            && !gesture.admitted
            && self.controller_page_ready(gesture.request)
    }

    pub(super) fn poll_gesture_point(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        current: Option<AudioSourceContext>,
    ) {
        if let Some(point) = self.stopped_gesture_point.as_mut()
            && let Some(cut) = point.wait_stream_cut
            && control.status.stream_applied_command_prefix.load(Acquire) >= cut
        {
            point.wait_stream_cut = None;
        }
        let Some(gesture) = self.gesture else {
            return;
        };
        let source_live = self.sources.iter().any(|source| {
            source.binding == gesture.binding
                && source.accepted
                && !source.retiring
                && source.project_generation == gesture.origin.context.project_generation
                && source.device == gesture.origin.device.value()
        });
        let Some(point) = self.gesture_point.as_mut() else {
            return;
        };
        let same_output = control.output_identity() == Some(point.output);
        point.current = current == Some(point.context) && same_output;
        if !same_output || point.terminal.is_some() {
            return;
        }
        if point.token.is_none() {
            if point.cancel_requested {
                point.admission_error = Some("vst3 parameter input was cancelled before admission");
                return;
            }
            if !point.current {
                return;
            }
            let Some(state) = point.waiting_state.as_ref() else {
                point.admission_error = Some("vst3 parameter admission state is missing");
                return;
            };
            if !source_live {
                point.admission_error = Some("vst3 parameter source binding is no longer active");
                return;
            }
            match control.status.recording_source_observation() {
                AudioSourceObservation::Pending => return,
                AudioSourceObservation::Applied(source)
                    if source.context == point.context
                        && source.output_identity == point.output => {}
                _ => {
                    point.admission_error = Some("vst3 parameter output source changed");
                    return;
                }
            }
            match control.native_source_matches_accepted_state_on_control(point.binding, state) {
                Ok(true) => {}
                Ok(false) => {
                    point.admission_error = Some("vst3 parameter canonical state changed");
                    return;
                }
                Err(reason) if parameter_admission_busy(reason) => return,
                Err(reason) => {
                    point.admission_error = Some(reason);
                    return;
                }
            }
            match control.begin_native_source_parameter_change_on_control(
                point.binding,
                gesture.origin.parameter,
                point.value.normalized(),
            ) {
                Ok(token) => {
                    point.token = Some(token);
                    point.waiting_state = None;
                }
                Err(reason) if parameter_admission_busy(reason) => return,
                Err(reason) => {
                    point.admission_error = Some(reason);
                    return;
                }
            }
        }
        let Some(token) = point.token.as_ref() else {
            return;
        };
        if point.cancel_requested {
            let _ =
                control.withdraw_native_source_parameter_change_on_control(point.binding, token);
        }
        let progress =
            match control.poll_native_source_parameter_change_on_control(point.binding, token) {
                Ok(NativeParameterPoll::TransportRevoked) => control
                    .drain_stopped_native_source_parameter_change_on_control(point.binding, token),
                result => result,
            };
        if let Ok(NativeParameterPoll::Terminal(terminal)) = progress {
            point.terminal = Some(terminal);
        }
    }

    pub(super) fn poll_gesture_capture(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        current: Option<AudioSourceContext>,
    ) {
        let Some(commit) = self.gesture_capture.as_mut() else {
            return;
        };
        let same_output = control.output_identity() == Some(commit.output);
        commit.current = same_output && current == Some(commit.context);
        if !same_output {
            return;
        }
        if let Some(accepted) = commit.decision {
            if let Some(snapshot) = commit.captured.as_ref() {
                match control.finalize_native_source_state_snapshot_on_control(
                    commit.binding,
                    snapshot,
                    accepted,
                ) {
                    Ok(NativeSnapshotAction::Current) => {
                        self.gesture_capture = None;
                        if accepted {
                            self.gesture = None;
                        }
                    }
                    Ok(NativeSnapshotAction::SourceClosed) => {
                        self.gesture_capture = None;
                        if accepted {
                            self.gesture = None;
                        }
                    }
                    Err(_) => {} // Keep the exact staged receipt until its owner resolves.
                }
            }
            return;
        }
        if commit.captured.is_some() || commit.failure.is_some() {
            return;
        }
        if commit.token.is_none() {
            if !commit.current {
                return;
            }
            let Some(state) = commit.waiting_state.as_ref() else {
                commit.failure = Some("vst3 commit admission state is missing".into());
                return;
            };
            match control.status.recording_source_observation() {
                AudioSourceObservation::Pending => return,
                AudioSourceObservation::Applied(source)
                    if source.context == commit.context
                        && source.output_identity == commit.output => {}
                _ => {
                    commit.failure = Some("vst3 parameter output source changed".into());
                    return;
                }
            }
            match control.native_source_matches_accepted_state_on_control(commit.binding, state) {
                Ok(true) => {}
                Ok(false) => {
                    commit.failure = Some("vst3 parameter canonical state changed".into());
                    return;
                }
                Err(reason) if parameter_admission_busy(reason) => return,
                Err(reason) => {
                    commit.failure = Some(reason.into());
                    return;
                }
            }
            match control.begin_native_source_state_capture_on_control(commit.binding) {
                Ok(token) => {
                    commit.token = Some(token);
                    commit.waiting_state = None;
                }
                Err("native preparation is busy" | "native capture mailbox is busy") => return,
                Err(reason) => {
                    commit.failure = Some(reason.into());
                    return;
                }
            }
        }
        let Some(token) = commit.token.as_ref() else {
            return;
        };
        match control.poll_native_source_state_snapshot_on_control(commit.binding, token) {
            Ok(NativeWorkerStateSnapshotProgress::Captured(snapshot)) => {
                commit.captured = Some(snapshot)
            }
            Ok(
                NativeWorkerStateSnapshotProgress::Pending
                | NativeWorkerStateSnapshotProgress::Failed
                | NativeWorkerStateSnapshotProgress::Discarded,
            )
            | Err("native preparation is busy" | "native capture mailbox is busy") => {}
            Err(_) => {} // A close/refusal cannot erase the original token.
        }
    }

    fn prepare_commit(
        &mut self,
        output: Option<&mut WorkerPreparedOutput>,
        current: Option<AudioSourceContext>,
        canonical: &NativeDeviceState,
        snapshot: GestureSnapshot,
    ) -> Result<ResultState, Error> {
        let gesture = self
            .gesture
            .ok_or_else(|| Error::Refused("vst3 gesture is unavailable".into()))?;
        if !gesture.admitted
            || snapshot.key != gesture.key
            || snapshot.sequence != gesture.last_sequence
            || snapshot.plugin_value != Some(gesture.last_value)
            || self.gesture_point.is_some()
            || snapshot.target
                != (GestureTarget::PluginParameter {
                    device: gesture.origin.device,
                    parameter: gesture.origin.parameter,
                })
        {
            return Err(Error::Refused("vst3 commit identity differs".into()));
        }
        if self.gesture_capture.is_none() {
            if current != Some(gesture.origin.context) {
                return Err(Error::Refused("vst3 parameter context changed".into()));
            }
            let control = output
                .and_then(|output| output.publication.control())
                .ok_or_else(|| Error::Refused("vst3 output is unavailable".into()))?;
            if control.output_identity() != Some(gesture.output) {
                return Err(Error::Refused("vst3 parameter output changed".into()));
            }
            self.gesture_capture = Some(PendingCommit {
                snapshot,
                context: gesture.origin.context,
                binding: gesture.binding,
                output: gesture.output,
                token: None,
                waiting_state: Some(canonical.clone()),
                captured: None,
                staged: false,
                decision: None,
                failure: None,
                current: true,
            });
            return Ok(ResultState::Pending);
        }
        let commit = self
            .gesture_capture
            .as_mut()
            .ok_or_else(|| Error::Refused("vst3 capture disappeared".into()))?;
        if commit.snapshot != snapshot
            || commit.context != gesture.origin.context
            || commit.decision.is_some()
        {
            return Err(Error::Refused("vst3 capture identity differs".into()));
        }
        if let Some(reason) = commit.failure.take() {
            self.gesture_capture = None;
            return Err(Error::Refused(reason));
        }
        let Some(captured) = commit.captured.as_ref() else {
            return Ok(ResultState::Pending);
        };
        if !commit.current || current != Some(commit.context) {
            return Ok(ResultState::Pending);
        }
        let Some(control) = output.and_then(|output| output.publication.control()) else {
            return Ok(ResultState::Pending);
        };
        if control.output_identity() != Some(commit.output)
            || !matches!(
                control.native_source_matches_accepted_state_on_control(commit.binding, canonical),
                Ok(true)
            )
            || control.status.recording_source().is_none_or(|source| {
                source.context != commit.context || source.output_identity != commit.output
            })
        {
            return Ok(ResultState::Pending);
        }
        if !commit.staged {
            match control.stage_native_source_state_snapshot_on_control(commit.binding, captured) {
                Ok(NativeSnapshotAction::Current) => commit.staged = true,
                Ok(NativeSnapshotAction::SourceClosed) => {
                    commit.staged = true;
                    commit.current = false;
                    return Ok(ResultState::Pending);
                }
                Err("native preparation is busy" | "native capture mailbox is busy") => {
                    return Ok(ResultState::Pending);
                }
                Err(reason) => {
                    let reason = reason.to_owned();
                    self.gesture_capture = None;
                    return Err(Error::Refused(reason));
                }
            }
        }
        Ok(ResultState::Captured(captured.state().clone()))
    }

    pub(in crate::native_audio) fn finish_plugin_commit(
        &mut self,
        context: AudioSourceContext,
        key: GestureKey,
        sequence: u64,
        accepted: bool,
    ) {
        if let Some(commit) = self.gesture_capture.as_mut()
            && commit.context == context
            && commit.snapshot.key == key
            && commit.snapshot.sequence == sequence
            && commit.staged
            && commit.decision.is_none()
        {
            commit.decision = Some(accepted);
        }
    }

    pub(super) fn staged_gesture_binding(
        &self,
        control: &mut hex_audio::prepared::PublicationControl,
        input: &ResidentEffectPreparation<'_>,
        owner: ProcessorChainOwner,
        device: u64,
        state: &NativeDeviceState,
        cancellation: Option<&hex_session::service::RequestCancellation>,
    ) -> Result<Option<Binding>, String> {
        let (Some(gesture), Some(commit)) = (self.gesture, self.gesture_capture.as_ref()) else {
            return Ok(None);
        };
        if !input.preparing_edit()
            || device != gesture.origin.device.value()
            || input.context().project_generation != commit.context.project_generation
            || commit.context.editor_revision.checked_add(1)
                != Some(input.context().editor_revision)
        {
            return Ok(None);
        }
        let snapshot = commit
            .captured
            .as_ref()
            .ok_or("vst3 staged capture is missing")?;
        if !commit.staged
            || commit.decision.is_some()
            || snapshot.state() != state
            || control.output_identity() != Some(commit.output)
            || !self.sources.iter().any(|source| {
                source.binding == commit.binding
                    && source.owner == owner
                    && source.device == device
                    && source.accepted
                    && !source.retiring
            })
        {
            return Err("vst3 staged capture differs from the exact gesture".into());
        }
        loop {
            edits::check_cancel(cancellation)?;
            match control.matches_staged_native_source_state_snapshot_on_control(
                commit.binding,
                snapshot,
                state,
            ) {
                Ok(true) => return Ok(Some(commit.binding)),
                Ok(false) => return Err("vst3 staged capture source changed".into()),
                Err("native preparation is busy" | "native capture mailbox is busy") => {
                    edits::poll_control(control)
                }
                Err(reason) => return Err(reason.into()),
            }
        }
    }

    pub(in crate::native_audio) fn cancel_plugin_point(
        &mut self,
        context: AudioSourceContext,
        request: GestureRequest,
    ) {
        if let Some(point) = self.gesture_point.as_mut()
            && point.context == context
            && point.matches(request)
        {
            point.cancel_requested = true;
        }
        if let Some(point) = self.stopped_gesture_point.as_mut()
            && point.context == context
            && point.matches(request)
        {
            point.cancel_requested = true;
        }
    }

    fn prepare_point(
        &mut self,
        mut output: Option<&mut WorkerPreparedOutput>,
        current: Option<AudioSourceContext>,
        canonical: &NativeDeviceState,
        snapshot: GestureSnapshot,
        value: PluginParameterValue,
        restore: bool,
    ) -> Result<ResultState, Error> {
        let gesture = self
            .gesture
            .ok_or_else(|| Error::Refused("vst3 gesture is unavailable".into()))?;
        if !gesture.admitted
            || snapshot.key != gesture.key
            || snapshot.target
                != (GestureTarget::PluginParameter {
                    device: gesture.origin.device,
                    parameter: gesture.origin.parameter,
                })
            || snapshot.sequence < gesture.last_sequence
        {
            return Err(Error::Refused("vst3 parameter gesture differs".into()));
        }
        if let Some(point) = self.stopped_gesture_point.as_ref() {
            if point.snapshot != snapshot || point.value != value || point.restore != restore {
                return Err(Error::Refused(
                    "another stopped vst3 point is pending".into(),
                ));
            }
            if point.output_retired {
                return if point.retired_without_submission {
                    Ok(ResultState::StoppedOutputClosed {
                        old_output: point.output,
                    })
                } else {
                    Ok(ResultState::Pending)
                };
            }
            if !point.fence_requested {
                if point.cancel_requested {
                    self.stopped_gesture_point = None;
                    return Err(Error::Refused(
                        "vst3 parameter input was cancelled before admission".into(),
                    ));
                }
            } else if !point.graph_installed {
                let refusal = self.restore.as_ref().and_then(|restore| {
                    restore
                        .gesture_fence_refusal(point.context)
                        .map(str::to_owned)
                });
                if let Some(reason) = refusal {
                    let control = output
                        .as_deref_mut()
                        .and_then(|output| output.publication.control());
                    if let Some(control) = control
                        && self
                            .restore
                            .as_ref()
                            .is_some_and(|restore| restore.has_original_output(control))
                    {
                        self.cancel_restore(Some(control));
                        self.stopped_gesture_point = None;
                        return Err(Error::Refused(reason));
                    }
                    return Ok(ResultState::Pending);
                }
                return if self
                    .restore
                    .as_ref()
                    .is_some_and(|restore| restore.ready_for_gesture(point.context))
                {
                    Ok(ResultState::StoppedGraphReady)
                } else {
                    Ok(ResultState::Pending)
                };
            } else if point.cancel_requested && point.flushed {
                // The SDK already changed state. Keep custody until the
                // cancellation can restore that exact point; never claim it
                // was withdrawn merely because its graph installed.
                return Ok(ResultState::Pending);
            } else {
                let cancelled = point.cancel_requested;
                let graph_refused = point.graph_refused;
                self.stopped_gesture_point = None;
                if graph_refused {
                    return Err(Error::Refused(
                        "vst3 stopped graph publication was refused".into(),
                    ));
                }
                if cancelled {
                    return Err(Error::Refused(
                        "vst3 parameter input was cancelled before admission".into(),
                    ));
                }
                if restore {
                    self.gesture = None;
                } else if let Some(active) = self.gesture.as_mut() {
                    active.last_value = value;
                    active.last_sequence = snapshot.sequence;
                }
                return Ok(ResultState::Applied);
            }
        }
        if let Some(point) = self.gesture_point.as_ref() {
            if point.snapshot != snapshot || point.value != value || point.restore != restore {
                return Err(Error::Refused(
                    "another vst3 parameter point is pending".into(),
                ));
            }
            if let Some(reason) = point.admission_error {
                self.gesture_point = None;
                return Err(Error::Refused(reason.into()));
            }
            let Some(terminal) = point.terminal else {
                return Ok(ResultState::Pending);
            };
            if !point.current || current != Some(point.context) || !terminal.source_current() {
                return Ok(ResultState::Pending);
            }
            let Some(control) = output.and_then(|output| output.publication.control()) else {
                return Ok(ResultState::Pending);
            };
            if control.output_identity() != Some(gesture.output)
                || !matches!(
                    control.native_source_matches_accepted_state_on_control(
                        gesture.binding,
                        canonical
                    ),
                    Ok(true)
                )
                || control.status.recording_source().is_none_or(|source| {
                    source.context != gesture.origin.context
                        || source.output_identity != gesture.output
                })
            {
                // A consumed SDK point cannot authorize a gesture ACK after
                // its original source or installed graph stops being current.
                return Ok(ResultState::Pending);
            }
            match terminal.progress() {
                NativeWorkerParameterProgress::Processed(processed)
                    if processed.point.id == gesture.origin.parameter
                        && processed.point.normalized == value.normalized() =>
                {
                    self.gesture_point = None;
                    if restore {
                        self.gesture = None;
                    } else if let Some(active) = self.gesture.as_mut() {
                        active.last_value = value;
                        active.last_sequence = snapshot.sequence;
                    }
                    return Ok(ResultState::Applied);
                }
                NativeWorkerParameterProgress::Refused
                | NativeWorkerParameterProgress::Withdrawn => {
                    self.gesture_point = None;
                    return Err(Error::Refused("vst3 parameter input was refused".into()));
                }
                _ => return Ok(ResultState::Pending),
            }
        }
        if current != Some(gesture.origin.context) {
            return Err(Error::Refused("vst3 parameter context changed".into()));
        }
        let output = output.ok_or_else(|| Error::Refused("vst3 output is unavailable".into()))?;
        let control = output
            .publication
            .control()
            .ok_or_else(|| Error::Refused("vst3 output is unavailable".into()))?;
        if control.output_identity() != Some(gesture.output) {
            return Err(Error::Refused("vst3 parameter output changed".into()));
        }
        if !self.sources.iter().any(|source| {
            source.binding == gesture.binding
                && source.accepted
                && !source.retiring
                && source.project_generation == gesture.origin.context.project_generation
                && source.device == gesture.origin.device.value()
        }) {
            return Err(Error::Refused(
                "vst3 parameter source binding is no longer active".into(),
            ));
        }
        let source_pending = match control.status.recording_source_observation() {
            AudioSourceObservation::Pending => true,
            AudioSourceObservation::Applied(source)
                if source.context == gesture.origin.context
                    && source.output_identity == gesture.output =>
            {
                false
            }
            _ => {
                return Err(Error::Refused(
                    "vst3 parameter output source changed".into(),
                ));
            }
        };
        let state_matches = if source_pending {
            None
        } else {
            match control
                .native_source_matches_accepted_state_on_control(gesture.binding, canonical)
            {
                Ok(matches) => Some(matches),
                Err(reason) if parameter_admission_busy(reason) => None,
                Err(reason) => return Err(Error::Refused(reason.into())),
            }
        };
        if state_matches == Some(false) {
            return Err(Error::Refused(
                "vst3 parameter canonical state changed".into(),
            ));
        }
        if !control.status.playing.load(Acquire) {
            if state_matches.is_none() {
                if self.stopped_gesture_point.is_none() {
                    self.stopped_gesture_point = Some(StoppedPoint {
                        snapshot,
                        value,
                        restore,
                        binding: gesture.binding,
                        output: gesture.output,
                        context: gesture.origin.context,
                        cancel_requested: false,
                        graph_refused: false,
                        output_retired: false,
                        retired_without_submission: false,
                        fence_requested: false,
                        wait_stream_cut: None,
                        flushed: false,
                        rollback: false,
                        graph_installed: false,
                    });
                }
                return Ok(ResultState::Pending);
            }
            match self.prepare_restore(output, gesture.origin.context) {
                Ok(Some(false)) => {
                    if let Some(point) = self.stopped_gesture_point.as_mut() {
                        point.fence_requested = true;
                    } else {
                        self.stopped_gesture_point = Some(StoppedPoint {
                            snapshot,
                            value,
                            restore,
                            binding: gesture.binding,
                            output: gesture.output,
                            context: gesture.origin.context,
                            cancel_requested: false,
                            graph_refused: false,
                            output_retired: false,
                            retired_without_submission: false,
                            fence_requested: true,
                            wait_stream_cut: None,
                            flushed: false,
                            rollback: false,
                            graph_installed: false,
                        });
                    }
                    return Ok(ResultState::Pending);
                }
                Ok(_) => return Err(Error::Refused("stopped vst3 source is unavailable".into())),
                Err(reason) => return Err(Error::Refused(reason)),
            }
        }
        if self
            .stopped_gesture_point
            .as_ref()
            .is_some_and(|point| !point.fence_requested)
        {
            self.stopped_gesture_point = None;
        }
        let token = if state_matches == Some(true) {
            match control.begin_native_source_parameter_change_on_control(
                gesture.binding,
                gesture.origin.parameter,
                value.normalized(),
            ) {
                Ok(token) => Some(token),
                Err(reason) if parameter_admission_busy(reason) => None,
                Err(reason) => return Err(Error::Refused(reason.into())),
            }
        } else {
            None
        };
        let waiting_state = token.is_none().then(|| canonical.clone());
        self.gesture_point = Some(PendingPoint {
            snapshot,
            value,
            restore,
            binding: gesture.binding,
            output: gesture.output,
            context: gesture.origin.context,
            token,
            waiting_state,
            admission_error: None,
            terminal: None,
            current: true,
            cancel_requested: false,
        });
        Ok(ResultState::Pending)
    }

    pub(in crate::native_audio) fn finish_stale_plugin_gesture(
        &mut self,
        context: AudioSourceContext,
        request: GestureRequest,
        current: Option<AudioSourceContext>,
    ) -> StalePluginGestureDisposition {
        let Some(gesture) = self.gesture else {
            return StalePluginGestureDisposition::Pending;
        };
        if context != gesture.origin.context
            || !matches!(request, GestureRequest::BeginPlugin { key, origin } if key == gesture.key && origin == gesture.origin)
            || gesture.admitted
        {
            return StalePluginGestureDisposition::Pending;
        }
        if !self.controller_page_ready(gesture.request) {
            return StalePluginGestureDisposition::Pending;
        }
        match self.take_controller_page(gesture.request, current) {
            Ok(PluginControllerPageResult::Pending) => StalePluginGestureDisposition::Pending,
            _ => {
                // Begin is observation-only, so no native point needs restoring.
                self.gesture = None;
                StalePluginGestureDisposition::Restored
            }
        }
    }
}
