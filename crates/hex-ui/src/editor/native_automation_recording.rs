//! GPUI owns one pointer gesture and a bounded delivery batch. Session owns
//! the draft and history. The callback owns actual application and restoration.
use super::EditorView;
use hex_audio::{
    native_parameter_input::{AbortReason, Receipt, Target},
    prepared::{Client, RetiredScalarReceiptBatch, RetiredScalarReceipts, ScalarReceipt},
};
use hex_engine::{NativeParameterApplication, NativeTransientValue};
use hex_lattice_contract::{DeviceAutomationTarget, HostGain, ParameterValue};
use hex_project::DeviceId;
use hex_session::{
    device_gesture::{GestureKey, GestureTarget},
    service::{
        DeviceAutomationTakeAction as Action, DeviceAutomationTakeProgress as Progress,
        DeviceAutomationTakeScope, Event, Request,
        automation_draft::{DraftProvenance, EVENTS_PER_BATCH},
    },
};
use hex_widgets::NumericEventPhase;
use std::sync::{Arc, atomic::Ordering::Acquire};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Arm {
    generation: u64,
    revision: u64,
    device: DeviceId,
    kind: DeviceAutomationTarget,
}

#[derive(Default)]
pub(super) struct Recorder {
    arm: Option<Arm>,
    touch: Option<Touch>,
    serial: u64,
    request: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Starting,
    Delivering,
    Sealing,
    Orphaned,
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
                if self.phase != Phase::Orphaned {
                    self.batch.push((sequence, application));
                }
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

    fn drain(&mut self, client: Option<&mut Client>) {
        if self.terminal.is_some()
            || self
                .in_flight
                .is_some_and(|(_, kind)| kind == Submission::Append)
        {
            return;
        }
        if let Some(client) = client {
            // Normal credits and terminal receipts have bounded callback
            // capacity. Stop at the batch bound, including across retries.
            for _ in 0..EVENTS_PER_BATCH {
                if self.batch.len() == EVENTS_PER_BATCH || self.terminal.is_some() {
                    break;
                }
                let Some(receipt) = client.native_parameter_receipt() else {
                    break;
                };
                self.receipt(receipt);
            }
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
            {
                let mut receipts = batch.receipts().skip(self.retired_read);
                while self.batch.len() < EVENTS_PER_BATCH && self.terminal.is_none() {
                    let Some(receipt) = receipts.next() else {
                        self.terminal = Some(Action::Interrupt);
                        return;
                    };
                    self.retired_read += 1;
                    if let ScalarReceipt::NativeParameter(receipt) = receipt {
                        self.receipt(receipt);
                    }
                }
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
            Phase::Starting | Phase::Orphaned => None,
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
    pub(super) fn busy(&self) -> bool {
        self.touch.is_some()
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
    pub(super) fn native_automation_arm_selected(
        &self,
        device: DeviceId,
        kind: DeviceAutomationTarget,
    ) -> bool {
        self.native_automation_recorder.arm.is_some_and(|arm| {
            arm.device == device
                && arm.kind == kind
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
            device,
            kind,
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

    pub(super) fn begin_native_automation_touch(
        &mut self,
        gesture: GestureTarget,
        value: ParameterValue,
    ) -> Result<Option<GestureKey>, &'static str> {
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
            batch: Vec::with_capacity(EVENTS_PER_BATCH),
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
            (Submission::Append, _) => {
                // A permanent refusal cannot be retried into another take.
                // Session preserves its durable prefix and reports failure.
                touch.batch.clear();
                touch.end = Some(End::Cancel);
            }
            (Submission::Terminal, _) => {
                touch.terminal = None;
                touch.phase = Phase::Sealing;
            }
            (Submission::Poll, Progress::Pending) => {}
            (Submission::Poll, Progress::Committed) => {
                if let Some(arm) = self.native_automation_recorder.arm.as_mut()
                    && arm.generation == touch.scope.stamp.project_generation
                    && arm.revision == touch.scope.stamp.editor_revision
                    && arm.device == touch.audio.input.device()
                    && arm.kind == touch.audio.input.target()
                    && arm.revision.checked_add(1) == Some(event.state.revision)
                {
                    arm.revision = event.state.revision;
                }
                return;
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

    pub(super) fn native_automation_request_not_submitted(&mut self, request: &Request) {
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
        if matches!(touch.phase, Phase::Starting | Phase::Sealing) || touch.terminal.is_some() {
            self.native_automation_recorder.touch = None;
        } else {
            // No worker terminal will arrive. Release the audio endpoint's
            // real credit and preserve the draft prefix already in Session.
            touch.phase = Phase::Orphaned;
            touch.end = Some(End::Cancel);
            touch.in_flight = None;
            touch.batch.clear();
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
            return true;
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
            batch: Vec::with_capacity(EVENTS_PER_BATCH),
            terminal: None,
            retired: None,
            retired_batch: None,
            retired_read: 0,
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
}
