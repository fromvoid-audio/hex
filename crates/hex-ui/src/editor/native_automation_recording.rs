//! GPUI owns one pointer gesture and a bounded delivery batch. Session owns
//! the draft and history. The callback owns actual application and restoration.
//! Refused submissions leave the original batch and any unread endpoint or
//! retired receipts with this recorder. Cancel stops new input; neither a
//! callback terminal nor an unavailable worker resolves that recovery custody.
use super::EditorView;
use hex_audio::{
    native_parameter_input::{
        AbortReason, MAX_PENDING_RECEIPTS, MAX_PENDING_VALUE_RECEIPTS, Receipt, Target,
    },
    prepared::{Client, RetiredScalarReceiptBatch, RetiredScalarReceipts, ScalarReceipt},
};
use hex_engine::{NativeParameterApplication, NativeTransientValue};
use hex_lattice_contract::{DeviceAutomationTarget, HostGain, ParameterValue};
use hex_project::DeviceId;
use hex_session::{
    device_gesture::{
        GestureClient, GestureKey, GesturePhase, GestureTarget, PluginParameterOrigin,
    },
    service::{
        DeviceAutomationTakeAction as Action, DeviceAutomationTakeProgress as Progress,
        DeviceAutomationTakeScope, Event, MAX_NATIVE_RECOVERY_CONTINUATION_EVENTS,
        NativeRecoveryContinuation, NativeRecoveryTerminal, Request,
        automation_draft::{DraftProvenance, EVENTS_PER_BATCH},
    },
};
use hex_widgets::NumericEventPhase;
use std::sync::{Arc, atomic::Ordering::Acquire};

// A refused writer batch can coexist with an entire endpoint window. Reserve
// room to read the terminal even when both sets of applied values are full.
const RETAINED_BATCH_CAPACITY: usize = EVENTS_PER_BATCH + MAX_PENDING_RECEIPTS;
const _: () = assert!(
    EVENTS_PER_BATCH + MAX_PENDING_VALUE_RECEIPTS <= MAX_NATIVE_RECOVERY_CONTINUATION_EVENTS
);

#[derive(Clone, Copy, PartialEq, Eq)]
struct Arm {
    generation: u64,
    revision: u64,
    target: ArmTarget,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArmTarget {
    Native {
        device: DeviceId,
        kind: DeviceAutomationTarget,
    },
    Vst3 {
        device: DeviceId,
        class: [u8; 16],
        parameter: u32,
    },
}

#[derive(Default)]
pub(super) struct Recorder {
    arm: Option<Arm>,
    touch: Option<Touch>,
    serial: u64,
    request: u64,
    plugin_take: Option<(GestureKey, Arm)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Starting,
    Delivering,
    Sealing,
    Orphaned,
    Retained,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Submission {
    Begin,
    Append,
    Terminal,
    Poll,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    Release,
    Cancel,
}

struct Touch {
    gesture: GestureTarget,
    audio: Target,
    scope: DeviceAutomationTakeScope,
    phase: Phase,
    latest: ParameterValue,
    pending: Option<NativeTransientValue>,
    end: Option<End>,
    end_sent: Option<End>,
    issued_sequence: u64,
    delivered_sequence: u64,
    in_flight: Option<(u64, Submission)>,
    batch: Vec<(u64, NativeParameterApplication)>,
    terminal: Option<Action>,
    retired: Option<RetiredScalarReceipts>,
    retired_batch: Option<Box<RetiredScalarReceiptBatch>>,
    retired_read: usize,
}

impl Touch {
    fn key(&self) -> GestureKey {
        GestureKey {
            generation: self.scope.stamp.project_generation,
            token: self.audio.gesture_serial,
        }
    }

    fn receipt(&mut self, receipt: Receipt) {
        if receipt.target() != self.audio {
            return;
        }
        match receipt {
            Receipt::Applied {
                sequence,
                application,
                ..
            } => {
                self.delivered_sequence = self.delivered_sequence.max(sequence);
                self.batch.push((sequence, application));
            }
            Receipt::Superseded { .. } => {}
            Receipt::Finished { application, .. } => {
                self.terminal = Some(Action::Finish(application))
            }
            Receipt::Aborted { reason, .. } => {
                self.terminal = Some(Action::Abort(abort_reason(reason)))
            }
        }
    }

    /// Drain one bounded source window. Return true only if the source had
    /// no further receipt before the current work or storage limit.
    fn drain_receipts(&mut self, mut next: impl FnMut() -> Option<ScalarReceipt>) -> bool {
        let (capacity, steps) = if self.phase == Phase::Orphaned {
            (RETAINED_BATCH_CAPACITY, MAX_PENDING_RECEIPTS)
        } else {
            (EVENTS_PER_BATCH, EVENTS_PER_BATCH)
        };
        for _ in 0..steps {
            if self.batch.len() >= capacity || self.terminal.is_some() {
                break;
            }
            match next() {
                Some(ScalarReceipt::NativeParameter(receipt)) => self.receipt(receipt),
                Some(ScalarReceipt::Gain(_)) => {}
                None => return true,
            }
        }
        false
    }

