//! Bounded callback admission against the actual prepared route and input root.
use super::*;
use crate::{Invalid, computer_keyboard::KeyboardActivationClock, midi_capture::*};
impl SessionPlayback {
    pub(crate) fn arm_midi_capture(
        &mut self,
        binding: MidiCaptureBinding,
        clock: &KeyboardActivationClock,
    ) -> Result<MidiCaptureClock, Invalid> {
        if binding.take_id == 0
            || binding.output_epoch == 0
            || binding.sample_rate != self.revision.arrangement().sample_rate()
            || binding.first_frame >= binding.end_frame
            || self.arrangement_frame < binding.first_frame
            || self.arrangement_frame >= binding.end_frame
        {
            return Err(Invalid(
                "midi capture requires a playing resident nonlooping arrangement",
            ));
        }
        let latency = u64::from(self.revision.arrangement().main_latency_frames());
        binding
            .end_frame
            .checked_add(latency)
            .ok_or(Invalid("midi capture latency exceeds frame range"))?;
        if !self.capture_route_supported() {
            return Err(Invalid(
                "midi capture requires a playing resident nonlooping arrangement",
            ));
        }
        let route = self
            .routes
            .iter()
            .position(|route| {
                route.id == binding.track
                    && route.owner == RouteOwner::Arrangement
                    && route.pending.is_none()
            })
            .ok_or(Invalid("midi capture arrangement route is unavailable"))?;
        self.computer_keyboard
            .as_mut()
            .ok_or(Invalid("midi recording input was not prepared"))?
            .arm_capture(binding, clock, route, latency)?;
        Ok(MidiCaptureClock {
            content_frame: self.arrangement_frame,
            performance_frame: self.performance_frame,
        })
    }
    fn capture_route_supported(&self) -> bool {
        self.playing
            && !self.linear_arrangement_export
            && !self.current_sampler_console
            && !self.revision.contains_streamed_sources()
            && self.loop_region().is_none()
    }
    pub(super) fn begin_midi_capture_render(&mut self, frames: usize) {
        let supported = self.capture_route_supported();
        let Some(keyboard) = self.computer_keyboard.as_mut() else {
            return;
        };
        keyboard.invalidate_capture_render();
        let Some(capture) = keyboard
            .capture
            .as_ref()
            .filter(|capture| capture.is_active())
        else {
            return;
        };
        let Some(binding) = capture.binding else {
            keyboard.fail_capture(MidiCaptureFault::InputChanged);
            return;
        };
        let route_supported = self.routes.get(capture.route).is_some_and(|route| {
            route.id == binding.track
                && route.owner == RouteOwner::Arrangement
                && route.pending.is_none()
        });
        let keys_supported = keyboard
            .input
            .lease()
            .and_then(|input| input.ordered_keys.as_ref())
            .is_some_and(|queue| queue.active(binding.keys));
        if !supported || !route_supported {
            keyboard.fail_capture(MidiCaptureFault::RouteChanged);
            return;
        }
        if !keys_supported || !keyboard.validate_capture_input(binding, None) {
            let fault = if keyboard.input.lease().is_some_and(|input| {
                !input.is_cancelled()
                    && (input.ordered_keys_faulted(binding.keys)
                        || binding.hardware.is_some_and(|token| {
                            input
                                .live_notes()
                                .is_some_and(|live| live.activation_rejected(token))
                        }))
            }) {
                MidiCaptureFault::Capacity
            } else {
                MidiCaptureFault::InputChanged
            };
            keyboard.fail_capture(fault);
            return;
        }
        if let Some(capture) = &mut keyboard.capture
            && capture
                .begin(self.arrangement_frame, self.performance_frame, frames)
                .is_err()
        {
            capture.fail(MidiCaptureFault::Clock);
        }
    }
    pub(super) fn seal_midi_capture_render(&mut self) {
        let Some(keyboard) = &mut self.computer_keyboard else {
            return;
        };
        let Some(binding) = keyboard
            .capture
            .as_ref()
            .filter(|capture| capture.is_active())
            .and_then(|capture| capture.binding)
        else {
            return;
        };
        // A producer refusal during rendering must not be hidden by a concurrent
        // normal Finish. Output-clock revocation is deliberately not rechecked:
        // this batch describes an already rendered transaction, and the caller
        // still validates its delivered host chunk and Stop intent.
        let fault = keyboard.input.lease().and_then(|input| {
            if input.is_cancelled() {
                Some(MidiCaptureFault::InputChanged)
            } else if input.ordered_keys_faulted(binding.keys)
                || binding.hardware.is_some_and(|token| {
                    input
                        .live_notes()
                        .is_some_and(|live| live.activation_rejected(token))
                })
            {
                Some(MidiCaptureFault::Capacity)
            } else if input
                .ordered_keys
                .as_ref()
                .is_none_or(|queue| !queue.active(binding.keys))
                || match binding.hardware {
                    Some(token) => input
                        .live_notes()
                        .is_none_or(|live| !live.same_current_activation(token)),
                    None => input.live_notes().is_some_and(|live| !live.is_cancelled()),
                }
            {
                Some(MidiCaptureFault::InputChanged)
            } else {
                None
            }
        });
        if let Some(fault) = fault {
            keyboard.fail_capture(fault);
            return;
        }
        if let Some(capture) = &mut keyboard.capture {
            capture.seal();
        }
    }
    pub(super) fn fail_midi_capture_render(&mut self) {
        if let Some(keyboard) = self.computer_keyboard.as_mut() {
            keyboard.fail_capture(MidiCaptureFault::RenderFailed);
        }
    }
    pub(crate) fn midi_capture_render(
        &self,
        take_id: u64,
    ) -> Result<Option<MidiCaptureRender<'_>>, Invalid> {
        self.computer_keyboard
            .as_ref()
            .map_or(Ok(None), |keyboard| keyboard.capture_render(take_id))
    }
    pub(crate) fn finish_midi_capture(
        &mut self,
        take_id: u64,
    ) -> Result<MidiCaptureClock, Invalid> {
        self.computer_keyboard
            .as_mut()
            .ok_or(Invalid("midi recording input was not prepared"))?
            .finish_capture(take_id)?;
        Ok(MidiCaptureClock {
            content_frame: self.arrangement_frame,
            performance_frame: self.performance_frame,
        })
    }
    pub(crate) fn midi_capture_fault(&self, take_id: u64) -> Option<MidiCaptureFault> {
        self.computer_keyboard
            .as_ref()?
            .capture
            .as_ref()
            .filter(|capture| {
                capture
                    .binding
                    .is_some_and(|binding| binding.take_id == take_id)
            })?
            .faulted
    }
    pub(crate) fn midi_capture_active(&self) -> bool {
        self.computer_keyboard
            .as_ref()
            .is_some_and(|keyboard| keyboard.midi_capture_active())
    }
}
