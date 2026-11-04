//! Shared compensated/typed-route path. Upstream latency and typed audio routes
//! use single-frame routing. Serial Main-only latency retains batched upstream
//! rendering; its stamps, Main processors and retained tails still advance one
//! sample at a time through the same compensation owner.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct BatchedSources<'a> {
    pub(super) index: usize,
    pub(super) audio: &'a [bool; MAX_TRACKS],
    pub(super) analytic: &'a [bool; MAX_TRACKS],
}

impl SessionPlayback {
    /// Prepare resident audio and eligible analytic MIDI. A shared receiver uses
    /// its scratch only when its active route has no local audio source.
    /// Active suppressions or continuations retain per-frame scheduling. No page provider is read and
    /// per-frame DSP/routing order is unchanged. The caller bounds each window
    /// before every transport or route discontinuity.
    pub(super) fn prepare_resident_sources(
        &mut self,
        performance_frame: u64,
        arrangement_frame: u64,
        frames: usize,
        eligible: &mut [bool; MAX_TRACKS],
        analytic: &mut [bool; MAX_TRACKS],
    ) -> Result<(), SessionPlaybackError> {
        if frames > 256 || self.resident_source_scratch.len() != self.route_scratch.len() {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        // Keep small Session graphs on the original one-frame path; larger
        // graphs can amortize the preparation work across the window.
        let batch_session = self
            .route_scratch
            .iter()
            .filter(|route| matches!(route.owner, RouteOwner::SessionAudio { .. }))
            .take(4)
            .count()
            == 4;
        let loop_fade = self
            .active_loop_region_at(arrangement_frame)
            .and_then(|region| {
                crate::loop_seam::LoopEndFade::new(
                    region,
                    self.revision.arrangement().sample_rate(),
                )
            });
        eligible.fill(false);
        analytic.fill(false);
        let batch_analytic = self.suppression_scratch.is_empty()
            && self.analytic_continuations.iter().all(Option::is_none);
        let timing = timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor);
        for (index, (route, scratch)) in self
            .route_scratch
            .iter()
            .zip(self.resident_source_scratch.iter_mut())
            .enumerate()
        {
            let output = &mut scratch[..frames];
            output.fill([0.0; 2]);
            if batch_analytic && self.revision.source_routing().is_shared(index) {
                let track = self
                    .revision
                    .arrangement()
                    .tracks()
                    .get(index)
                    .ok_or(SessionPlaybackError::UnknownTrack)?;
                let audio_free = match route.owner {
                    RouteOwner::Arrangement => track.clips.is_empty(),
                    RouteOwner::Session { .. } | RouteOwner::Silence => true,
                    RouteOwner::SessionAudio { .. } => false,
                };
                if audio_free
                    && track.receives_source_audio()
                    && self
                        .mutable_instruments
                        .get(index)
                        .is_some_and(Option::is_none)
                {
                    let floor = self
                        .historical_floor
                        .into_iter()
                        .chain(self.route_historical_floors.get(index).copied().flatten())
                        .max();
                    render_source_instrument_chunk(
                        RouteSourceContext {
                            revision: &self.revision,
                            midi: crate::session_inventory::EffectiveSessionMidi::new(
                                self.revision.session(),
                                self.midi_admission.as_ref(),
                            ),
                            route: effective_midi_route(
                                &self.revision,
                                &self.route_scratch,
                                index,
                            )?,
                            historical_floor: floor,
                            suppressions: &self.suppression_scratch,
                            continuations: &self.analytic_continuations,
                            index,
                            raw: true,
                            loop_fade: None,
                            timing: &timing,
                            table: self.revision.session().sine_table(),
                        },
                        &self.route_scratch,
                        self.historical_floor,
                        &self.route_historical_floors,
                        &mut self.spans,
                        performance_frame,
                        arrangement_frame,
                        output,
                    )?;
                    *analytic
                        .get_mut(index)
                        .ok_or(SessionPlaybackError::DeviceRuntime)? = true;
                    continue;
                }
            }
            match route.owner {
                RouteOwner::Arrangement => {
                    let track = self
                        .revision
                        .arrangement()
                        .tracks()
                        .get(index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    if batch_analytic
                        && track.clips.is_empty()
                        && track.midi_source_index.is_none()
                        && !self.revision.source_routing().is_shared(index)
                        && self
                            .mutable_instruments
                            .get(index)
                            .is_some_and(Option::is_none)
                        && track
                            .instrument
                            .as_ref()
                            .is_some_and(|instrument| !instrument.mutable)
                    {
                        track
                            .render_raw_instrument(
                                arrangement_frame,
                                performance_frame,
                                output,
                                &self.suppression_scratch,
                                &self.analytic_continuations,
                                Some(self.revision.session().timing()),
                            )
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        *analytic
                            .get_mut(index)
                            .ok_or(SessionPlaybackError::DeviceRuntime)? = true;
                    } else if !track.has_streamed_clips {
                        if track.audio_content_enabled() {
                            track
                                .render_raw_audio::<dyn MediaPages>(arrangement_frame, output, None)
                                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                            if let Some(fade) = loop_fade {
                                fade.apply(arrangement_frame, output);
                            }
                        }
                        *eligible
                            .get_mut(index)
                            .ok_or(SessionPlaybackError::DeviceRuntime)? = true;
                    }
                }
                RouteOwner::Session { clip, .. } if batch_analytic => {
                    let track = self
                        .revision
                        .arrangement()
                        .tracks()
                        .get(index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    if track.midi_source_index.is_some()
                        || self.revision.source_routing().is_shared(index)
                        || !self
                            .mutable_instruments
                            .get(index)
                            .is_some_and(Option::is_none)
                    {
                        continue;
                    }
                    let midi = crate::session_inventory::EffectiveSessionMidi::new(
                        self.revision.session(),
                        self.midi_admission.as_ref(),
                    );
                    let prepared = midi
                        .midi_clip_for_track(clip, route.id)
                        .ok_or(SessionPlaybackError::UnknownClip)?;
                    if prepared.track != route.id {
                        return Err(SessionPlaybackError::ClipTrackMismatch);
                    }
                    if !prepared.instrument_enabled()
                        || prepared.instrument_template().is_event_source()
                    {
                        continue;
                    }
                    let floor = self
                        .historical_floor
                        .into_iter()
                        .chain(self.route_historical_floors.get(index).copied().flatten())
                        .max();
                    render_route_source_chunk::<dyn MediaPages, _>(
                        RouteSourceContext {
                            revision: &self.revision,
                            midi,
                            route: *route,
                            historical_floor: floor,
                            suppressions: &self.suppression_scratch,
                            continuations: &self.analytic_continuations,
                            index,
                            raw: true,
                            loop_fade: None,
                            timing: &timing,
                            table: self.revision.session().sine_table(),
                        },
                        &mut self.spans,
                        performance_frame,
                        arrangement_frame,
                        output,
                        None,
                        SourceStage::Instrument,
                    )?;
                    *analytic
                        .get_mut(index)
                        .ok_or(SessionPlaybackError::DeviceRuntime)? = true;
                }
                // A fading outgoing clip renders on the per-frame route path.
                RouteOwner::SessionAudio { clip, launch_frame }
                    if batch_session && route.audio_release.is_none() =>
                {
                    let track = self
                        .revision
                        .arrangement()
                        .tracks()
                        .get(index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    let is_eligible = eligible
                        .get_mut(index)
                        .ok_or(SessionPlaybackError::DeviceRuntime)?;
                    if !track.audio_content_enabled() {
                        *is_eligible = true;
                        continue;
                    }
                    let prepared = self
                        .revision
                        .session()
                        .audio_clip(clip)
                        .ok_or(SessionPlaybackError::UnknownClip)?;
                    if prepared.track != route.id {
                        return Err(SessionPlaybackError::ClipTrackMismatch);
                    }
                    if let PreparedAudioSource::Resident(audio) = prepared.source() {
                        if prepared.active {
                            render_resident_session_audio(
                                prepared,
                                audio,
                                launch_frame,
                                performance_frame,
                                output,
                                crate::PreparedTrackMix::identity(),
                            )?;
                        }
                        *is_eligible = true;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    /// Publish one transport origin immediately before processing its Main
    /// sample. Pre-pushing a whole block could overwrite the bounded stamp ring.
    pub(super) fn push_compensated_stamp(
        &mut self,
        performance_frame: u64,
        arrangement_frame: u64,
        loop_pass: u64,
    ) -> Result<(), SessionPlaybackError> {
        let timing = timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor);
        let beat = timing
            .frame_to_beat(performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        self.compensation
            .as_mut()
            .ok_or(SessionPlaybackError::DeviceRuntime)?
            .delays
            .push_transport_stamp(crate::compensation::TransportStamp {
                tempo_micro_bpm: timing.micro_bpm_at_frame(performance_frame).ok(),
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
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_compensated_route_frame<P: MediaPages + ?Sized>(
        &mut self,
        performance_frame: u64,
        arrangement_frame: u64,
        loop_pass: u64,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
        frame_offset: usize,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        capture: &mut Option<&mut dyn CaptureSink>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        external: Option<hex_lattice::NativeExternalAutomation<'_>>,
        resident_source: Option<BatchedSources<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        let loop_fade = self
            .active_loop_region_at(arrangement_frame)
            .and_then(|region| {
                crate::loop_seam::LoopEndFade::new(
                    region,
                    self.revision.arrangement().sample_rate(),
                )
            });
        if output.len() != 1 {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        self.push_compensated_stamp(performance_frame, arrangement_frame, loop_pass)?;
        let grouped = self
            .compensation
            .as_ref()
            .is_some_and(|runtime| runtime.graph.has_group_outputs());
        let mut detached = if grouped {
            Some([[0.0; 2]; MAX_TRACKS])
        } else {
            None
        };
        if let Some(detached) = detached.as_mut() {
            self.render_detached_tails(
                performance_frame,
                output,
                frame_offset,
                capture,
                Some(detached),
            )?;
        }
        let region = self.active_loop_region();
        let timing = timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor);
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
                        let analytic_index = resident_source
                            .filter(|batch| batch.analytic.get(index).copied().unwrap_or(false))
                            .map(|batch| batch.index);
                        if analytic_index.is_some() {
                            // This route has no audio source; its scratch window
                            // holds the instrument side of the ordered graph.
                        } else if let Some(sample_index) = resident_source
                            .filter(|batch| batch.audio.get(index).copied().unwrap_or(false))
                            .map(|batch| batch.index)
                        {
                            audio[0] = *self
                                .resident_source_scratch
                                .get(index)
                                .and_then(|samples| samples.get(sample_index))
                                .ok_or(SessionPlaybackError::DeviceRuntime)?;
                        } else {
                            render_route_source_chunk(
                                RouteSourceContext {
                                    revision: &self.revision,
                                    midi: crate::session_inventory::EffectiveSessionMidi::new(
                                        self.revision.session(),
                                        self.midi_admission.as_ref(),
                                    ),
                                    route,
                                    historical_floor: self.historical_floor_for(index),
                                    suppressions: &self.suppression_scratch,
                                    continuations: &self.analytic_continuations,
                                    index,
                                    // Retained Main history can keep this path active
                                    // after the current graph returns to a fused mix.
                                    // The content-origin fader below owns that mix.
                                    raw: true,
                                    loop_fade,
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
                        }
                        // An ordinary arrangement audio track without a MIDI
                        // source cannot contribute instrument samples. Shared
                        // sources and session routes must still use the normal
                        // scheduler, even when this track has no local instrument.
                        if let Some(sample_index) = analytic_index {
                            generator[0] = *self
                                .resident_source_scratch
                                .get(index)
                                .and_then(|samples| samples.get(sample_index))
                                .ok_or(SessionPlaybackError::DeviceRuntime)?;
                        } else if track.instrument.is_some()
                            || track.midi_source_index.is_some()
                            || self.revision.source_routing().is_shared(index)
                            || !matches!(route.owner, RouteOwner::Arrangement)
                        {
                            render_source_instrument_chunk(
                                RouteSourceContext {
                                    revision: &self.revision,
                                    midi: crate::session_inventory::EffectiveSessionMidi::new(
                                        self.revision.session(),
                                        self.midi_admission.as_ref(),
                                    ),
                                    route: effective_midi_route(
                                        &self.revision,
                                        &self.route_scratch,
                                        index,
                                    )?,
                                    historical_floor: self.historical_floor_for(index),
                                    suppressions: &self.suppression_scratch,
                                    continuations: &self.analytic_continuations,
                                    index,
                                    raw: true,
                                    loop_fade: None,
                                    timing: &timing,
                                    table: self.revision.session().sine_table(),
                                },
                                &self.route_scratch,
                                self.historical_floor,
                                &self.route_historical_floors,
                                &mut self.spans,
                                performance_frame,
                                arrangement_frame,
                                &mut generator,
                            )?;
                        }
                        if self.source_input_receiver() == Some(index) {
                            let authored = self
                                .revision
                                .source_routing()
                                .contributors(index)
                                .iter()
                                .try_fold(0usize, |sum, &contributor| {
                                    sum.checked_add(
                                        self.source_voice_envelope(usize::from(contributor)),
                                    )
                                })
                                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                            source_live::render_source_live_input(
                                self.computer_keyboard.as_mut().map(Box::as_mut),
                                self.mutable_instruments[index].as_mut(),
                                &mut self.mutable_onsets,
                                &mut self.mutable_events,
                                authored,
                                performance_frame,
                                &mut generator,
                                &timing,
                                true,
                            )?;
                        }
                        render_mutable_instrument_chunk(
                            MutableInstrumentChunkContext {
                                revision: &self.revision,
                                midi: crate::session_inventory::EffectiveSessionMidi::new(
                                    self.revision.session(),
                                    self.midi_admission.as_ref(),
                                ),
                                sources,
                                routes: &self.route_scratch,
                                runtimes: &mut self.mutable_instruments,
                                onsets: &mut self.mutable_onsets,
                                suppressions: &self.suppression_scratch,
                                events: &mut self.mutable_events,
                                spans: &mut self.spans,
                                performance_frame,
                                arrangement_frame,
                                arrangement_loop_pass: loop_pass,
                                chase_at_start: self.interactive_start_frame
                                    == Some(performance_frame),
                                loop_region: if self.linear_arrangement_export {
                                    None
                                } else {
                                    region.filter(|region| region.enabled())
                                },
                                timing: &timing,
                            },
                            index,
                            &mut generator,
                        )?;
                    }
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
                                self.playing,
                                &mut generator,
                            )?;
                        }
                    }
                    if !self.playing {
                        if self.source_input_receiver() == Some(index) {
                            source_live::render_source_live_input(
                                self.computer_keyboard.as_mut().map(Box::as_mut),
                                self.mutable_instruments[index].as_mut(),
                                &mut self.mutable_onsets,
                                &mut self.mutable_events,
                                0,
                                performance_frame,
                                &mut generator,
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
                                    performance_frame,
                                    &mut generator,
                                    &mut self.mutable_onsets,
                                    &[],
                                    &mut self.mutable_events,
                                    Some(&source_clock),
                                    &hex_lattice::EMPTY_DEVICE_RESOURCES,
                                    None,
                                )
                                .map_err(|error| error.render_error())?;
                        }
                    }
                    if let Some(keyboard) = self.computer_keyboard.as_mut().filter(|keyboard| {
                        !keyboard.is_source_input() && keyboard.track() == route.id
                    }) {
                        let clock = keyboard
                            .musical_clock(&timing, performance_frame, self.playing)
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        keyboard
                            .render(&mut generator, Some(&clock))
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    }
                    for keyboard in self
                        .current_sampler_keyboards
                        .iter_mut()
                        .filter(|keyboard| keyboard.track() == route.id)
                    {
                        let source = sources.ok_or(SessionPlaybackError::DeviceRuntime)?;
                        let resources = source
                            .resources(keyboard.track())
                            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                        keyboard
                            .render_with_resources(&mut generator, None, &resources)
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    }
                    let compensation = self
                        .compensation
                        .as_mut()
                        .ok_or(SessionPlaybackError::DeviceRuntime)?;
                    let processed = compensation
                        .process_track_with_foreign(
                            index,
                            track,
                            &mut self.device_chains[index],
                            audio[0],
                            generator[0],
                            frame_offset,
                            self.device_metering,
                            foreign,
                            observer,
                            self.native_parameter_state
                                .as_deref_mut()
                                .and_then(|state| state.processing.as_mut()),
                            external,
                        )
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    let override_gain = self
                        .gain_recording_override
                        .filter(|_| self.revision.gain_recording_track() == Some(route.id));
                    let mix = compensation.fader_mix(track, index, override_gain);
                    let mixed = if let Some(mix) = mix {
                        let fader = &mut self.track_fader_scratch[index];
                        let mix = if override_gain.is_some() {
                            fader.retarget(
                                mix.channel_gain(),
                                self.revision.arrangement().sample_rate(),
                            );
                            mix.with_channel_gain(fader.next())
                        } else if track.has_gain_automation() {
                            fader.follow(mix.channel_gain());
                            mix
                        } else {
                            mix.with_channel_gain(fader.next())
                        };
                        mix.apply_frame(processed)
                    } else {
                        [0.0; 2]
                    };
                    // Retired tails retain their old post-fader output; they do
                    // not re-enter current authored sends or feeds.
                    let routed = if let Some(detached) = detached.as_ref() {
                        [mixed[0] + detached[index][0], mixed[1] + detached[index][1]]
                    } else {
                        mixed
                    };
                    let main = compensation
                        .route_track_output(index, routed)
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
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
                        .effects_with_foreign(
                            &compensation.graph.returns[index].devices,
                            &mut self.return_chains[index],
                            0,
                            &mut frame,
                            frame_offset,
                            foreign,
                            observer,
                            self.native_parameter_state
                                .as_deref_mut()
                                .and_then(|state| state.processing.as_mut()),
                            external,
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
        if !grouped {
            self.render_detached_tails(performance_frame, output, frame_offset, capture, None)?;
        }
        if self.playing {
            self.return_effect_tails
                .render_chunk(output, frame_offset, capture, foreign)?;
        }
        Ok(())
    }
}
