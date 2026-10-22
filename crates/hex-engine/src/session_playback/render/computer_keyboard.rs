//! Monitor rendering freezes both musical clocks and never consumes authored events.
use super::*;

impl SessionPlayback {
    pub(crate) fn cancel_current_sampler_keyboard(&mut self) {
        self.current_sampler_keyboard_revoked = self
            .current_sampler_keyboard_revoked
            .max(self.current_sampler_keyboard_epoch);
        for keyboard in &mut self.current_sampler_keyboards {
            keyboard.reset_current_activation();
        }
    }
    pub(crate) fn apply_current_sampler_keyboard(
        &mut self,
        frame: Option<crate::computer_keyboard::CurrentSamplerKeyboardFrame>,
    ) -> Result<(), crate::Invalid> {
        let Some(frame) = frame else {
            self.cancel_current_sampler_keyboard();
            return Ok(());
        };
        if frame
            .words
            .iter()
            .any(|word| !crate::computer_keyboard::valid_key_word(*word))
        {
            return Err(crate::Invalid("sampler keyboard words are invalid"));
        }
        if frame.epoch == 0
            || frame.epoch <= self.current_sampler_keyboard_revoked
            || frame.epoch < self.current_sampler_keyboard_epoch
            || !self
                .current_sampler_keyboards
                .iter()
                .any(|k| k.is_current_target(frame.track, frame.device))
        {
            return Err(crate::Invalid("sampler keyboard activation is stale"));
        }
        if frame.epoch != self.current_sampler_keyboard_epoch {
            self.cancel_current_sampler_keyboard();
            self.current_sampler_keyboard_epoch = frame.epoch;
        }
        let keyboard = self
            .current_sampler_keyboards
            .iter_mut()
            .find(|k| k.is_current_target(frame.track, frame.device))
            .ok_or(crate::Invalid("sampler keyboard target is unavailable"))?;
        keyboard.set_current_words(frame.words)
    }

    pub(super) fn render_keyboard_monitor(
        &mut self,
        output: &mut [Frame],
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        capture: &mut Option<&mut dyn CaptureSink>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        for chain in &mut self.device_chains {
            chain.begin_block();
        }
        for chain in &mut self.return_chains {
            chain.begin_block();
        }
        self.main_chain.begin_block();
        self.route_scratch.copy_from_slice(&self.routes);
        let step = if self.compensation.is_some() { 1 } else { 256 };
        for (chunk_index, chunk) in output.chunks_mut(step).enumerate() {
            let offset = chunk_index * step;
            if self.compensation.is_some() {
                self.render_compensated_route_frame::<dyn MediaPages>(
                    self.performance_frame,
                    self.arrangement_frame,
                    self.arrangement_loop_pass,
                    chunk,
                    None,
                    offset,
                    observer,
                    capture,
                    None,
                )?;
                self.compensation
                    .as_ref()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?
                    .process_main(&mut self.main_chain, chunk, offset, observer)
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                continue;
            }
            for frames in &mut self.return_scratch {
                frames[..chunk.len()].fill([0.0; 2]);
            }
            let timing = timing_view_for(self.revision.session(), self.periodic_tempo, self.anchor);
            for (index, track) in self.revision.arrangement().tracks().iter().enumerate() {
                let mut frames = [[0.0; 2]; 256];
                let frames = &mut frames[..chunk.len()];
                if let Some(keyboard) = self
                    .computer_keyboard
                    .as_mut()
                    .filter(|keyboard| Some(keyboard.track()) == track.canonical_track_id())
                {
                    let clock = keyboard
                        .musical_clock(&timing, self.performance_frame, false)
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    keyboard
                        .render(frames, Some(&clock))
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                }
                for keyboard in self
                    .current_sampler_keyboards
                    .iter_mut()
                    .filter(|k| Some(k.track()) == track.canonical_track_id())
                {
                    let source = sources.ok_or(SessionPlaybackError::DeviceRuntime)?;
                    let resources = source
                        .resources(keyboard.track())
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    keyboard
                        .render_with_resources(frames, None, &resources)
                        .map_err(|e| SessionPlaybackError::Arrangement(e.0))?;
                }
                track.apply_instrument_output_gain(frames);
                let chain = &mut self.device_chains[index];
                if chain.is_empty() {
                    track.apply_ordered_suffix(frames);
                } else if let Some(observer) = observer.as_mut() {
                    chain
                        .process_range_tapped(
                            track.instrument_position()..chain.len(),
                            frames,
                            offset,
                            &mut |owner, device, kind, phase, offset, frames| {
                                observer(
                                    crate::PostNodeIdentity::new(owner, device, kind),
                                    phase,
                                    offset,
                                    frames,
                                )
                            },
                        )
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                } else {
                    chain
                        .process_range(track.instrument_position()..chain.len(), frames)
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                }
                for (frame_index, frame) in frames.iter_mut().enumerate() {
                    let mixed = track.mix_at(self.arrangement_frame).apply_frame(*frame);
                    *frame = mixed;
                    chunk[frame_index][0] += mixed[0];
                    chunk[frame_index][1] += mixed[1];
                    for send in track.sends() {
                        self.return_scratch[send.return_index][frame_index][0] +=
                            mixed[0] * send.gain;
                        self.return_scratch[send.return_index][frame_index][1] +=
                            mixed[1] * send.gain;
                    }
                }
                if let Some(capture) = capture.as_mut()
                    && let Some(id) = track.canonical_track_id()
                {
                    capture.add(OutputCaptureId::Track(id), offset, frames)?;
                }
            }
            self.render_return_buses(chunk, offset, None, observer, capture)?;
        }
        if self.compensation.is_none() {
            let main_clock = crate::effect_clock::ArrangementEffectClock::session(
                self.revision.session().timing(),
                self.arrangement_frame,
                self.performance_frame,
                self.arrangement_loop_pass,
                false,
            );
            crate::main_chain::process(
                &mut self.main_chain,
                output,
                0,
                Some(&main_clock),
                observer,
            )
            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
        }
        self.main_chain.commit_block();
        self.revision.arrangement().master_mix().apply(output);
        for chain in &mut self.device_chains {
            chain.commit_block();
        }
        for chain in &mut self.return_chains {
            chain.commit_block();
        }
        Ok(())
    }
}
