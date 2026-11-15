//! One control-owned plug-in editor take. Prepare returns copied provenance so
//! Session can open its existing draft before Start affects SDK PCM. The only
//! controller edit reader supplies order; the private Main receipt supplies
//! values and clocks. One unmatched quantum and one observation bound custody.
use super::*;
use hex_audio::foreign_parameter_delivery::editor::{
    AbortReason, ConfirmedQuantum, GroupTouchTarget, Receipt, Retired, Target, TouchPlan,
};
use hex_audio::plugin_worker::ParameterEdit;
use hex_audio::prepared::{AppliedAudioSource, PublicationControl};
use hex_session::device_gesture::GestureKey;
use hex_session::service::automation_draft::{Vst3CaptureTarget, Vst3GroupCaptureTarget};
use hex_session::service::{
    DeviceAutomationSource, DeviceCaptureTarget, MAX_VST3_EDITOR_GROUP_MEMBERS as MEMBERS,
    PluginEditorRecordingInput, PluginEditorRecordingTarget,
    PluginParameterObservation as Observation, RequestCancellation, ResidentRuntimeLoop,
    Vst3EditorGroupObservation, Vst3EditorGroupReceipt, Vst3EditorGroupRows,
};
mod correlation;
mod group;
use correlation::{Correlation, Match};
use group::{GroupCorrelation, Quantum};

// Audio can leave its whole confirmed queue behind. One Desktop observation,
// one Session action and one racing final callback are separate credits.
const _: () = assert!(
    hex_session::service::MAX_FAILED_PLUGIN_EDITOR_OBSERVATIONS
        >= hex_audio::foreign_parameter_delivery::editor::MAX_EDITOR_MAIN_RECEIPTS + 3
);

#[derive(Clone, Copy)]
enum Plan {
    Single(TouchPlan),
    Group {
        plans: [GroupTouchTarget; MEMBERS],
        len: usize,
    },
}

#[derive(Clone, Copy)]
pub(super) struct Prepared {
    key: GestureKey,
    applied: AppliedAudioSource,
    source: DeviceAutomationSource,
    plan: Plan,
}

pub(super) enum Recording {
    Prepared(Box<Prepared>),
    Active(Box<Active>),
}

pub(super) struct Active {
    correlation: ActiveCorrelation,
    pending: Option<ConfirmedQuantum>,
    observation: Option<Observation>,
    retired: Option<Retired>,
    cancel: bool,
    closed: bool,
    drained: bool,
}

