//! Monitor rendering freezes both musical clocks and never consumes authored events.
use super::*;

impl SessionPlayback {
    pub(super) fn render_stopped_main_tails(
        &mut self,
        output: &mut [Frame],
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        self.return_effect_tails.begin_block();
        let clock = crate::effect_clock::ArrangementEffectClock::session(
            self.revision.session().timing(),
            self.arrangement_frame,
            self.performance_frame,
            self.arrangement_loop_pass,
            false,
        );
        let step = if self.compensation.is_some() { 1 } else { 256 };
        for (index, chunk) in output.chunks_mut(step).enumerate() {
            self.return_effect_tails.render_main_chunk(
                chunk,
                self.compensation.as_ref(),
                &clock,
                index * step,
                foreign,
            )?;
        }
        self.return_effect_tails.commit_block();
        self.revision.arrangement().master_mix().apply(output);
        Ok(())
    }
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
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        for runtime in self.mutable_instruments.iter_mut().flatten() {
            runtime.begin_block();
        }
        self.release_scratch.clear();
        self.release_scratch.extend(
            self.releases
                .iter()
                .copied()
                .filter(|entry| entry.end_frame() > self.performance_frame),
        );
        for release in &self.release_scratch {
            release
                .monitor_offset
                .checked_add(output.len() as u64)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        }
        for tail in self.tail_mutable_instruments.iter_mut().flatten() {
            tail.runtime.begin_block();
        }
        for chain in self.tail_groups.iter_mut().flatten() {
            chain.begin_block();
        }
        self.tail_output_scratch
            .copy_from_slice(&self.tail_output_active);

        for chain in &mut self.device_chains {
            chain.begin_block();
        }
        for chain in &mut self.return_chains {
            chain.begin_block();
        }
        self.main_chain.begin_block();
        // A state replacement can introduce a retained bus/Main suffix while
        // transport is already stopped. Its real zero-input completion still
        // owns the original endpoint; monitoring must drain that suffix too.
        // An explicit Stop separately resets these tails in the command owner.
        let drain_edit_tails = self.performance_frame == 0 && self.arrangement_frame == 0;
        if drain_edit_tails {
            self.return_effect_tails.begin_block();
        }
        self.route_scratch.copy_from_slice(&self.routes);
        let mut offset = 0usize;
        while offset < output.len() {
            let max_frames =
                (output.len() - offset).min(if self.compensation.is_some() { 1 } else { 256 });
            let count = self
                .computer_keyboard
                .as_ref()
                .map_or(max_frames, |keyboard| {
                    keyboard.source_input_span(max_frames)
                });
            let end = offset
                .checked_add(count)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            let chunk = &mut output[offset..end];
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
                    sources,
                    foreign,
                    None,
                    None,
                )?;
                if drain_edit_tails {
                    self.return_effect_tails
                        .render_chunk(chunk, offset, capture, foreign)?;
                }
                self.compensation
                    .as_ref()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?
                    .process_main_with_foreign(
                        &mut self.main_chain,
                        chunk,
                        offset,
                        foreign,
                        observer,
                        None,
                        None,
                    )
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                let tail_clock = crate::effect_clock::ArrangementEffectClock::session(
                    self.revision.session().timing(),
                    self.arrangement_frame,
                    self.performance_frame,
                    self.arrangement_loop_pass,
                    false,
                );
                if drain_edit_tails {
                    self.return_effect_tails.render_main_chunk(
                        chunk,
                        self.compensation.as_ref(),
                        &tail_clock,
                        offset,
                        foreign,
                    )?;
                }
                self.advance_paused_analytic_releases(count)?;
                offset = end;
                continue;
            }
            for frames in &mut self.return_scratch {
                frames[..chunk.len()].fill([0.0; 2]);
            }
            let timing =
                timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor);
            for (index, track) in self.revision.arrangement().tracks().iter().enumerate() {
                let mut frames = [[0.0; 2]; 256];
                let frames = &mut frames[..chunk.len()];
                if self.source_input_receiver() == Some(index) {
                    source_live::render_source_live_input(
                        self.computer_keyboard.as_mut().map(Box::as_mut),
                        self.mutable_instruments[index].as_mut(),
                        &mut self.mutable_onsets,
                        &mut self.mutable_events,
                        0,
                        self.performance_frame,
                        frames,
                        &timing,
                        false,
                    )?;
                } else if let Some(runtime) = self.mutable_instruments[index]
                    .as_mut()
                    .filter(|runtime| runtime.is_source_owner())
                {
                    self.mutable_onsets.clear();
                    let source_clock = runtime
                        .source_monitor_clock(
                            timing
                                .frame_to_beat(self.performance_frame)
                                .map_err(|_| SessionPlaybackError::TempoConversion)?
                                .raw(),
                            timing
                                .tempo_micro_bpm_at_frame(self.performance_frame)
                                .ok_or(SessionPlaybackError::TempoConversion)?,
                            timing.sample_rate(),
                        )
                        .map_err(|error| error.render_error())?;
                    runtime
                        .process_source_segment(
                            self.performance_frame,
                            frames,
                            &mut self.mutable_onsets,
                            &[],
                            &mut self.mutable_events,
                            Some(&source_clock),
                            &hex_lattice::EMPTY_DEVICE_RESOURCES,
                            None,
                        )
                        .map_err(|error| error.render_error())?;
                }
                if let Some(keyboard) = self.computer_keyboard.as_mut().filter(|keyboard| {
                    !keyboard.is_source_input()
                        && Some(keyboard.track()) == track.canonical_track_id()
                }) {
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
                for release in self.release_scratch.iter().filter(|release| {
                    release.location.track_index() == index
                        && release.location.tail_group().is_none()
                }) {
                    render_release_raw(
                        release,
                        self.revision.session().sine_table(),
                        self.revision.arrangement().shared_contexts(),
                        &self.retained_contexts,
                        &self.analytic_continuations,
                        self.performance_frame,
                        self.playing,
                        frames,
                    )?;
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
                    let mix = track.mix_at(self.arrangement_frame);
                    let fader = &mut self.track_fader_scratch[index];
                    let mix = if track.has_gain_automation() {
                        fader.follow(mix.channel_gain());
                        mix
                    } else {
                        mix.with_channel_gain(fader.next())
                    };
                    let mixed = mix.apply_frame(*frame);
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
            self.render_return_buses(
                chunk,
                offset,
                None,
                observer,
                capture,
                &mut hex_lattice::RefuseForeign,
                None,
            )?;
            if drain_edit_tails {
                self.return_effect_tails
                    .render_chunk(chunk, offset, capture, foreign)?;
            }
            self.render_detached_tails(self.performance_frame, chunk, offset, capture, None)?;
            self.advance_paused_analytic_releases(count)?;
            offset = end;
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
            if drain_edit_tails {
                for (index, chunk) in output.chunks_mut(256).enumerate() {
                    self.return_effect_tails.render_main_chunk(
                        chunk,
                        None,
                        &main_clock,
                        index * 256,
                        foreign,
                    )?;
                }
            }
        }
        for runtime in self.mutable_instruments.iter_mut().flatten() {
            runtime.commit_block();
        }
        for tail in self.tail_mutable_instruments.iter_mut().flatten() {
            tail.runtime.commit_block();
        }
        for chain in self.tail_groups.iter_mut().flatten() {
            chain.commit_block();
        }

        self.main_chain.commit_block();
        if drain_edit_tails {
            self.return_effect_tails.commit_block();
        }
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