    fn drain(&mut self, client: Option<&mut Client>) {
        if self.terminal.is_some()
            || self
                .in_flight
                .is_some_and(|(_, kind)| kind == Submission::Append)
        {
            return;
        }
        if let Some(client) = client {
            self.drain_receipts(|| {
                client
                    .native_parameter_receipt()
                    .map(ScalarReceipt::NativeParameter)
            });
            return;
        }
        if self.retired_batch.is_none()
            && let Some(result) = self
                .retired
                .as_mut()
                .and_then(RetiredScalarReceipts::try_take)
        {
            self.retired = None;
            match result {
                Ok(batch) => self.retired_batch = Some(Box::new(batch)),
                Err(_) => self.terminal = Some(Action::Interrupt),
            }
        }
        if let Some(batch) = self.retired_batch.take() {
            let mut read = 0;
            let exhausted = {
                let mut receipts = batch.receipts().skip(self.retired_read);
                self.drain_receipts(|| {
                    let receipt = receipts.next()?;
                    read += 1;
                    Some(receipt)
                })
            };
            self.retired_read += read;
            if exhausted && self.terminal.is_none() {
                self.terminal = Some(Action::Interrupt);
            }
            if self.terminal.is_none() {
                self.retired_batch = Some(batch);
            }
        } else if self.retired.is_none() && self.terminal.is_none() {
            self.terminal = Some(Action::Interrupt);
        }
    }

    fn next_submission(&self) -> Option<(Submission, Action)> {
        if self.in_flight.is_some() {
            return None;
        }
        match self.phase {
            Phase::Starting | Phase::Orphaned | Phase::Retained => None,
            Phase::Delivering if !self.batch.is_empty() => Some((
                Submission::Append,
                Action::Append(Arc::from(self.batch.as_slice())),
            )),
            Phase::Delivering => self.terminal.as_ref().map(|terminal| {
                (
                    Submission::Terminal,
                    match *terminal {
                        Action::Finish(application) => Action::Finish(application),
                        Action::Abort(reason) => Action::Abort(reason),
                        _ => Action::Interrupt,
                    },
                )
            }),
            Phase::Sealing => Some((Submission::Poll, Action::Poll)),
        }
    }

    fn retain_after_refusal(&mut self) {
        self.phase = if self.terminal.is_some() {
            Phase::Retained
        } else {
            Phase::Orphaned
        };
        self.end = Some(End::Cancel);
        self.in_flight = None;
    }

    fn release_ready(&self) -> bool {
        self.end == Some(End::Release)
            && self.end_sent.is_none()
            && self.issued_sequence != 0
            && self.delivered_sequence == self.issued_sequence
    }

    fn drive_audio(&mut self, client: &mut Client) -> Result<(), &'static str> {
        if self.terminal.is_some() {
            return Ok(());
        }
        if self.end == Some(End::Cancel) {
            if self.end_sent != Some(End::Cancel) {
                client.native_parameter_cancel()?;
                self.end_sent = Some(End::Cancel);
            }
            return Ok(());
        }
        if let Some(value) = self.pending {
            match client.native_parameter_change(value) {
                Ok(()) => {
                    self.issued_sequence = self
                        .issued_sequence
                        .checked_add(1)
                        .ok_or("automation input sequence limit reached")?;
                    self.pending = None;
                }
                Err(
                    "native parameter input is applying; retry the value"
                    | "native parameter input command queue is full",
                ) => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        // A Clear admitted in the same callback as the final Set would
        // supersede it before the processor ever hears the intended value.
        if self.release_ready() {
            client.native_parameter_release()?;
            self.end_sent = Some(End::Release);
        }
        Ok(())
    }
}

pub(super) fn automation_target(
    parameter: hex_lattice_contract::ParameterId,
) -> DeviceAutomationTarget {
    if parameter == hex_engine::DEVICE_INPUT_GAIN.id {
        DeviceAutomationTarget::HostInputGain
    } else if parameter == hex_engine::DEVICE_OUTPUT_GAIN.id {
        DeviceAutomationTarget::HostOutputGain
    } else {
        DeviceAutomationTarget::Parameter(parameter)
    }
}

fn input_value(
    kind: DeviceAutomationTarget,
    value: ParameterValue,
) -> Result<NativeTransientValue, &'static str> {
    match (kind, value) {
        (DeviceAutomationTarget::Parameter(id), value) => {
            Ok(NativeTransientValue::Parameter { id, value })
        }
        (DeviceAutomationTarget::HostInputGain, ParameterValue::Float(db)) => HostGain::from_db(db)
            .map(NativeTransientValue::HostInputGain)
            .map_err(|_| "invalid input gain"),
        (DeviceAutomationTarget::HostOutputGain, ParameterValue::Float(db)) => {
            HostGain::from_db(db)
                .map(NativeTransientValue::HostOutputGain)
                .map_err(|_| "invalid output gain")
        }
        _ => Err("unsupported native automation control"),
    }
}

fn abort_reason(reason: AbortReason) -> hex_session::service::automation_draft::DraftAbortReason {
    use hex_session::service::automation_draft::DraftAbortReason as Draft;
    match reason {
        AbortReason::SourceChanged => Draft::SourceChanged,
        AbortReason::TargetUnavailable => Draft::TargetUnavailable,
        AbortReason::TransportChanged => Draft::TransportChanged,
        AbortReason::OutputFailed => Draft::OutputFailed,
        AbortReason::UnsupportedOutput => Draft::UnsupportedOutput,
        AbortReason::Cancelled => Draft::Cancelled,
        AbortReason::ClockExhausted => Draft::ClockExhausted,
    }
}

impl Recorder {
    pub(super) fn reserve_request(&mut self) -> Result<u64, &'static str> {
        self.request = self
            .request
            .checked_add(1)
            .ok_or("automation request limit reached")?;
        Ok(self.request)
    }
    pub(super) fn busy(&self) -> bool {
        self.touch.is_some()
    }