impl Plugins {
    pub(in crate::native_audio) fn prepare_editor_recording(
        &mut self,
        control: &mut PublicationControl,
        input: PluginEditorRecordingInput,
        sample_rate: u32,
        clock: ResidentRuntimeLoop,
        cancellation: Option<&RequestCancellation>,
    ) -> Result<Observation, String> {
        if self.editor_recording.is_some()
            || self.parameter_gesture.is_some()
            || !self.confirmed_parameters.is_empty()
            || self.restore.is_some()
            || self.pending.is_some()
            || self.reopen.is_some()
            || self.edit.is_some()
        {
            return Err("finish the previous vst3 operation first".into());
        }
        let device = input.target.device();
        let class = input.target.class();
        let (applied, transport_epoch) = control
            .status
            .live_native_parameter_source()
            .filter(|(applied, _)| {
                applied.context == input.context
                    && clock.source_context == Some(input.context)
                    && applied.output_identity == clock.output_identity
                    && applied.audio_revision == clock.audio_revision
            })
            .ok_or("vst3 editor recording requires the current playing output")?;
        if input.key.generation != input.context.project_generation
            || input.key.token == 0
            || sample_rate == 0
        {
            return Err("vst3 editor recording scope differs".into());
        }
        let source = self
            .sources
            .iter()
            .rev()
            .find(|source| {
                source.accepted
                    && !source.retiring
                    && source.device == device.value()
                    && source.project_generation == input.context.project_generation
                    && source.binding.root == applied.output_identity
            })
            .ok_or("the selected vst3 editor source is unavailable on this output")?;
        if source
            .authored
            .as_ref()
            .and_then(AuthoredSelection::dense_parameter)
            .is_some()
        {
            return Err("vst3 editor touch needs a source without dense modulation".into());
        }
        let binding = source.binding;
        let (target, plan) = match input.target {
            PluginEditorRecordingTarget::Single {
                parameter, reading, ..
            } => {
                let read_base = |control: &mut PublicationControl| {
                    let metadata = edits::source_metadata(control, binding, cancellation)?;
                    if metadata.selection().cid != class {
                        return Err("vst3 editor class changed".into());
                    }
                    Ok::<_, String>(
                        metadata
                            .authored_base_values()
                            .find_map(|(id, value)| (id == parameter).then_some(value)),
                    )
                };
                let base = match read_base(control)? {
                    Some(base) => base,
                    None => {
                        let host = self.host.as_ref().ok_or("vst3 host is unavailable")?;
                        parameter_gesture::metadata::extend_authored(
                            host,
                            control,
                            binding,
                            class,
                            parameter,
                            None,
                            cancellation,
                        )?;
                        read_base(control)?.ok_or("vst3 editor parameter has no exact base")?
                    }
                };
                (
                    DeviceCaptureTarget::Vst3(Vst3CaptureTarget {
                        device,
                        class,
                        parameter,
                        binding,
                    }),
                    Plan::Single(TouchPlan {
                        parameter,
                        base,
                        reading,
                    }),
                )
            }
            PluginEditorRecordingTarget::Group(group) => {
                let metadata = edits::source_metadata(control, binding, cancellation)?;
                if metadata.selection().cid != class {
                    return Err("vst3 editor class changed".into());
                }
                let mut plans = [GroupTouchTarget::default(); MEMBERS];
                for (slot, &parameter) in plans.iter_mut().zip(group.ids()) {
                    let base = metadata
                        .authored_base_values()
                        .find_map(|(id, value)| (id == parameter).then_some(value))
                        .ok_or("vst3 group parameter has no prepared restoration")?;
                    *slot = GroupTouchTarget {
                        parameter,
                        base,
                        reading: true,
                    };
                }
                (
                    DeviceCaptureTarget::Vst3Group(Vst3GroupCaptureTarget {
                        draft: group,
                        binding,
                    }),
                    Plan::Group {
                        plans,
                        len: group.ids().len(),
                    },
                )
            }
        };
        let source = DeviceAutomationSource {
            target,
            sample_rate,
            output_identity: applied.output_identity,
            audio_revision: applied.audio_revision,
            transport_epoch,
            section_loop: clock.section_loop,
        };
        let prepared = Prepared {
            key: input.key,
            applied,
            source,
            plan,
        };
        let ready = match target {
            DeviceCaptureTarget::Vst3(target) => Observation::Ready {
                key: prepared.key,
                context: applied.context,
                target,
                source,
            },
            DeviceCaptureTarget::Vst3Group(_) => Observation::Grouped(Vst3EditorGroupObservation {
                key: prepared.key,
                context: applied.context,
                source,
                receipt: Vst3EditorGroupReceipt::Ready,
            }),
            DeviceCaptureTarget::Native(_) => {
                return Err("vst3 editor scope requires a plug-in".into());
            }
        };
        self.editor_recording = Some(Recording::Prepared(Box::new(prepared)));
        Ok(ready)
    }

    pub(in crate::native_audio) fn start_editor_recording(
        &mut self,
        control: &mut PublicationControl,
        key: GestureKey,
    ) -> Result<(), String> {
        let Some(Recording::Prepared(prepared)) = self.editor_recording.as_ref() else {
            return Err("vst3 editor recording was not prepared".into());
        };
        let prepared = **prepared;
        if prepared.key != key {
            return Err("vst3 editor recording belongs to another take".into());
        }
        if control.status.live_native_parameter_source()
            != Some((prepared.applied, prepared.source.transport_epoch))
        {
            return Err("vst3 editor recording source changed".into());
        }
        let epoch = prepared.source.transport_epoch;
        let correlation = match (prepared.source.target, prepared.plan) {
            (DeviceCaptureTarget::Vst3(target), Plan::Single(plan)) => {
                if !self.matches_capture_source(prepared.applied.context, target) {
                    return Err("vst3 editor recording source changed".into());
                }
                let scope = control.arm_native_source_editor_touch_delivery_on_control(
                    prepared.applied,
                    epoch,
                    target.binding,
                    plan,
                )?;
                ActiveCorrelation::Single(
                    Correlation::new(scope, key, target, prepared.source).inspect_err(|_| {
                        control.cancel_native_source_editor_delivery_on_control(scope);
                    })?,
                )
            }
            (DeviceCaptureTarget::Vst3Group(target), Plan::Group { plans, len }) => {
                if !self.matches_capture_binding(
                    prepared.applied.context,
                    target.draft.device().value(),
                    target.binding,
                ) {
                    return Err("vst3 editor group source changed".into());
                }
                let scope = control.arm_native_source_editor_group_delivery_on_control(
                    prepared.applied,
                    epoch,
                    target.binding,
                    &plans[..len],
                )?;
                ActiveCorrelation::Group(Box::new(GroupControl {
                    correlation: GroupCorrelation::new(scope, target.draft),
                    identity: Vst3EditorGroupObservation {
                        key,
                        context: prepared.applied.context,
                        source: prepared.source,
                        receipt: Vst3EditorGroupReceipt::Ready,
                    },
                    terminal_sent: false,
                }))
            }
            _ => return Err("vst3 editor recording plan differs from its source".into()),
        };
        self.editor_recording = Some(Recording::Active(Box::new(Active {
            correlation,
            pending: None,
            observation: None,
            retired: None,
            cancel: false,
            closed: false,
            drained: false,
        })));
        self.latency_poll_at = None;
        Ok(())
    }

