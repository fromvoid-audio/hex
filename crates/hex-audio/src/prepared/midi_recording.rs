//! MIDI receipts are confirmed only by device::finish_host_chunk after direct
//! conversion. Engine render success and Status are correlation, never delivery.
use super::*;
use crate::midi_recording::{AbortReason, Target};
use hex_engine::midi_capture::{MidiCaptureClock, MidiCaptureFault};

impl Callback {
    pub(super) fn recording_source(&self) -> Option<AppliedAudioSource> {
        match self.active.as_ref()? {
            ActivePublication::Legacy(active) => Some(AppliedAudioSource {
                context: active.source_context?,
                audio_revision: active.revision,
                output_identity: self.candidate_root.root_id(),
            }),
            ActivePublication::Eligible(active) => active.applied_audio_source(),
            ActivePublication::Candidate(_) => None,
        }
    }
    fn midi_source_matches(&self, target: Target) -> bool {
        self.recording_source()
            == Some(AppliedAudioSource {
                context: hex_engine::AudioSourceContext {
                    project_generation: target.project_generation,
                    editor_revision: target.canonical_revision,
                },
                audio_revision: target.audio_revision,
                output_identity: target.output_identity,
            })
    }
    fn midi_target_refusal(&self, target: Target) -> Option<AbortReason> {
        if self.status.closing.load(Acquire) {
            return Some(AbortReason::Cancelled);
        }
        if self.status.device_fault.load(Acquire)
            || self.status.render_fault.load(Acquire)
            || target.transport_epoch <= self.status.last_output_fault_epoch.load(Acquire)
        {
            return Some(AbortReason::OutputFailed);
        }
        if self.output_transport.is_some() {
            return Some(AbortReason::UnsupportedOutput);
        }
        if !self.midi_source_matches(target) {
            return Some(AbortReason::SourceChanged);
        }
        if target.transport_epoch != self.status.transport_epoch.load(Acquire) {
            return Some(AbortReason::TransportChanged);
        }
        None
    }
    fn cleanup_midi_capture(&mut self, target: Option<Target>) {
        if self.midi_recording.target().is_none()
            && let Some(target) = target
            && let Some(mut active) = self
                .active
                .as_mut()
                .and_then(ActivePublication::current_mut)
        {
            active.playback.finish_midi_capture(target.capture.take_id);
        }
        if self.midi_recording.receipt_fault {
            self.status.midi_recording_fault.store(true, Release);
        }
    }
    pub(super) fn invalidate_midi_recording(&mut self, reason: AbortReason) {
        if self.midi_recording.target().is_none()
            && let Some(target) = self.midi_recording.next_arm()
        {
            self.midi_recording.accept(target);
        }
        let target = self.midi_recording.target();
        self.midi_recording.abort(reason);
        self.cleanup_midi_capture(target);
    }
    /// Called before priority Stop resets the actual playback clocks. A pending
    /// Arm has no notes, but still receives its one reserved terminal receipt.
    pub(super) fn finish_midi_before_stop(&mut self) {
        if self.midi_recording.target().is_none()
            && let Some(target) = self.midi_recording.next_arm()
        {
            self.midi_recording.accept(target);
        }
        let Some(target) = self.midi_recording.target() else {
            return;
        };
        if let Some(reason) = self.midi_target_refusal(target) {
            self.midi_recording.abort(reason);
        } else if let Some(fault) = self
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .and_then(|active| {
                active
                    .playback
                    .midi_capture_finish_fault(target.capture.take_id)
            })
            .filter(|fault| *fault != MidiCaptureFault::InputChanged)
        {
            self.midi_recording.abort(fault_reason(fault));
        } else if self.midi_recording.requested_finish() == Some(false) {
            self.midi_recording.finish();
        } else {
            self.midi_recording
                .abort(if self.midi_recording.requested_finish() == Some(true) {
                    AbortReason::Cancelled
                } else {
                    AbortReason::TransportChanged
                });
        }
        self.cleanup_midi_capture(Some(target));
    }
    pub(super) fn prepare_midi_recording_quantum(&mut self, frames: usize) {
        if self.midi_recording.has_staged() {
            self.invalidate_midi_recording(AbortReason::OutputFailed);
        }
        let new = if self.midi_recording.target().is_none() {
            self.midi_recording.next_arm()
        } else {
            None
        };
        if let Some(target) = new {
            self.midi_recording.accept(target);
        }
        let Some(target) = self.midi_recording.target() else {
            return;
        };
        if let Some(reason) = self.midi_target_refusal(target) {
            self.invalidate_midi_recording(reason);
            return;
        }
        if let Some(cancelled) = self.midi_recording.requested_finish() {
            let fault = self
                .active
                .as_ref()
                .and_then(ActivePublication::current)
                .and_then(|active| {
                    active
                        .playback
                        .midi_capture_finish_fault(target.capture.take_id)
                });
            if let Some(fault) = fault.filter(|fault| *fault != MidiCaptureFault::InputChanged) {
                self.midi_recording.abort(fault_reason(fault));
            } else if cancelled {
                self.midi_recording.abort(AbortReason::Cancelled);
            } else {
                self.midi_recording.finish();
            }
            self.cleanup_midi_capture(Some(target));
            return;
        }
        let result = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
            .ok_or(AbortReason::SourceChanged)
            .and_then(|mut active| {
                if !active.playback.playing() {
                    return Err(AbortReason::TransportChanged);
                }
                if new.is_some() {
                    active
                        .playback
                        .arm_midi_capture(target.capture, &self.status.keyboard_activation)
                        .map_err(|_| AbortReason::InputChanged)?;
                }
                Ok(MidiCaptureClock {
                    content_frame: active.playback.position(),
                    performance_frame: active.playback.performance_frame(),
                })
            });
        match result {
            Ok(clock) => self.midi_recording.stage(clock, frames),
            Err(reason) => self.invalidate_midi_recording(reason),
        }
    }
    /// The native adapter calls this once after copying an entire direct chunk.
    /// No event array is copied here: borrow the engine's successful whole-render
    /// slice, publish bounded scalar receipts, then retire capture on terminal.
    pub(crate) fn finish_midi_recording_quantum(&mut self, output_valid: bool) {
        let Some(target) = self.midi_recording.target() else {
            return;
        };
        if let Some(reason) = self.midi_target_refusal(target) {
            self.invalidate_midi_recording(reason);
            return;
        }
        if self.midi_recording.requested_finish() == Some(true) {
            self.invalidate_midi_recording(AbortReason::Cancelled);
            return;
        }
        if !output_valid {
            let reason = self
                .active
                .as_ref()
                .and_then(ActivePublication::current)
                .and_then(|active| active.playback.midi_capture_fault(target.capture.take_id))
                .map_or(AbortReason::OutputFailed, fault_reason);
            self.invalidate_midi_recording(reason);
            return;
        }
        let active = self.active.as_ref().and_then(ActivePublication::current);
        let result = active
            .as_ref()
            .map(|active| active.playback.midi_capture_render(target.capture.take_id));
        match result {
            Some(Ok(Some(render))) => self.midi_recording.delivered(render),
            _ => {
                let fault = active
                    .and_then(|active| active.playback.midi_capture_fault(target.capture.take_id));
                if fault == Some(MidiCaptureFault::InputChanged)
                    && self.midi_recording.requested_finish() == Some(false)
                {
                    self.midi_recording.finish();
                } else {
                    self.midi_recording.abort(match fault {
                        Some(MidiCaptureFault::Capacity) => AbortReason::Capacity,
                        Some(MidiCaptureFault::RouteChanged) => AbortReason::RouteChanged,
                        Some(MidiCaptureFault::RenderFailed) => AbortReason::RenderFailed,
                        Some(MidiCaptureFault::Clock) => AbortReason::InvalidClock,
                        Some(MidiCaptureFault::InputChanged) => AbortReason::InputChanged,
                        None => AbortReason::InvalidClock,
                    });
                }
            }
        }
        if self.midi_recording.target().is_some()
            && let Some(cancelled) = self.midi_recording.requested_finish()
        {
            if cancelled {
                self.midi_recording.abort(AbortReason::Cancelled);
            } else {
                self.midi_recording.finish();
            }
        }
        self.cleanup_midi_capture(Some(target));
    }
}

fn fault_reason(fault: MidiCaptureFault) -> AbortReason {
    match fault {
        MidiCaptureFault::InputChanged => AbortReason::InputChanged,
        MidiCaptureFault::Capacity => AbortReason::Capacity,
        MidiCaptureFault::RouteChanged => AbortReason::RouteChanged,
        MidiCaptureFault::RenderFailed => AbortReason::RenderFailed,
        MidiCaptureFault::Clock => AbortReason::InvalidClock,
    }
}