    fn begin_plugin(
        &mut self,
        client: &mut GestureClient,
        origin: PluginParameterOrigin,
        class: Option<[u8; 16]>,
        playing: bool,
        desired_playing: bool,
    ) -> Result<GestureKey, &'static str> {
        let armed = self.arm.filter(|arm| {
            arm.generation == origin.context.project_generation
                && class.is_some_and(|class| {
                    arm.target
                        == ArmTarget::Vst3 {
                            device: origin.device,
                            class,
                            parameter: origin.parameter,
                        }
                })
        });
        let Some(arm) = armed else {
            return client.begin_plugin(origin);
        };
        if playing != desired_playing {
            return Err("wait for playback to settle before recording automation");
        }
        if !playing {
            return client.begin_plugin(origin);
        }
        if arm.revision != origin.context.editor_revision {
            return Err("automation arm changed · arm this parameter again");
        }
        let key = client.begin_plugin_recording(origin)?;
        self.plugin_take = Some((key, arm));
        Ok(key)
    }

    pub(super) fn owns(&self, target: GestureTarget, key: Option<GestureKey>) -> bool {
        self.touch.as_ref().is_some_and(|touch| {
            touch.gesture == target && Some(touch.key()) == key && touch.end.is_none()
        })
    }

    pub(super) fn value(&self, target: GestureTarget) -> Option<ParameterValue> {
        self.touch
            .as_ref()
            .filter(|touch| {
                touch.gesture == target && touch.phase != Phase::Sealing && touch.terminal.is_none()
            })
            .map(|touch| touch.latest)
    }
}

impl EditorView {
    pub(super) fn retained_native_recovery_scope(&self) -> Option<DeviceAutomationTakeScope> {
        self.native_automation_recorder
            .touch
            .as_ref()
            .filter(|touch| touch.phase == Phase::Retained)
            .map(|touch| touch.scope)
    }

    pub(super) fn native_recovery_continuation(
        &self,
        scope: DeviceAutomationTakeScope,
    ) -> Option<NativeRecoveryContinuation> {
        let touch = self
            .native_automation_recorder
            .touch
            .as_ref()
            .filter(|touch| touch.phase == Phase::Retained && touch.scope == scope)?;
        let terminal = match touch.terminal.as_ref() {
            Some(Action::Finish(application)) => NativeRecoveryTerminal::Restored(*application),
            Some(Action::Abort(reason)) => NativeRecoveryTerminal::Aborted(*reason),
            // No caller terminal means it was already admitted, or the
            // endpoint reported no terminal. Session preserves an admitted
            // original terminal when it combines the writer and caller tail.
            Some(Action::Interrupt) | None => NativeRecoveryTerminal::Interrupted,
            _ => return None,
        };
        Some(NativeRecoveryContinuation {
            applications: Arc::from(touch.batch.as_slice()),
            terminal,
        })
    }

    pub(super) fn release_native_recovery(&mut self, scope: DeviceAutomationTakeScope) {
        if self.retained_native_recovery_scope() == Some(scope) {
            self.native_automation_recorder.touch = None;
        }
    }

