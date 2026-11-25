//! Bounded callback admission against the actual prepared route and input root.
use super::*;
use crate::{Invalid, computer_keyboard::KeyboardActivationClock, midi_capture::*};
impl SessionPlayback {
    pub(crate) fn arrangement_loop_pass(&self) -> u64 {
        self.arrangement_loop_pass
    }
    pub(crate) fn midi_capture_clock(&self, route: MidiCaptureRoute) -> MidiCaptureClock {
        match route {
            MidiCaptureRoute::Arrangement => MidiCaptureClock {
                content_frame: self.arrangement_frame,
                performance_frame: self.performance_frame,
                loop_pass: self.arrangement_loop_pass,
            },
            MidiCaptureRoute::SessionMidi { .. } => MidiCaptureClock {
                content_frame: self.performance_frame,
                performance_frame: self.performance_frame,
                loop_pass: 0,
            },
        }
    }
    fn route_matches_binding(&self, route: &TrackRoute, binding: MidiCaptureBinding) -> bool {
        if route.id != binding.track || route.pending.is_some() {
            return false;
        }
        match binding.route {
            MidiCaptureRoute::Arrangement => route.owner == RouteOwner::Arrangement,
            MidiCaptureRoute::SessionMidi {
                clip,
                origin,
                clock_anchor,
            } => {
                self.clock_anchor() == clock_anchor
                    && matches!(
                        route.owner,
                        RouteOwner::Session {
                            clip: active_clip,
                            origin: active_origin,
                            ..
                        } if active_clip == clip && active_origin == origin
                    )
            }
        }
    }
    pub(crate) fn arm_midi_capture(
        &mut self,
        binding: MidiCaptureBinding,
        clock: &KeyboardActivationClock,
    ) -> Result<MidiCaptureClock, Invalid> {
        let active_loop = self
            .loop_region()
            .map(|region| (region.start(), region.end()));
        let (loop_valid, position_valid) = match binding.route {
            MidiCaptureRoute::Arrangement => {
                let pos = match active_loop {
                    Some((loop_start, loop_end)) => {
                        loop_start < loop_end && self.arrangement_frame < loop_end
                    }
                    None => {
                        self.arrangement_frame >= binding.first_frame
                            && self.arrangement_frame < binding.end_frame
                    }
                };
                (binding.loop_region == active_loop, pos)
            }
            MidiCaptureRoute::SessionMidi { .. } => (
                binding.loop_region.is_none() && active_loop.is_none(),
                self.performance_frame >= binding.first_frame
                    && self.performance_frame < binding.end_frame,
            ),
        };
        if binding.take_id == 0
            || binding.output_epoch == 0
            || binding.sample_rate != self.revision.arrangement().sample_rate()
            || binding.first_frame >= binding.end_frame
            || !loop_valid
            || !position_valid
        {
            return Err(Invalid(
                "midi capture requires a playing resident arrangement or active console clip",
            ));
        }
        let latency = u64::from(self.revision.arrangement().main_latency_frames());
        binding
            .end_frame
            .checked_add(latency)
            .ok_or(Invalid("midi capture latency exceeds frame range"))?;
        if !self.capture_route_supported() {
            return Err(Invalid(
                "midi capture requires a playing resident arrangement or active console clip",
            ));
        }
        let route = self
            .routes
            .iter()
            .position(|route| self.route_matches_binding(route, binding))
            .ok_or(Invalid("midi capture route is unavailable"))?;
        self.computer_keyboard
            .as_mut()
            .ok_or(Invalid("midi recording input was not prepared"))?
            .arm_capture(binding, clock, route, latency)?;
        Ok(self.midi_capture_clock(binding.route))
    }
    fn capture_route_supported(&self) -> bool {
        self.playing
            && !self.linear_arrangement_export
            && !self.current_sampler_console
            && !self.revision.contains_streamed_sources()
    }
    pub(super) fn begin_midi_capture_render(&mut self, frames: usize) {
        let supported = self.capture_route_supported();
        let active_loop = self
            .loop_region()
            .map(|region| (region.start(), region.end()));
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
        let route_index = capture.route;
        let loop_matches = match binding.route {
            MidiCaptureRoute::Arrangement => binding.loop_region == active_loop,
            MidiCaptureRoute::SessionMidi { .. } => {
                binding.loop_region.is_none() && active_loop.is_none()
            }
        };
        let route_supported = self
            .routes
            .get(route_index)
            .is_some_and(|route| self.route_matches_binding(route, binding));
        let capture_clock = self.midi_capture_clock(binding.route);
        let Some(keyboard) = self.computer_keyboard.as_mut() else {
            return;
        };
        let keys_supported = keyboard
            .input
            .lease()
            .and_then(|input| input.ordered_keys.as_ref())
            .is_some_and(|queue| queue.active(binding.keys));
        if !supported || !route_supported || !loop_matches {
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
                .begin(
                    capture_clock.content_frame,
                    capture_clock.performance_frame,
                    capture_clock.loop_pass,
                    frames,
                )
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
        let route = self
            .computer_keyboard
            .as_ref()
            .and_then(|keyboard| keyboard.capture.as_ref())
            .and_then(|capture| capture.binding)
            .filter(|binding| binding.take_id == take_id)
            .map_or(MidiCaptureRoute::Arrangement, |binding| binding.route);
        self.computer_keyboard
            .as_mut()
            .ok_or(Invalid("midi recording input was not prepared"))?
            .finish_capture(take_id)?;
        Ok(self.midi_capture_clock(route))
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
