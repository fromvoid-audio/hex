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
    pub(super) fn render_return_buses(
        &mut self,
        output: &mut [Frame],
        frame_offset: usize,
        effect_clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        capture: &mut Option<&mut dyn CaptureSink>,
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