    pub(super) fn native_automation_arm_selected(
        &self,
        device: DeviceId,
        kind: DeviceAutomationTarget,
    ) -> bool {
        self.native_automation_recorder.arm.is_some_and(|arm| {
            arm.target == (ArmTarget::Native { device, kind })
                && arm.generation == self.project_generation
                && self
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == arm.revision)
        })
    }

    pub(super) fn toggle_native_automation_arm(
        &mut self,
        device: DeviceId,
        kind: DeviceAutomationTarget,
    ) {
        if self.native_automation_recorder.busy()
            || self.gain_recorder.blocks_edits()
            || self.midi_recorder.blocks_edits()
            || self.device_gesture_pending()
            || self.edit_pending()
        {
            self.error = Some("finish the current edit or take before arming automation".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if self.audio.is_none()
            || self.document_audio_mode != hex_session::DocumentAudioMode::Resident
        {
            self.error = Some("automation recording requires ordinary native playback".into());
            return;
        }
        if hex_engine::native_parameter_input_target(&state.project, device, kind).is_err() {
            self.error = Some("this parameter cannot record automation".into());
            return;
        }
        let arm = Arm {
            generation: self.project_generation,
            revision: state.revision,
            target: ArmTarget::Native { device, kind },
        };
        let selected = self.native_automation_recorder.arm != Some(arm);
        if self.enqueue(Request::ArmNativeParameterRecording {
            project_generation: arm.generation,
            revision: arm.revision,
            target: selected.then_some((device, kind)),
        }) {
            self.native_automation_recorder.arm = selected.then_some(arm);
        }
    }

    pub(super) fn vst3_automation_arm_selected(
        &self,
        device: DeviceId,
        class: [u8; 16],
        parameter: u32,
    ) -> bool {
        self.native_automation_recorder.arm.is_some_and(|arm| {
            arm.target
                == (ArmTarget::Vst3 {
                    device,
                    class,
                    parameter,
                })
                && arm.generation == self.project_generation
                && self
                    .state
                    .as_ref()
                    .is_some_and(|state| state.revision == arm.revision)
        })
    }

    pub(super) fn toggle_vst3_automation_arm(
        &mut self,
        device: DeviceId,
        class: [u8; 16],
        parameter: u32,
    ) {
        if self.automation_close_preflight.frozen()
            || self.automation_take_recovery.unresolved()
            || self.native_automation_recorder.busy()
            || self.gain_recorder.blocks_edits()
            || self.midi_recorder.blocks_edits()
            || self.device_gesture_pending()
            || self.edit_pending()
        {
            self.error = Some("finish the current edit or take before arming automation".into());
            return;
        }
        if self.audio.is_none()
            || self.document_audio_mode != hex_session::DocumentAudioMode::Resident
        {
            self.error = Some("automation recording requires ordinary playback".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let arm = Arm {
            generation: self.project_generation,
            revision: state.revision,
            target: ArmTarget::Vst3 {
                device,
                class,
                parameter,
            },
        };
        if self.native_automation_recorder.arm.is_some_and(|current| {
            matches!(current.target, ArmTarget::Native { .. })
                && current.generation == arm.generation
                && current.revision == arm.revision
        }) && !self.enqueue(Request::ArmNativeParameterRecording {
            project_generation: arm.generation,
            revision: arm.revision,
            target: None,
        }) {
            return;
        }
        self.native_automation_recorder.arm =
            (self.native_automation_recorder.arm != Some(arm)).then_some(arm);
    }

    pub(super) fn begin_vst3_parameter_gesture(
        &mut self,
        origin: PluginParameterOrigin,
    ) -> Result<GestureKey, &'static str> {
        if self.automation_close_preflight.frozen() {
            return Err("close is pending · automation recording is unavailable");
        }
        if self.native_automation_recorder.busy() || self.automation_take_recovery.unresolved() {
            return Err("finish or recover the current automation take first");
        }
        let class = self
            .state
            .as_ref()
            .and_then(|state| state.project.device(origin.device))
            .and_then(|device| hex_session::service::vst3_automation_class(device.state()));
        let playing = self
            .audio
            .as_ref()
            .and_then(|audio| audio.status())
            .is_some_and(|status| status.playing.load(Acquire));
        let desired_playing = self.play_intent.desired_playing(playing);
        self.native_automation_recorder.begin_plugin(
            &mut self.device_gesture,
            origin,
            class,
            playing,
            desired_playing,
        )
    }

    pub(super) fn owns_vst3_recording(&self, key: GestureKey) -> bool {
        self.native_automation_recorder
            .plugin_take
            .is_some_and(|(recording, _)| recording == key)
    }

    pub(super) fn observe_vst3_automation_arm(
        &mut self,
        project_replaced: bool,
        ack: Option<&hex_session::device_gesture::GestureAck>,
        active: bool,
        revision: u64,
    ) {
        let Some((key, mut arm)) = self.native_automation_recorder.plugin_take else {
            return;
        };
        if project_replaced {
            self.native_automation_recorder.plugin_take = None;
            return;
        }
        let Some(ack) = ack.filter(|ack| ack.key == key) else {
            return;
        };
        if active {
            return;
        }
        self.native_automation_recorder.plugin_take = None;
        if ack.accepted
            && ack.phase == GesturePhase::Commit
            && arm.revision.checked_add(1) == Some(revision)
            && self.native_automation_recorder.arm == Some(arm)
        {
            arm.revision = revision;
            self.native_automation_recorder.arm = Some(arm);
        }
    }

    pub(super) fn begin_native_automation_touch(
        &mut self,
        gesture: GestureTarget,
        value: ParameterValue,
    ) -> Result<Option<GestureKey>, &'static str> {
        if self.automation_close_preflight.frozen() {
            return Err("close is pending · automation recording is unavailable");
        }
        if self.native_automation_recorder.busy() {
            return Err("finish the current automation touch first");
        }
        let GestureTarget::DeviceParameter { device, parameter } = gesture else {
            return Ok(None);
        };
        let kind = automation_target(parameter);
        if !self.native_automation_arm_selected(device, kind) {
            return Ok(None);
        }
        let audio = self
            .audio
            .as_ref()
            .ok_or("automation recording output is unavailable")?;
        let status = audio
            .status()
            .ok_or("automation recording output is unavailable")?;
        if !status.playing.load(Acquire) {
            return Ok(None);
        }
        if self.device_gesture_pending()
            || self.gain_recorder.blocks_edits()
            || self.midi_recorder.blocks_edits()
        {
            return Err("another gesture owns the current source");
        }
        let state = self
            .state
            .as_ref()
            .ok_or("automation source is unavailable")?;
        let input = hex_engine::native_parameter_input_target(&state.project, device, kind)
            .map_err(|error| error.0)?;
        let source = status
            .recording_source()
            .filter(|source| {
                source.context.project_generation == self.project_generation
                    && source.context.editor_revision == state.revision
            })
            .ok_or("automation arm is still applying")?;
        let applied = self
            .audible_playback
            .as_ref()
            .filter(|candidate| {
                candidate.audio_revision == source.audio_revision
                    && candidate.project_generation == self.project_generation
            })
            .and_then(|candidate| candidate.observation.as_ref());
        if applied.and_then(|observation| observation.native_parameter_input_target())
            != Some(input)
        {
            return Err("automation arm is still applying");
        }
        let transport_epoch = status.transport_epoch.load(Acquire);
        if transport_epoch == 0 {
            return Err("automation transport is not ready");
        }
        let serial = self
            .native_automation_recorder
            .serial
            .checked_add(1)
            .ok_or("automation gesture limit reached")?;
        let request = self
            .native_automation_recorder
            .request
            .checked_add(1)
            .ok_or("automation request limit reached")?;
        let pending = input_value(kind, value)?;
        let scope = DeviceAutomationTakeScope {
            target: input.into(),
            stamp: DraftProvenance {
                sample_rate: audio.sample_rate,
                output_identity: source.output_identity,
                project_generation: self.project_generation,
                editor_revision: state.revision,
                audio_revision: source.audio_revision,
                transport_epoch,
                gesture_serial: serial,
            },
        };
        let touch = Touch {
            gesture,
            audio: Target {
                source,
                input,
                transport_epoch,
                gesture_serial: serial,
            },
            scope,
            phase: Phase::Starting,
            latest: value,
            pending: Some(pending),
            end: None,
            end_sent: None,
            issued_sequence: 0,
            delivered_sequence: 0,
            in_flight: Some((request, Submission::Begin)),
            batch: Vec::with_capacity(RETAINED_BATCH_CAPACITY),
            terminal: None,
            retired: None,
            retired_batch: None,
            retired_read: 0,
        };
        if !self.enqueue(Request::DeviceAutomationTake {
            request_id: request,
            scope,
            action: Action::Begin,
        }) {
            return Err("automation take could not start");
        }
        let key = touch.key();
        self.native_automation_recorder.serial = serial;
        self.native_automation_recorder.request = request;
        self.native_automation_recorder.touch = Some(touch);
        Ok(Some(key))
    }

    pub(super) fn update_native_automation_touch(
        &mut self,
        phase: NumericEventPhase,
        value: ParameterValue,
    ) -> Result<(), &'static str> {
        let Some(touch) = self.native_automation_recorder.touch.as_mut() else {
            return Ok(());
        };
        if phase == NumericEventPhase::Cancel {
            touch.end = Some(End::Cancel);
            return Ok(());
        }
        if touch.latest != value {
            touch.pending = Some(input_value(touch.audio.input.target(), value)?);
            touch.latest = value;
        }
        if phase == NumericEventPhase::Commit {
            touch.end = Some(End::Release);
        }
        Ok(())
    }

    pub(super) fn cancel_native_automation_touch(&mut self) {
        if let Some(touch) = self.native_automation_recorder.touch.as_mut()
            && touch.phase != Phase::Sealing
        {
            touch.end = Some(End::Cancel);
        }
    }

    pub(super) fn retire_native_automation_client(&mut self, client: &mut Client) -> bool {
        let Some(touch) = self
            .native_automation_recorder
            .touch
            .as_mut()
            .filter(|touch| {
                matches!(touch.phase, Phase::Delivering | Phase::Orphaned)
                    && client.native_parameter_target() == Some(touch.audio)
            })
        else {
            return false;
        };
        touch.retired = client.close_retaining_native_parameter_receipts();
        true
    }

    pub(super) fn native_automation_take_completion(&mut self, event: &Event) {
        let Some(completion) = event.device_automation_take_complete.as_ref() else {
            return;
        };
        let Some(mut touch) = self.native_automation_recorder.touch.take() else {
            return;
        };
        let Some((id, kind)) = touch.in_flight else {
            self.native_automation_recorder.touch = Some(touch);
            return;
        };
        if id != completion.request_id {
            self.native_automation_recorder.touch = Some(touch);
            return;
        }
        touch.in_flight = None;
        match (kind, &completion.progress) {
            (Submission::Begin, Progress::Started) => {
                touch.phase = Phase::Delivering;
                let accepted = touch.end != Some(End::Cancel)
                    && touch.pending.take().is_some_and(|value| {
                        self.audio.as_mut().is_some_and(|audio| {
                            audio
                                .control
                                .native_parameter_begin(touch.audio, value)
                                .is_ok()
                        })
                    });
                if accepted {
                    touch.issued_sequence = 1;
                } else {
                    if touch.end != Some(End::Cancel) {
                        self.error = Some("the armed audio source did not admit this touch".into());
                    }
                    touch.terminal = Some(Action::Interrupt);
                }
            }
            (Submission::Begin, _) => return,
            (Submission::Append, Progress::Appended) => touch.batch.clear(),
            (Submission::Append, Progress::Backpressured) => {}
            (Submission::Append, _) => touch.retain_after_refusal(),
            (Submission::Terminal, Progress::Sealing) => {
                touch.terminal = None;
                touch.phase = Phase::Sealing;
            }
            (Submission::Terminal, _) => touch.retain_after_refusal(),
            (Submission::Poll, Progress::Pending) => {}
            (Submission::Poll, Progress::Committed) => {
                if let Some(arm) = self.native_automation_recorder.arm.as_mut()
                    && arm.generation == touch.scope.stamp.project_generation
                    && arm.revision == touch.scope.stamp.editor_revision
                    && arm.target
                        == (ArmTarget::Native {
                            device: touch.audio.input.device(),
                            kind: touch.audio.input.target(),
                        })
                    && arm.revision.checked_add(1) == Some(event.state.revision)
                {
                    arm.revision = event.state.revision;
                }
                return;
            }
            (Submission::Poll, Progress::NativeWriterRetained { path, reason }) => {
                // Sealing already retired the endpoint. The writer owns the
                // admitted terminal; there are no more caller receipts.
                touch.phase = Phase::Retained;
                self.error = Some(
                    format!(
                        "{reason} · automation take retained at {}",
                        hex_session::service::text::verbatim(path.display())
                    )
                    .into(),
                );
            }
            (Submission::Poll, Progress::Retained(path)) => {
                if event.error.is_none() {
                    self.error = Some(
                        format!(
                            "automation take retained at {}",
                            hex_session::service::text::verbatim(path.display())
                        )
                        .into(),
                    );
                }
                return;
            }
            (Submission::Poll, _) => return,
        }
        self.native_automation_recorder.touch = Some(touch);
    }

    pub(super) fn native_automation_request_not_submitted(
        &mut self,
        request: &Request,
        worker_closed: bool,
    ) {
        let Request::DeviceAutomationTake { request_id, .. } = request else {
            return;
        };
        let Some(touch) = self
            .native_automation_recorder
            .touch
            .as_mut()
            .filter(|touch| touch.in_flight.is_some_and(|(id, _)| id == *request_id))
        else {
            return;
        };
        if touch.phase == Phase::Starting {
            self.native_automation_recorder.touch = None;
        } else if worker_closed {
            // Submission refusal is not a writer receipt. Retain originals
            // while releasing the actual endpoint and awaiting recovery.
            touch.retain_after_refusal();
        } else {
            // The request never reached the worker. Retry the exact batch or
            // terminal at normal poll cadence when queue pressure clears.
            touch.in_flight = None;
        }
    }

    pub(super) fn tick_native_automation_recording(&mut self) -> bool {
        let Some(mut touch) = self.native_automation_recorder.touch.take() else {
            return false;
        };
        let mut changed = false;
        if matches!(touch.phase, Phase::Delivering | Phase::Orphaned) {
            let client = self
                .audio
                .as_mut()
                .map(|audio| &mut audio.control)
                .filter(|client| {
                    client.output_identity() == Some(touch.audio.source.output_identity)
                        && client.status().is_some()
                });
            if let Some(client) = client {
                touch.drain(Some(client));
                if let Err(error) = touch.drive_audio(client) {
                    self.error = Some(error.into());
                    if client.native_parameter_target() == Some(touch.audio) {
                        touch.end = Some(End::Cancel);
                    } else {
                        touch.terminal = Some(Action::Interrupt);
                    }
                    changed = true;
                }
            } else {
                touch.drain(None);
            }
        }
        if touch.phase == Phase::Orphaned && touch.terminal.is_some() {
            touch.phase = Phase::Retained;
            changed = true;
        }
        let proposal = touch.next_submission();
        let scope = touch.scope;
        self.native_automation_recorder.touch = Some(touch);
        if let Some((kind, action)) = proposal
            && let Some(id) = self.native_automation_recorder.request.checked_add(1)
        {
            if let Some(touch) = self.native_automation_recorder.touch.as_mut() {
                touch.in_flight = Some((id, kind));
            }
            self.native_automation_recorder.request = id;
            if self.enqueue(Request::DeviceAutomationTake {
                request_id: id,
                scope,
                action,
            }) {
                changed = true;
            } else if let Some(touch) = self.native_automation_recorder.touch.as_mut() {
                touch.in_flight = None;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_audio::prepared::AppliedAudioSource;
    use hex_engine::{AudioSourceContext, NativeTransientOperation};
    use hex_lattice_contract::DeviceTypeId;
    use hex_project::{Command, Created, Editor, Project};

    fn touch() -> Touch {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let state =
            hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                .unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("utility fixture did not create a device");
        };
        let kind = DeviceAutomationTarget::HostOutputGain;
        let input =
            hex_engine::native_parameter_input_target(editor.project(), device, kind).unwrap();
        let audio = Target {
            input,
            source: AppliedAudioSource {
                context: AudioSourceContext {
                    project_generation: 1,
                    editor_revision: 1,
                },
                audio_revision: 2,
                output_identity: 4,
            },
            transport_epoch: 3,
            gesture_serial: 5,
        };
        Touch {
            gesture: GestureTarget::DeviceParameter {
                device,
                parameter: hex_engine::DEVICE_OUTPUT_GAIN.id,
            },
            audio,
            scope: DeviceAutomationTakeScope {
                target: input.into(),
                stamp: DraftProvenance {
                    sample_rate: 48_000,
                    output_identity: 4,
                    project_generation: 1,
                    editor_revision: 1,
                    audio_revision: 2,
                    transport_epoch: 3,
                    gesture_serial: 5,
                },
            },
            phase: Phase::Starting,
            latest: ParameterValue::Float(-6.0),
            pending: None,
            end: None,
            end_sent: None,
            issued_sequence: 0,
            delivered_sequence: 0,
            in_flight: Some((1, Submission::Begin)),
            batch: Vec::with_capacity(RETAINED_BATCH_CAPACITY),
            terminal: None,
            retired: None,
            retired_batch: None,
            retired_read: 0,
        }
    }

    #[test]
    fn only_an_exact_armed_playing_vst3_target_uses_the_recording_gesture() {
        use hex_session::device_gesture::{GestureRequest, PluginParameterValue};

        let device = touch().audio.input.device();
        let class = [9; 16];
        let origin = PluginParameterOrigin {
            context: AudioSourceContext {
                project_generation: 1,
                editor_revision: 3,
            },
            device,
            parameter: 0,
            page_first: 0,
            metadata_revision: 4,
            initial: PluginParameterValue::new(0.25).unwrap(),
        };
        let arm = Arm {
            generation: 1,
            revision: 3,
            target: ArmTarget::Vst3 {
                device,
                class,
                parameter: 0,
            },
        };
        for (armed, selected_class, playing, desired, expected) in [
            (Some(arm), Some(class), true, true, Ok(true)),
            (Some(arm), Some(class), false, false, Ok(false)),
            (None, Some(class), true, true, Ok(false)),
            (Some(arm), Some([8; 16]), true, true, Ok(false)),
            (
                Some(Arm {
                    generation: 0,
                    ..arm
                }),
                Some(class),
                true,
                true,
                Ok(false),
            ),
            (
                Some(Arm { revision: 2, ..arm }),
                Some(class),
                true,
                true,
                Err("automation arm changed · arm this parameter again"),
            ),
            (
                Some(arm),
                Some(class),
                true,
                false,
                Err("wait for playback to settle before recording automation"),
            ),
        ] {
            let mut recorder = Recorder {
                arm: armed,
                ..Recorder::default()
            };
            let mut client = GestureClient::default();
            let result =
                recorder.begin_plugin(&mut client, origin, selected_class, playing, desired);
            match expected {
                Ok(expected) => {
                    let key = result.unwrap();
                    assert_eq!(
                        client.next_request(),
                        Some(GestureRequest::BeginPlugin {
                            key,
                            origin,
                            recording: expected,
                        })
                    );
                    assert_eq!(
                        recorder.plugin_take.map(|(owner, _)| owner),
                        expected.then_some(key)
                    );
                }
                Err(reason) => {
                    assert_eq!(result, Err(reason));
                    assert!(
                        client.next_request().is_none(),
                        "refusal must not enqueue a scalar edit"
                    );
                }
            }
        }
    }

    #[test]
    fn delivered_batch_retries_in_order_before_the_actual_restoration() {
        let mut touch = touch();
        assert!(touch.next_submission().is_none());
        let application = NativeParameterApplication {
            identity: touch.audio.input.identity(),
            operation: NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(
                HostGain::ZERO,
            )),
            arrangement_frame: 19,
            performance_frame: 23,
            loop_pass: 0,
            remaining_latency_frames: 4,
        };
        let restored = NativeParameterApplication {
            operation: NativeTransientOperation::Clear(DeviceAutomationTarget::HostOutputGain),
            arrangement_frame: 23,
            performance_frame: 27,
            ..application
        };
        touch.phase = Phase::Delivering;
        touch.in_flight = None;
        touch.end = Some(End::Release);
        touch.issued_sequence = 2;
        assert!(!touch.release_ready());
        touch.receipt(Receipt::Applied {
            target: touch.audio,
            sequence: 1,
            application,
        });
        assert!(!touch.release_ready());
        touch.receipt(Receipt::Superseded {
            target: touch.audio,
            sequence: 2,
        });
        assert!(!touch.release_ready());
        touch.issued_sequence = 3;
        touch.receipt(Receipt::Applied {
            target: touch.audio,
            sequence: 3,
            application,
        });
        assert!(touch.release_ready());
        touch.receipt(Receipt::Finished {
            target: touch.audio,
            application: restored,
        });
        let Some((Submission::Append, Action::Append(batch))) = touch.next_submission() else {
            panic!("delivery must be sent before the restoration");
        };
        assert_eq!(&*batch, &[(1, application), (3, application)]);
        touch.in_flight = Some((2, Submission::Append));
        assert!(touch.next_submission().is_none());
        touch.in_flight = None;
        let Some((Submission::Append, Action::Append(retry))) = touch.next_submission() else {
            panic!("backpressure must retain the original batch");
        };
        assert_eq!(retry, batch);
        touch.batch.clear();
        assert!(
            matches!(touch.next_submission(), Some((Submission::Terminal, Action::Finish(value))) if value == restored)
        );
    }

    #[test]
    fn full_refused_batch_retains_entire_endpoint_window_and_restoration() {
        let mut touch = touch();
        touch.phase = Phase::Delivering;
        touch.in_flight = None;
        let target = touch.audio;
        let applied = |sequence| NativeParameterApplication {
            identity: target.input.identity(),
            operation: NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(
                HostGain::ZERO,
            )),
            arrangement_frame: sequence + 1,
            performance_frame: sequence + 5,
            loop_pass: 2,
            remaining_latency_frames: 4,
        };
        let end = (EVENTS_PER_BATCH + MAX_PENDING_VALUE_RECEIPTS) as u64;
        let restored = NativeParameterApplication {
            operation: NativeTransientOperation::Clear(DeviceAutomationTarget::HostOutputGain),
            ..applied(end + 1)
        };
        let mut waiting = (1..=end)
            .map(|sequence| Receipt::Applied {
                target,
                sequence,
                application: applied(sequence),
            })
            .chain(std::iter::once(Receipt::Finished {
                target,
                application: restored,
            }))
            .map(ScalarReceipt::NativeParameter);

        touch.drain_receipts(|| waiting.next());
        assert_eq!(touch.batch.len(), EVENTS_PER_BATCH);
        assert!(touch.terminal.is_none());
        touch.in_flight = Some((2, Submission::Append));
        touch.retain_after_refusal();
        touch.drain_receipts(|| waiting.next());
        assert_eq!(touch.batch.len(), end as usize);
        assert_eq!(
            touch.batch,
            (1..=end).map(|seq| (seq, applied(seq))).collect::<Vec<_>>()
        );
        assert!(matches!(
            touch.terminal,
            Some(Action::Finish(application))
                if application == restored
        ));
        assert!(waiting.next().is_none());
        assert!(touch.next_submission().is_none());
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn refused_and_lost_submissions_keep_delivered_receipts_until_recovery(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{AnyWindowHandle, AppContext, Empty};
        use hex_session::service::{DeviceAutomationTakeCompletion, Service};
        use std::time::Instant;

        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        let deadline = crate::test_wait::hang_deadline();
        let mut event = loop {
            if let Ok(event) = service.try_event() {
                break event;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        event.device_automation_take_complete = Some(DeviceAutomationTakeCompletion {
            request_id: 2,
            progress: Progress::Refused,
        });
        let visual = cx.add_window(|_, _| Empty);
        let handle: AnyWindowHandle = *visual;
        cx.update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline fixture".into()))
            });
            for worker_closed in [false, true] {
                editor.update(cx, |view, cx| {
                    let mut capture = touch();
                    let application = NativeParameterApplication {
                        identity: capture.audio.input.identity(),
                        operation: NativeTransientOperation::Set(
                            NativeTransientValue::HostOutputGain(HostGain::ZERO),
                        ),
                        arrangement_frame: 19,
                        performance_frame: 23,
                        loop_pass: 0,
                        remaining_latency_frames: 4,
                    };
                    capture.phase = Phase::Delivering;
                    capture.in_flight = Some((2, Submission::Append));
                    capture.receipt(Receipt::Applied {
                        target: capture.audio,
                        sequence: 1,
                        application,
                    });
                    view.native_automation_recorder.touch = Some(capture);
                    if worker_closed {
                        let capture = view.native_automation_recorder.touch.as_ref().unwrap();
                        let request = Request::DeviceAutomationTake {
                            request_id: 2,
                            scope: capture.scope,
                            action: Action::Append(Arc::from(capture.batch.as_slice())),
                        };
                        view.native_automation_request_not_submitted(&request, false);
                        let capture = view.native_automation_recorder.touch.as_mut().unwrap();
                        let Some((Submission::Append, Action::Append(retry))) =
                            capture.next_submission()
                        else {
                            panic!("temporary pressure must retry the original batch");
                        };
                        assert_eq!(retry.as_ref(), &[(1, application)]);
                        capture.in_flight = Some((2, Submission::Append));
                        view.native_automation_request_not_submitted(&request, true);
                    } else {
                        view.native_automation_take_completion(&event);
                    }
                    let capture = view.native_automation_recorder.touch.as_mut().unwrap();
                    assert_eq!(capture.batch, [(1, application)]);
                    assert!(capture.end == Some(End::Cancel));
                    let late = NativeParameterApplication {
                        arrangement_frame: 27,
                        performance_frame: 31,
                        ..application
                    };
                    capture.receipt(Receipt::Applied {
                        target: capture.audio,
                        sequence: 2,
                        application: late,
                    });
                    assert_eq!(capture.batch, [(1, application), (2, late)]);
                    capture.receipt(Receipt::Aborted {
                        target: capture.audio,
                        reason: AbortReason::Cancelled,
                    });
                    view.tick_native_automation_recording();
                    assert!(view.native_automation_recorder.busy());
                    let capture = view.native_automation_recorder.touch.as_ref().unwrap();
                    assert_eq!(capture.batch.len(), 2);
                    assert!(capture.terminal.is_some());
                    assert!(capture.next_submission().is_none());
                    let (gesture, latest) = (capture.gesture, capture.latest);
                    assert!(view.begin_native_automation_touch(gesture, latest).is_err());
                    view.request_window_close(window, cx);
                    assert!(view.error.as_ref().unwrap().contains("recover"));
                });
            }
            editor.update(cx, |view, _| {
                let mut capture = touch();
                let terminal = Action::Abort(
                    hex_session::service::automation_draft::DraftAbortReason::Cancelled,
                );
                capture.phase = Phase::Delivering;
                capture.in_flight = Some((2, Submission::Terminal));
                capture.terminal = Some(terminal);
                view.native_automation_recorder.touch = Some(capture);
                view.native_automation_take_completion(&event);
                assert!(view.native_automation_recorder.busy());
                let capture = view.native_automation_recorder.touch.as_ref().unwrap();
                assert!(matches!(capture.terminal, Some(Action::Abort(_))));
                assert!(capture.next_submission().is_none());
            });
            editor.update(cx, |view, _| {
                let mut capture = touch();
                let scope = capture.scope;
                capture.phase = Phase::Sealing;
                capture.in_flight = Some((3, Submission::Poll));
                view.native_automation_recorder.touch = Some(capture);
                event.device_automation_take_complete = Some(DeviceAutomationTakeCompletion {
                    request_id: 4,
                    progress: Progress::NativeWriterRetained {
                        path: "/fixture/original.hex-automation-draft".into(),
                        reason: "writer refused".into(),
                    },
                });
                view.native_automation_take_completion(&event);
                let capture = view.native_automation_recorder.touch.as_ref().unwrap();
                assert!(capture.phase == Phase::Sealing && capture.in_flight.is_some());
                event
                    .device_automation_take_complete
                    .as_mut()
                    .unwrap()
                    .request_id = 3;
                view.native_automation_take_completion(&event);
                let capture = view.native_automation_recorder.touch.as_ref().unwrap();
                assert_eq!(capture.scope, scope);
                assert!(capture.phase == Phase::Retained);
                assert!(capture.in_flight.is_none());
                assert!(view.native_automation_recorder.busy());
                assert!(view.error.as_ref().unwrap().contains("writer refused"));
            });
            window.remove_window();
        })
        .unwrap();
    }
}
