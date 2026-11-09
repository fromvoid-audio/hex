//! Finite Arrangement path for compensated graphs and typed track routes.
use super::*;
use crate::{MediaPages, RendererRuntime};
impl RendererRuntime {
    pub(crate) fn render_compensated<P: MediaPages + ?Sized>(
        &mut self,
        project: &Project,
        start: u64,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
    ) -> Result<(), Invalid> {
        let volume_clock = project.midi_volume_timing();
        let volume_timing = volume_clock
            .as_ref()
            .map(|clock| clock as &dyn crate::prepared_tempo::MidiTempoBreaks);
        for chain in self
            .device_chains
            .iter_mut()
            .chain(self.source_effects.chains_mut())
            .chain(self.return_chains.iter_mut())
            .chain(std::iter::once(&mut self.main_chain))
        {
            chain.begin_block();
        }
        let compensation = self
            .compensation
            .as_mut()
            .ok_or(Invalid("compensation runtime is unavailable"))?;
        compensation.delays.begin_transaction().map_err(error)?;
        let result = (|| {
            for (offset, destination) in output.iter_mut().enumerate() {
                let frame = start
                    .checked_add(offset as u64)
                    .ok_or(Invalid("render range overflow"))?;
                let end = frame
                    .checked_add(1)
                    .ok_or(Invalid("render range overflow"))?;
                let beat = project
                    .tempo_map
                    .as_ref()
                    .ok_or(Invalid("compensated timing map is unavailable"))?
                    .frame_to_beat(frame)
                    .map_err(|e| Invalid(e.0))?;
                compensation
                    .delays
                    .push_transport_stamp(TransportStamp {
                        tempo_micro_bpm: project
                            .tempo_map
                            .as_ref()
                            .map(|map| map.tempo_at(beat).micro_bpm()),
                        performance_frame: frame,
                        transport_is_running: true,
                        arrangement_frame: frame,
                        loop_pass: 0,
                        performance_beat_raw: beat.raw(),
                        arrangement_beat_raw: Some(beat.raw()),
                        meter: None,
                        cycle_beats_raw: None,
                    })
                    .map_err(error)?;
                for scratch in &mut self.return_scratch {
                    scratch[0] = [0.0; 2];
                }
                compensation.inputs.fill([0.0; 2]);
                compensation.main.fill([0.0; 2]);
                for order in 0..compensation.graph.owner_order.len() {
                    match compensation.graph.owner_order[order] {
                        Owner::Track(index) => {
                            let track = &project.tracks[index];
                            let mut audio = [[0.0; 2]];
                            let mut generator = [[0.0; 2]];
                            if track.audio_content_enabled() {
                                track.render_raw_audio(frame, &mut audio, pages.as_deref_mut())?;
                            }
                            crate::render_source_generators(
                                project,
                                index,
                                frame,
                                end,
                                &mut generator,
                                volume_timing,
                            )?;
                            self.source_effects.process_isolated(
                                project,
                                index,
                                frame,
                                &mut generator,
                            )?;
                            let processed = compensation.process_track(
                                index,
                                track,
                                &mut self.device_chains[index],
                                audio[0],
                                generator[0],
                                offset,
                                false,
                                &mut None,
                            )?;
                            let mixed = compensation.fader(track, index, None, processed)?;
                            compensation.route_track_output(index, mixed)?;
                            for feed_index in 0..compensation.graph.tracks[index].audio_feeds.len()
                            {
                                let feed =
                                    &compensation.graph.tracks[index].audio_feeds[feed_index];
                                let (target, gain, edge) = (feed.target, feed.gain, feed.edge);
                                let sent =
                                    compensation.edge(edge, [mixed[0] * gain, mixed[1] * gain])?;
                                compensation.inputs[target][0] += sent[0];
                                compensation.inputs[target][1] += sent[1];
                            }
                            for send_index in 0..compensation.graph.tracks[index].sends.len() {
                                let send = &compensation.graph.tracks[index].sends[send_index];
                                let (target, gain, edge) = (send.target, send.gain, send.edge);
                                let sent =
                                    compensation.edge(edge, [mixed[0] * gain, mixed[1] * gain])?;
                                self.return_scratch[target][0][0] += sent[0];
                                self.return_scratch[target][0][1] += sent[1];
                            }
                        }
                        Owner::Return(index) => {
                            let bus = &project.return_buses[index];
                            let mut returned = self.return_scratch[index][0];
                            compensation.process_return(
                                index,
                                &mut self.return_chains[index],
                                &mut returned,
                                offset,
                                &mut None,
                            )?;
                            if let Some(matrix) = bus.matrix {
                                returned = matrix.apply(returned);
                            }
                            returned = if bus.muted {
                                [0.0; 2]
                            } else {
                                [returned[0] * bus.gain, returned[1] * bus.gain]
                            };
                            for feed_index in 0..compensation.graph.returns[index].audio_feeds.len()
                            {
                                let feed =
                                    &compensation.graph.returns[index].audio_feeds[feed_index];
                                let (target, gain, edge) = (feed.target, feed.gain, feed.edge);
                                let sent = compensation
                                    .edge(edge, [returned[0] * gain, returned[1] * gain])?;
                                compensation.inputs[target][0] += sent[0];
                                compensation.inputs[target][1] += sent[1];
                            }
                            for feed_index in 0..compensation.graph.returns[index].feeds.len() {
                                let feed = &compensation.graph.returns[index].feeds[feed_index];
                                let (target, gain, edge) = (feed.target, feed.gain, feed.edge);
                                let sent = compensation
                                    .edge(edge, [returned[0] * gain, returned[1] * gain])?;
                                self.return_scratch[target][0][0] += sent[0];
                                self.return_scratch[target][0][1] += sent[1];
                            }
                            self.return_scratch[index][0] = compensation
                                .edge(compensation.graph.returns[index].main, returned)?;
                        }
                    }
                }
                for main in &compensation.main {
                    destination[0] += main[0];
                    destination[1] += main[1];
                }
                for scratch in &self.return_scratch {
                    destination[0] += scratch[0][0];
                    destination[1] += scratch[0][1];
                }
                compensation.process_main(
                    &mut self.main_chain,
                    std::slice::from_mut(destination),
                    offset,
                    &mut None,
                )?;
            }
            Ok::<(), Invalid>(())
        })();
        if let Err(error) = result {
            compensation
                .delays
                .rollback_transaction()
                .map_err(super::error)?;
            output.fill([0.0; 2]);
            return Err(error);
        }
        compensation.delays.commit_transaction().map_err(error)?;
        for chain in self
            .device_chains
            .iter_mut()
            .chain(self.source_effects.chains_mut())
            .chain(self.return_chains.iter_mut())
            .chain(std::iter::once(&mut self.main_chain))
        {
            chain.commit_block();
        }
        project.master_gain.apply(output);
        Ok(())
    }
}
