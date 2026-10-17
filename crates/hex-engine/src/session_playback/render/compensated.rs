//! Shared compensated/typed-route path. One frame keeps content clocks exact;
//! zero-latency projects without typed routes retain the original chunk kernel.
use super::*;

impl SessionPlayback {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_compensated_route_frame<P: MediaPages + ?Sized>(
        &mut self,
        performance_frame: u64,
        arrangement_frame: u64,
        loop_pass: u64,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
        frame_offset: usize,
        observer: &mut Option<&mut crate::PostNodeObserver<'_>>,
        capture: &mut Option<&mut OutputCapture<'_, '_>>,
    ) -> Result<(), SessionPlaybackError> {
        if output.len() != 1 {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        let timing = timing_view_for(self.revision.session(), self.periodic_tempo, self.anchor);
        let beat = timing
            .frame_to_beat(performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        self.compensation
            .as_mut()
            .ok_or(SessionPlaybackError::DeviceRuntime)?
            .delays
            .push_transport_stamp(crate::compensation::TransportStamp {
                performance_frame,
                transport_is_running: self.playing,
                arrangement_frame,
                loop_pass,
                performance_beat_raw: beat.raw(),
                arrangement_beat_raw: self
                    .revision
                    .session()
                    .timing()
                    .frame_to_beat(arrangement_frame)
                    .ok()
                    .map(|beat| beat.raw()),
            })
            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
        for scratch in &mut self.return_scratch {
            scratch[0] = [0.0; 2];
        }
        let compensation = self
            .compensation
            .as_mut()
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        compensation.inputs.fill([0.0; 2]);
        compensation.main.fill([0.0; 2]);
        let owner_count = compensation.graph.owner_order.len();
        for order in 0..owner_count {
            let owner = self
                .compensation
                .as_ref()
                .ok_or(SessionPlaybackError::DeviceRuntime)?
                .graph
                .owner_order[order];
            match owner {
                crate::signal_graph::Owner::Track(index) => {
                    let track = &self.revision.arrangement().tracks()[index];
                    let route = self.route_scratch[index];
                    let mut audio = [[0.0; 2]];
                    let mut generator = [[0.0; 2]];
                    if self.playing {
                        render_route_source_chunk(
                            RouteSourceContext {
                                revision: &self.revision,
                                route,
                                historical_floor: self.historical_floor_for(index),
                                suppressions: &self.suppression_scratch,
                                continuations: &self.analytic_continuations,
                                index,
                                raw: false,
                                timing: &timing,
                                table: self.revision.session().sine_table(),
                            },
                            &mut self.spans,
                            performance_frame,
                            arrangement_frame,
                            &mut audio,
                            pages.as_deref_mut(),
                            SourceStage::Audio,
                        )?;
                        for release in &self.release_scratch {
                            if release.location.track_index() == index
                                && release.location.tail_group().is_none()
                            {
                                render_release_raw(
                                    release,
                                    self.revision.session().sine_table(),
                                    self.revision.arrangement().shared_contexts(),
                                    &self.retained_contexts,
                                    &self.analytic_continuations,
                                    performance_frame,
                                    &mut generator,
                                )?;
                            }
                        }
                        render_route_source_chunk(
                            RouteSourceContext {
                                revision: &self.revision,
                                route: effective_midi_route(
                                    &self.revision,
                                    &self.route_scratch,
                                    index,
                                )?,
                                historical_floor: self.historical_floor_for(index),
                                suppressions: &self.suppression_scratch,
                                continuations: &self.analytic_continuations,
                                index,
                                raw: false,
                                timing: &timing,
                                table: self.revision.session().sine_table(),
                            },
                            &mut self.spans,
                            performance_frame,
                            arrangement_frame,
                            &mut generator,
                            pages.as_deref_mut(),
                            SourceStage::Instrument,
                        )?;
                        render_mutable_instrument_chunk(
                            MutableInstrumentChunkContext {
                                revision: &self.revision,
                                routes: &self.route_scratch,
                                runtimes: &mut self.mutable_instruments,
                                onsets: &mut self.mutable_onsets,
                                suppressions: &self.suppression_scratch,
                                events: &mut self.mutable_events,
                                spans: &mut self.spans,
                                performance_frame,
                                arrangement_frame,
                                arrangement_loop_pass: loop_pass,
                                loop_region: if self.linear_arrangement_export {
                                    None
                                } else {
                                    self.revision
                                        .arrangement()
                                        .loop_region()
                                        .filter(|region| region.enabled())
                                },
                                timing: &timing,
                            },
                            index,
                            &mut generator,
                        )?;
                    }
                    if let Some(keyboard) = self
                        .computer_keyboard
                        .as_mut()
                        .filter(|keyboard| keyboard.track() == route.id)
                    {
                        let clock = keyboard
                            .musical_clock(&timing, performance_frame, self.playing)
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        keyboard
                            .render(&mut generator, Some(&clock))
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    }
                    let compensation = self
                        .compensation
                        .as_mut()
                        .ok_or(SessionPlaybackError::DeviceRuntime)?;
                    let processed = compensation
                        .process_track(
                            index,
                            track,
                            &mut self.device_chains[index],
                            audio[0],
                            generator[0],
                            frame_offset,
                            self.device_metering,
                            observer,
                        )
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    let override_gain = self
                        .gain_recording_override
                        .filter(|_| self.revision.gain_recording_track() == Some(route.id));
                    let mixed = compensation
                        .fader(track, index, override_gain, processed)
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    let main = compensation
                        .edge(compensation.graph.tracks[index].main, mixed)
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    compensation.main[index] = main;
                    if let Some(capture) = capture.as_mut() {
                        capture.add(OutputCaptureId::Track(route.id), frame_offset, &[main])?;
                    }
                    for feed_index in 0..compensation.graph.tracks[index].audio_feeds.len() {
                        let feed = &compensation.graph.tracks[index].audio_feeds[feed_index];
                        let (target, gain, edge) = (feed.target, feed.gain, feed.edge);
                        let sent = compensation
                            .edge(edge, [mixed[0] * gain, mixed[1] * gain])
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        compensation.inputs[target][0] += sent[0];
                        compensation.inputs[target][1] += sent[1];
                    }
                    for send_index in 0..compensation.graph.tracks[index].sends.len() {
                        let send = &compensation.graph.tracks[index].sends[send_index];
                        let (target, gain, edge) = (send.target, send.gain, send.edge);
                        let sent = compensation
                            .edge(edge, [mixed[0] * gain, mixed[1] * gain])
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        self.return_scratch[target][0][0] += sent[0];
                        self.return_scratch[target][0][1] += sent[1];
                    }
                }
                crate::signal_graph::Owner::Return(index) => {
                    let compensation = self
                        .compensation
                        .as_mut()
                        .ok_or(SessionPlaybackError::DeviceRuntime)?;

                    let bus = &self.revision.arrangement().return_buses()[index];
                    let mut frame = self.return_scratch[index][0];
                    compensation
                        .effects(
                            &compensation.graph.returns[index].devices,
                            &mut self.return_chains[index],
                            0,
                            &mut frame,
                            frame_offset,
                            observer,
                        )
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    if let Some(matrix) = bus.matrix {
                        frame = matrix.apply(frame);
                    }
                    frame = if bus.muted {
                        [0.0; 2]
                    } else {
                        [frame[0] * bus.gain, frame[1] * bus.gain]
                    };
                    for feed_index in 0..compensation.graph.returns[index].audio_feeds.len() {
                        let feed = &compensation.graph.returns[index].audio_feeds[feed_index];
                        let (target, gain, edge) = (feed.target, feed.gain, feed.edge);
                        let sent = compensation
                            .edge(edge, [frame[0] * gain, frame[1] * gain])
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        compensation.inputs[target][0] += sent[0];
                        compensation.inputs[target][1] += sent[1];
                    }
                    for feed_index in 0..compensation.graph.returns[index].feeds.len() {
                        let feed = &compensation.graph.returns[index].feeds[feed_index];
                        let (target, gain, edge) = (feed.target, feed.gain, feed.edge);
                        let sent = compensation
                            .edge(edge, [frame[0] * gain, frame[1] * gain])
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        self.return_scratch[target][0][0] += sent[0];
                        self.return_scratch[target][0][1] += sent[1];
                    }
                    let main = compensation
                        .edge(compensation.graph.returns[index].main, frame)
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    self.return_scratch[index][0] = main;
                    if let Some(capture) = capture.as_mut() {
                        capture.add(OutputCaptureId::Return(bus.id), frame_offset, &[main])?;
                    }
                }
            }
        }
        for main in &self
            .compensation
            .as_ref()
            .ok_or(SessionPlaybackError::DeviceRuntime)?
            .main
        {
            output[0][0] += main[0];
            output[0][1] += main[1];
        }
        for scratch in &self.return_scratch {
            output[0][0] += scratch[0][0];
            output[0][1] += scratch[0][1];
        }
        // Only zero-latency routed layouts admit detached old chains. Positive
        // latency revisions refuse those transitions before moving audible owners.
        if self.playing {
            self.render_detached_tails(performance_frame, output, frame_offset, capture)?;
            self.return_effect_tails
                .render_chunk(output, frame_offset, capture)?;
        }
        Ok(())
    }
}