    pub(in crate::native_audio) fn cancel_editor_recording(&mut self, key: GestureKey) -> bool {
        match self.editor_recording.as_mut() {
            Some(Recording::Prepared(prepared)) if prepared.key == key => {
                self.editor_recording = None;
                true
            }
            Some(Recording::Active(active)) if active.correlation.key() == key => {
                active.cancel = true;
                true
            }
            _ => false,
        }
    }

    pub(in crate::native_audio) fn retire_editor_recording(
        &mut self,
        control: &mut PublicationControl,
    ) {
        if let Some(Recording::Active(active)) = self.editor_recording.as_mut()
            && !active.closed
        {
            active.retired = control.retire_native_source_editor_delivery_on_control();
            active.closed = true;
        }
    }

    pub(super) fn close_editor_recording(&mut self) {
        match self.editor_recording.as_mut() {
            Some(Recording::Prepared(_)) => self.editor_recording = None,
            Some(Recording::Active(active)) => active.closed = true,
            None => {}
        }
    }

    pub(super) fn recording_binding(&self) -> Option<Binding> {
        match self.editor_recording.as_ref() {
            Some(Recording::Active(active)) if !active.closed && !active.drained => {
                Some(active.correlation.target().binding)
            }
            _ => None,
        }
    }

    pub(super) fn observe_recording_order(&mut self, binding: Binding, edits: &[ParameterEdit]) {
        let Some(Recording::Active(active)) = self.editor_recording.as_mut() else {
            return;
        };
        if active.correlation.target().binding == binding {
            for &edit in edits {
                if active.correlation.ordered(edit).is_err() {
                    active.cancel = true;
                    if matches!(active.correlation, ActiveCorrelation::Single(_)) {
                        break;
                    }
                }
            }
        }
    }

    pub(in crate::native_audio) fn poll_editor_recording(
        &mut self,
        control: Option<&mut PublicationControl>,
    ) {
        if let Some(Recording::Active(active)) = self.editor_recording.as_mut() {
            active.poll(control);
            if active.drained && active.observation.is_none() {
                self.editor_recording = None;
            }
        }
    }

    pub(super) fn take_editor_recording_observation(&mut self) -> Option<Observation> {
        // A retired reader has no output requirement. A live reader is polled
        // alongside the existing ordered controller poll, under the same owner.
        self.poll_editor_recording(None);
        let Some(Recording::Active(active)) = self.editor_recording.as_mut() else {
            return None;
        };
        if active.has_completed_observation() && !active.drained {
            return None;
        }
        active.observation.take()
    }
}

impl Active {
    fn poll(&mut self, mut control: Option<&mut PublicationControl>) {
        if self.drained {
            return;
        }
        if self.has_completed_observation() {
            self.poll_completed_terminal(control);
            return;
        }
        if self.observation.is_some() {
            return;
        }
        if let Some(observation) = self.correlation.take_completion() {
            self.observation = Some(observation);
            self.cancel = true;
            return;
        }
        let target = self.correlation.target();
        let order_closed = self.closed
            || control.as_ref().is_some_and(|control| {
                control.status.live_native_parameter_source()
                    != Some((target.source, target.transport_epoch))
            });
        if self.cancel {
            if let Some(reader) = self.retired.as_ref() {
                reader.cancel_on_control();
            } else if let Some(control) = control.as_deref_mut() {
                control.cancel_native_source_editor_delivery_on_control(target);
            }
        }
        // Bound control work when the queue contains other parameters or
        // cancelled input. Never read past an unmatched quantum or full outbox.
        for _ in 0..4 {
            if self.pending.is_none() {
                let next = if let Some(reader) = self.retired.as_mut() {
                    reader.try_receipt_on_control()
                } else if let Some(control) = control.as_deref_mut() {
                    control.try_native_source_editor_delivery_on_control()
                } else if self.closed {
                    self.finish();
                    return;
                } else {
                    return;
                };
                match next {
                    Some(Receipt::Confirmed(receipt)) => self.pending = Some(receipt),
                    Some(Receipt::Aborted { target: ended, .. }) if ended == target => {
                        self.finish();
                        return;
                    }
                    Some(Receipt::Aborted { .. }) => self.cancel = true,
                    None => return,
                }
            }
            let Some(receipt) = self.pending else {
                continue;
            };
            match self.correlation.confirmed(receipt, order_closed) {
                Ok(Match::WaitingForOrder) => return,
                Ok(Match::Unrelated) => self.pending = None,
                Ok(Match::Observed(observation)) => {
                    self.pending = None;
                    self.observation = Some(observation);
                    self.cancel |= matches!(observation, Observation::Restored(_));
                    return;
                }
                Err(_) => {
                    self.pending = None;
                    self.cancel = true;
                }
            }
        }
    }

