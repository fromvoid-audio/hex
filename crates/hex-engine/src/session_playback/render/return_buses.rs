//! Return-bus chain processing, cross-return feed propagation, and return fades.
use super::*;

fn apply_return_fade(
    fade: &mut Option<device_removal::ReturnFade>,
    id: hex_project::ReturnId,
    frames: &mut [Frame],
) {
    if let Some(fade) = fade.as_mut().filter(|fade| fade.id == id) {
        for frame in frames {
            let gain = fade.fader.next();
            frame[0] *= gain[0];
            frame[1] *= gain[1];
        }
    }
}

impl SessionPlayback {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_return_buses(
        &mut self,
        output: &mut [Frame],
        frame_offset: usize,
        effect_clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        capture: &mut Option<&mut dyn CaptureSink>,
        wave: &mut Option<&mut dyn crate::wave_executor::WaveExecutor>,
        stage: ForeignRenderStage<'_>,
    ) -> Result<(), SessionPlaybackError> {
        let ForeignRenderStage { foreign, external } = stage;
        let arrangement = self.revision.arrangement();
        if !arrangement.return_feed_plan.is_empty() {
            for &index in arrangement.return_feed_plan.order() {
                let work_start = self.render_work.return_bus(index);
                let bus = &arrangement.return_buses()[index];
                let frames = &mut self.return_scratch[index][..output.len()];
                let chain = self
                    .return_chains
                    .get_mut(index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let range = 0..chain.len();
                crate::native_parameter_input::process_range_with_external(
                    self.native_parameter_state
                        .as_deref_mut()
                        .and_then(|state| state.processing.as_mut()),
                    external,
                    chain,
                    range,
                    frames,
                    frame_offset,
                    effect_clock,
                    effect_clock,
                    0,
                    0,
                    foreign,
                    observer,
                )
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                if let Some(matrix) = bus.matrix {
                    matrix.apply_block(frames);
                }
                apply_return_fade(&mut self.return_fade_scratch, bus.id, frames);
                if bus.muted {
                    frames.fill([0.0; 2]);
                } else {
                    for frame in frames.iter_mut() {
                        frame[0] *= bus.gain;
                        frame[1] *= bus.gain;
                    }
                }
                if let Some(capture) = capture.as_mut() {
                    capture.add(
                        OutputCaptureId::Return(bus.id),
                        frame_offset,
                        &self.return_scratch[index][..output.len()],
                    )?;
                }
                arrangement.return_feed_plan.propagate(
                    index,
                    &mut self.return_scratch,
                    output.len(),
                );
                self.render_work.finish(work_start);
            }
            let count = output.len();
            for frames in &self.return_scratch {
                for (destination, source) in output.iter_mut().zip(&frames[..count]) {
                    destination[0] += source[0];
                    destination[1] += source[1];
                }
            }
            // Captured chains feed Main only, never any current or old edge.
            if self.playing {
                self.return_effect_tails
                    .render_chunk(output, frame_offset, capture, foreign)?;
            }
            return Ok(());
        }
        let bus_count = self.revision.arrangement().return_buses().len();
        let can_parallel_returns = external.is_none()
            && self
                .native_parameter_state
                .as_deref()
                .is_none_or(|state| state.processing.is_none())
            && output.len() <= crate::wave_executor::WAVE_MAX_FRAMES
            && (crate::wave_executor::PARALLEL_WAVE_MIN_TASKS
                ..=crate::wave_executor::MAX_WAVE_TASKS)
                .contains(&bus_count)
            && wave.as_deref().is_some_and(|exec| exec.worker_count() > 0);
        if can_parallel_returns && let Some(exec) = wave.as_deref_mut() {
            let mut eligible_count = 0usize;
            for index in 0..bus_count {
                let bus_id = self.revision.arrangement().return_buses()[index].id.value();
                if observer.as_deref().is_some_and(|obs| {
                    obs.observes_owner(crate::ProcessorChainOwner::Return(bus_id))
                }) {
                    continue;
                }
                let chain = &self.return_chains[index];
                if !chain.is_empty()
                    && chain.len() <= crate::wave_executor::MAX_WAVE_STAGES
                    && chain.foreign_bindings().next().is_none()
                {
                    eligible_count += 1;
                }
            }
            if eligible_count >= crate::wave_executor::PARALLEL_WAVE_MIN_TASKS
                && let Some(clock_window) = exec.prepare_clock()
            {
                if let Some(clock) = effect_clock {
                    clock_window.fill_from_clock(clock, output.len());
                } else {
                    clock_window.history_len = 0;
                    clock_window.incoming_len = 0;
                }
                let mut staged = [None; crate::wave_executor::MAX_WAVE_TASKS];
                for (index, slot_out) in staged[..bus_count].iter_mut().enumerate() {
                    let bus_id = self.revision.arrangement().return_buses()[index].id.value();
                    if observer.as_deref().is_some_and(|obs| {
                        obs.observes_owner(crate::ProcessorChainOwner::Return(bus_id))
                    }) {
                        continue;
                    }
                    let chain = &self.return_chains[index];
                    if !chain.is_empty()
                        && chain.len() <= crate::wave_executor::MAX_WAVE_STAGES
                        && chain.foreign_bindings().next().is_none()
                    {
                        let len = chain.len();
                        let moved = std::mem::replace(
                            &mut self.return_chains[index],
                            hex_lattice::NativeRuntimeChain::empty(),
                        );
                        let frames = &self.return_scratch[index][..output.len()];
                        match exec.push_task(index, moved, frames, frame_offset, 0, len, false, &[])
                        {
                            Ok(slot) => *slot_out = Some(slot),
                            Err(returned) => {
                                self.return_chains[index] = returned;
                            }
                        }
                    }
                }
                exec.execute_staged();
                let mut first_err: Option<SessionPlaybackError> = None;
                let count = output.len();
                for (index, slot_opt) in staged[..bus_count].iter().enumerate() {
                    if let Some(slot) = *slot_opt {
                        let (owner_index, chain, err) =
                            exec.reclaim_task(slot, &mut self.return_scratch[index][..count]);
                        self.return_chains[owner_index] = chain;
                        if first_err.is_none() && err.is_some() {
                            first_err = Some(SessionPlaybackError::DeviceRuntime);
                        }
                    }
                }
                exec.finish_wave();
                if let Some(err) = first_err {
                    return Err(err);
                }
                for (index, bus) in self
                    .revision
                    .arrangement()
                    .return_buses()
                    .iter()
                    .enumerate()
                {
                    let work_start = self.render_work.return_bus(index);
                    let frames = self
                        .return_scratch
                        .get_mut(index)
                        .ok_or(SessionPlaybackError::DeviceRuntime)?;
                    let frames = &mut frames[..count];
                    if staged[index].is_none() {
                        let chain = self
                            .return_chains
                            .get_mut(index)
                            .ok_or(SessionPlaybackError::DeviceRuntime)?;
                        let range = 0..chain.len();
                        crate::native_parameter_input::process_range_with_external(
                            self.native_parameter_state
                                .as_deref_mut()
                                .and_then(|state| state.processing.as_mut()),
                            external,
                            chain,
                            range,
                            frames,
                            frame_offset,
                            effect_clock,
                            effect_clock,
                            0,
                            0,
                            foreign,
                            observer,
                        )
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    }
                    if let Some(matrix) = bus.matrix {
                        matrix.apply_block(frames);
                    }
                    apply_return_fade(&mut self.return_fade_scratch, bus.id, frames);
                    if !bus.muted {
                        for (destination, source) in output.iter_mut().zip(frames.iter()) {
                            destination[0] += source[0] * bus.gain;
                            destination[1] += source[1] * bus.gain;
                        }
                    }
                    if capture.is_some() || self.device_metering {
                        for frame in frames.iter_mut() {
                            *frame = if bus.muted {
                                [0.0; 2]
                            } else {
                                [frame[0] * bus.gain, frame[1] * bus.gain]
                            };
                        }
                    }
                    if let Some(capture) = capture.as_mut() {
                        capture.add(OutputCaptureId::Return(bus.id), frame_offset, frames)?;
                    }
                    self.render_work.finish(work_start);
                }
                if self.playing {
                    self.return_effect_tails.render_chunk(
                        output,
                        frame_offset,
                        capture,
                        foreign,
                    )?;
                }
                return Ok(());
            }
        }
        for (index, bus) in self
            .revision
            .arrangement()
            .return_buses()
            .iter()
            .enumerate()
        {
            let work_start = self.render_work.return_bus(index);
            let frames = self
                .return_scratch
                .get_mut(index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let frames = &mut frames[..output.len()];
            let chain = self
                .return_chains
                .get_mut(index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let range = 0..chain.len();
            crate::native_parameter_input::process_range_with_external(
                self.native_parameter_state
                    .as_deref_mut()
                    .and_then(|state| state.processing.as_mut()),
                external,
                chain,
                range,
                frames,
                frame_offset,
                effect_clock,
                effect_clock,
                0,
                0,
                foreign,
                observer,
            )
            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            if let Some(matrix) = bus.matrix {
                matrix.apply_block(frames);
            }
            apply_return_fade(&mut self.return_fade_scratch, bus.id, frames);
            if !bus.muted {
                for (destination, source) in output.iter_mut().zip(frames.iter()) {
                    destination[0] += source[0] * bus.gain;
                    destination[1] += source[1] * bus.gain;
                }
            }
            if capture.is_some() || self.device_metering {
                for frame in frames.iter_mut() {
                    *frame = if bus.muted {
                        [0.0; 2]
                    } else {
                        [frame[0] * bus.gain, frame[1] * bus.gain]
                    };
                }
            }
            if let Some(capture) = capture.as_mut() {
                capture.add(OutputCaptureId::Return(bus.id), frame_offset, frames)?;
            }
            self.render_work.finish(work_start);
        }
        if self.playing {
            self.return_effect_tails
                .render_chunk(output, frame_offset, capture, foreign)?;
        }
        Ok(())
    }
}