    fn has_completed_observation(&self) -> bool {
        matches!(
            self.observation,
            Some(Observation::Restored(_))
                | Some(Observation::Grouped(Vst3EditorGroupObservation {
                    receipt: Vst3EditorGroupReceipt::Finished(_),
                    ..
                }))
        )
    }

    // Session publishes a finished take immediately after its terminal. Keep
    // that terminal until reading the exact audio abort releases its admission.
    fn poll_completed_terminal(&mut self, mut control: Option<&mut PublicationControl>) {
        let target = self.correlation.target();
        if let Some(reader) = self.retired.as_ref() {
            reader.cancel_on_control();
        } else if let Some(control) = control.as_deref_mut() {
            control.cancel_native_source_editor_delivery_on_control(target);
        } else {
            return;
        }
        for _ in 0..4 {
            let next = if let Some(reader) = self.retired.as_mut() {
                reader.try_receipt_on_control()
            } else if let Some(control) = control.as_deref_mut() {
                control.try_native_source_editor_delivery_on_control()
            } else {
                None
            };
            match next {
                Some(Receipt::Aborted {
                    target: ended,
                    reason,
                }) if ended == target => {
                    self.finish_completed_terminal(reason);
                    return;
                }
                Some(_) => {}
                None => return,
            }
        }
    }

    fn finish_completed_terminal(&mut self, reason: AbortReason) {
        if reason != AbortReason::Cancelled {
            self.observation = match self.observation {
                Some(Observation::Restored(point)) => Some(Observation::Interrupted {
                    key: point.key,
                    context: point.context,
                    target: point.target,
                    source: point.source,
                    sequence: point.sequence,
                }),
                Some(Observation::Grouped(mut group)) => {
                    group.receipt = Vst3EditorGroupReceipt::Interrupted;
                    Some(Observation::Grouped(group))
                }
                other => other,
            };
        }
        self.drained = true;
    }

    fn finish(&mut self) {
        self.observation = self.correlation.abort();
        self.drained = true;
    }
}

#[allow(clippy::large_enum_variant)] // The existing Active already owns its allocation.
enum ActiveCorrelation {
    Single(Correlation),
    Group(Box<GroupControl>),
}

struct GroupControl {
    correlation: GroupCorrelation,
    identity: Vst3EditorGroupObservation,
    terminal_sent: bool,
}

impl GroupControl {
    fn observation(&self, receipt: Vst3EditorGroupReceipt) -> Observation {
        Observation::Grouped(Vst3EditorGroupObservation {
            receipt,
            ..self.identity
        })
    }
}

impl ActiveCorrelation {
    fn target(&self) -> Target {
        match self {
            Self::Single(correlation) => correlation.target(),
            Self::Group(group) => group.correlation.target(),
        }
    }

    fn key(&self) -> GestureKey {
        match self {
            Self::Single(correlation) => correlation.key(),
            Self::Group(group) => group.identity.key,
        }
    }

    fn ordered(&mut self, edit: ParameterEdit) -> Result<(), &'static str> {
        match self {
            Self::Single(correlation) => correlation.ordered(edit),
            Self::Group(group) => group
                .correlation
                .ordered(edit)
                .then_some(())
                .ok_or("vst3 group order requires interruption"),
        }
    }

    fn confirmed(&mut self, main: ConfirmedQuantum, closed: bool) -> Result<Match, &'static str> {
        match self {
            Self::Single(correlation) => correlation.confirmed(main, closed),
            Self::Group(group) if group.terminal_sent => Ok(Match::Unrelated),
            Self::Group(group) => match group.correlation.confirmed(main, closed)? {
                Quantum::Unrelated => Ok(Match::Unrelated),
                Quantum::WaitingForOrder => Ok(Match::WaitingForOrder),
                Quantum::Applied(rows) => Ok(Match::Observed(group.observation(
                    Vst3EditorGroupReceipt::Applied(Vst3EditorGroupRows::new(
                        rows,
                        main.main_performance_frame,
                    )?),
                ))),
            },
        }
    }

    fn take_completion(&mut self) -> Option<Observation> {
        let Self::Group(group) = self else {
            return None;
        };
        if group.terminal_sent {
            return None;
        }
        let clock = group.correlation.completed()?;
        group.terminal_sent = true;
        Some(group.observation(Vst3EditorGroupReceipt::Finished(clock)))
    }

    fn abort(&self) -> Option<Observation> {
        match self {
            Self::Single(correlation) => correlation.abort(),
            Self::Group(group) => (!group.terminal_sent)
                .then(|| group.observation(Vst3EditorGroupReceipt::Interrupted)),
        }
    }
}
