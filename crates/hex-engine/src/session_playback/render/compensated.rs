//! Shared compensated/typed-route path. Upstream latency and typed audio routes
//! normally use single-frame routing. Independent resident audio tracks can
//! batch native stages, faders and alignment. Serial Main-only latency retains batched upstream
//! rendering; its stamps, Main processors and retained tails still advance one
//! sample at a time through the same compensation owner.
use super::*;
use crate::compensation::{OriginWindow, TransportStamp};

#[derive(Clone, Copy)]
pub(super) struct BatchedSources<'a> {
    pub(super) index: usize,
    pub(super) audio: &'a [bool; MAX_TRACKS],
    pub(super) analytic: &'a [bool; MAX_TRACKS],
    pub(super) stamps: Option<&'a [Option<TransportStamp>]>,
    pub(super) aligned: &'a [bool; MAX_TRACKS],
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
            let work_start = self.render_work.track(index);
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
                    self.render_work.finish(work_start);
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
                        self.render_work.finish(work_start);
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
                        self.render_work.finish(work_start);
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
                        self.render_work.finish(work_start);
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
            self.render_work.finish(work_start);
        }
        Ok(())
    }
    /// Advance independent resident audio owners and their Main alignment
    /// rings. The frame loop still pushes stamps and sums in its original order.
    pub(super) fn align_resident_tracks(
        &mut self,
        arrangement_frame: u64,
        frames: usize,
        eligible: &[bool; MAX_TRACKS],
        origins: Option<(&[Option<TransportStamp>], u32)>,
        aligned: &mut [bool; MAX_TRACKS],
    ) -> Result<(), SessionPlaybackError> {
        aligned.fill(false);
        let Some(compensation) = self.compensation.as_mut() else {
            return Ok(());
        };
        if !compensation.graph.independent_main_routes() {
            return Ok(());
        }
        arrangement_frame
            .checked_add(frames as u64)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        for (index, ((route, scratch), aligned)) in self
            .route_scratch
            .iter()
            .zip(&mut self.resident_source_scratch)
            .zip(aligned)
            .enumerate()
        {
            if !eligible[index]
                || !matches!(route.owner, RouteOwner::Arrangement)
                || self.revision.gain_recording_track() == Some(route.id)
            {
                continue;
            }
            let track = &self.revision.arrangement().tracks()[index];
            let Some((edge, arrival)) = compensation.graph.resident_main_alignment(index) else {
                continue;
            };
            if track.instrument_id.is_some() || track.instrument_position() != 0 {
                continue;
            }
            let chain = &mut self.device_chains[index];
            let native_origins = if chain.is_empty() {
                None
            } else {
                let Some((incoming, _)) = origins.filter(|&(_, history_frames)| {
                    // In this independent serial track, the fader arrival
                    // bounds the lookback of every native device stage.
                    arrival <= history_frames
                        && track.device_chain.iter().all(|device| {
                            !matches!(
                                device.definition.as_ref(),
                                Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
                            )
                        })
                }) else {
                    continue;
                };
                Some(OriginWindow {
                    history: &compensation.delays,
                    incoming,
                    delay: arrival,
                })
            };
            let work_start = self.render_work.track(index);
            let samples = &mut scratch[..frames];
            for frame in samples.iter_mut() {
                // Match the two zero-valued merges in the scalar route.
                frame[0] += 0.0;
                frame[1] += 0.0;
            }
            track.apply_instrument_input_gain(samples);
            for frame in samples.iter_mut() {
                frame[0] += 0.0;
                frame[1] += 0.0;
            }
            track.apply_instrument_output_gain(samples);
            if let Some(origins) = native_origins.as_ref() {
                for (stage, &node) in compensation.graph.tracks[index].devices.iter().enumerate() {
                    let planned = &compensation.graph.plan.nodes()[node];
                    let input = OriginWindow {
                        delay: planned.input_arrival_frames,
                        ..*origins
                    };
                    let output = OriginWindow {
                        delay: planned.output_arrival_frames,
                        ..*origins
                    };
                    crate::native_parameter_input::process_range_with_key(
                        None,
                        None,
                        chain,
                        stage..stage + 1,
                        samples,
                        0,
                        Some(&input),
                        Some(&output),
                        planned.input_arrival_frames,
                        planned.output_arrival_frames,
                        &mut hex_lattice::RefuseForeign,
                        None,
                        &mut None,
                    )
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                }
            }
            let fader = &mut self.track_fader_scratch[index];
            let automated = track.has_gain_automation();
            for (offset, frame) in samples.iter_mut().enumerate() {
                let mix = if automated {
                    let content = if let Some(origins) = native_origins.as_ref() {
                        origins
                            .stamp(offset)
                            .ok_or(SessionPlaybackError::DeviceRuntime)?
                            .arrangement_frame
                    } else {
                        arrangement_frame + offset as u64
                    };
                    track.mix_at(content)
                } else {
                    track.mix()
                };
                let mix = if automated {
                    fader.follow(mix.channel_gain());
                    mix
                } else {
                    mix.with_channel_gain(fader.next())
                };
                *frame = mix.apply_frame(*frame);
            }
            compensation
                .edge_block(edge, samples)
                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
            self.render_work.finish(work_start);
            *aligned = true;
        }
        Ok(())
    }
    /// Return the contiguous running history available to independent stages.
    /// A long delay on another track, such as a VST3 bridge, does not enlarge
    /// the native batching window or exclude tracks with shorter arrivals.
    pub(super) fn prepare_native_origin_window(
        &self,
        performance_frame: u64,
        arrangement_frame: u64,
        loop_pass: u64,
        stamps: &mut [Option<TransportStamp>],
    ) -> Result<Option<u32>, SessionPlaybackError> {
        let Some(compensation) = self.compensation.as_ref() else {
            return Ok(None);
        };
        if !compensation.graph.independent_main_routes()
            || self
                .native_parameter_state
                .as_deref()
                .is_some_and(|state| state.processing.is_some())
            || self.tail_output_scratch.iter().any(|active| *active)
            || self
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active)
        {
            return Ok(None);
        }
        let lookback = compensation.graph.main_latency().min(256);
        let history_frames = (0..lookback)
            .take_while(|&delay| {
                compensation
                    .delays
                    .stamp_at_delay(delay)
                    .is_some_and(|stamp| stamp.transport_is_running)
            })
            .count() as u32;
        for (offset, stamp) in stamps.iter_mut().enumerate() {
            let performance = performance_frame
                .checked_add(offset as u64)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            let arrangement = arrangement_frame
                .checked_add(offset as u64)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            *stamp = Some(self.compensated_stamp(performance, arrangement, loop_pass)?);
        }
        Ok(Some(history_frames))
    }

    fn compensated_stamp(
        &self,
        performance_frame: u64,
        arrangement_frame: u64,
        loop_pass: u64,
    ) -> Result<TransportStamp, SessionPlaybackError> {
        let timing = timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor);
        let beat = timing
            .frame_to_beat(performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        let session = self.revision.session();
        let arrangement_beat = session.timing().frame_to_beat(arrangement_frame).ok();
        Ok(TransportStamp {
            tempo_micro_bpm: timing.micro_bpm_at_frame(performance_frame).ok(),
            performance_frame,
            transport_is_running: self.playing,
            arrangement_frame,
            loop_pass,
            performance_beat_raw: beat.raw(),
            arrangement_beat_raw: arrangement_beat.map(hex_project::BeatTime::raw),
            meter: arrangement_beat.map(|beat| session.meter_map().meter_at(beat)),
            cycle_beats_raw: self
                .periodic_tempo_at(arrangement_frame)
                .map(|cycle| cycle.beat_range_raw()),
        })
    }

    /// Append a stamp immediately before consuming that frame. Pre-pushing
    /// a whole window could overwrite the bounded delay history.
    pub(super) fn push_compensated_stamp(
        &mut self,
        performance_frame: u64,
        arrangement_frame: u64,
        loop_pass: u64,
    ) -> Result<(), SessionPlaybackError> {
        let stamp = self.compensated_stamp(performance_frame, arrangement_frame, loop_pass)?;
        self.push_stamp(stamp)
    }

    fn push_stamp(&mut self, stamp: TransportStamp) -> Result<(), SessionPlaybackError> {
        self.compensation
            .as_mut()
            .ok_or(SessionPlaybackError::DeviceRuntime)?
            .delays
            .push_transport_stamp(stamp)
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
        if let Some(stamp) =
            resident_source.and_then(|batch| batch.stamps?.get(batch.index).copied().flatten())
        {
            self.push_stamp(stamp)?;
        } else {
            self.push_compensated_stamp(performance_frame, arrangement_frame, loop_pass)?;
        }
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
                    let work_start = self.render_work.track(index);
                    let track = &self.revision.arrangement().tracks()[index];
                    let route = self.route_scratch[index];
                    if let Some(batch) = resident_source.filter(|batch| batch.aligned[index]) {
                        let main = self.resident_source_scratch[index][batch.index];
                        self.compensation
                            .as_mut()
                            .ok_or(SessionPlaybackError::DeviceRuntime)?
                            .main[index] = main;
                        if let Some(capture) = capture.as_mut() {
                            capture.add_prepared_frame(
                                owner,
                                OutputCaptureId::Track(route.id),
                                frame_offset,
                                main,
                            )?;
                        }
                        continue;
                    }
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
                    if track.foreign_instrument.is_some() {
                        let clock =
                            crate::signal_graph::OriginClock(compensation.delays.stamp_at_delay(0));
                        let mut wet = [[0.0; 2]];
                        self.foreign_arrangement.render_frame(
                            index,
                            track,
                            foreign_arrangement::FrameContext {
                                performance: performance_frame,
                                arrangement: arrangement_frame,
                                loop_pass,
                                playing: self.playing,
                                chase: self.interactive_start_frame == Some(performance_frame),
                            },
                            &mut wet,
                            &mut self.mutable_onsets,
                            frame_offset,
                            foreign,
                            &clock,
                        )?;
                        generator[0][0] += wet[0][0];
                        generator[0][1] += wet[0][1];
                    }
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
                    let recording_track = self.revision.gain_recording_track() == Some(route.id);
                    let override_gain = self.gain_recording_override.filter(|_| recording_track);
                    let live_gain = if recording_track {
                        compensation.fader_content_frame(index).and_then(|frame| {
                            self.live_gain_scratch.next(
                                track,
                                frame,
                                override_gain,
                                self.revision.arrangement().sample_rate(),
                            )
                        })
                    } else {
                        None
                    };
                    let mix = compensation.fader_mix(track, index, live_gain);
                    let mixed = if let Some(mix) = mix {
                        let fader = &mut self.track_fader_scratch[index];
                        let mix = if track.has_gain_automation() || override_gain.is_some() {
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
                        capture.add_prepared_frame(
                            owner,
                            OutputCaptureId::Track(route.id),
                            frame_offset,
                            main,
                        )?;
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
                    self.render_work.finish(work_start);
                }
                crate::signal_graph::Owner::Return(index) => {
                    let work_start = self.render_work.return_bus(index);
                    let compensation = self
                        .compensation
                        .as_mut()
                        .ok_or(SessionPlaybackError::DeviceRuntime)?;

                    let bus = &self.revision.arrangement().return_buses()[index];
                    let mut frame = self.return_scratch[index][0];
                    compensation
                        .effects_with_foreign(
                            crate::signal_graph::Owner::Return(index),
                            0..compensation.graph.returns[index].devices.len(),
                            &mut self.return_chains[index],
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
                        capture.add_prepared_frame(
                            owner,
                            OutputCaptureId::Return(bus.id),
                            frame_offset,
                            main,
                        )?;
                    }
                    self.render_work.finish(work_start);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal_graph::tests::fixture;

    #[test]
    fn compensated_clock_keeps_meter_and_cycle_across_the_actual_loop_wrap() {
        use hex_project::{BeatTime, Command, Meter};

        let start = 23_998;
        let end = 24_002;
        let mut f = fixture(&[&[]], 24_010);
        let limiter = hex_lattice_contract::DeviceTypeId::from_static("hex.limiter");
        f.editor
            .execute(Command::InsertDevice {
                track: f.tracks[0],
                before: None,
                state: crate::default_native_device_state(limiter).unwrap(),
            })
            .unwrap();
        let seven_eight = Meter::new(7, 8).unwrap();
        f.editor
            .execute(Command::SetMeterPoint {
                beat: BeatTime::from_ratio(1, 1).unwrap(),
                meter: seven_eight,
            })
            .unwrap();
        f.editor
            .execute(Command::SetLoopRegion {
                start,
                end,
                enabled: true,
            })
            .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare(
                f.editor.project(),
                &f.media,
                Arc::new(crate::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let timing = revision.session().timing();
        let cycle = Some((
            timing.frame_to_beat(start).unwrap().raw(),
            timing.frame_to_beat(end).unwrap().raw(),
        ));
        let export = SessionPlayback::new_linear_arrangement_export(Arc::clone(&revision)).unwrap();
        let export_stamp = export.compensated_stamp(start, start, 0).unwrap();
        assert_eq!(
            (export_stamp.meter, export_stamp.cycle_beats_raw),
            (Some(Meter::COMMON_TIME), None)
        );
        let mut runtime = SessionPlayback::new(revision).unwrap();
        runtime.apply(SessionPlaybackCommand::Seek(start)).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 5]).unwrap();
        let history = &runtime.compensation.as_ref().unwrap().delays;
        let wrapped = history.stamp_at_delay(0).unwrap();
        assert_eq!((wrapped.arrangement_frame, wrapped.loop_pass), (start, 1));
        assert_eq!(
            (wrapped.meter, wrapped.cycle_beats_raw),
            (Some(Meter::COMMON_TIME), cycle)
        );
        let before_wrap = history.stamp_at_delay(2).unwrap();
        assert_eq!(
            (before_wrap.arrangement_frame, before_wrap.loop_pass),
            (24_000, 0)
        );
        assert_eq!(
            (before_wrap.meter, before_wrap.cycle_beats_raw),
            (Some(seven_eight), cycle)
        );
        runtime.apply(SessionPlaybackCommand::Seek(end)).unwrap();
        runtime.render(&mut [[0.0; 2]; 1]).unwrap();
        let after_loop = runtime
            .compensation
            .as_ref()
            .unwrap()
            .delays
            .stamp_at_delay(0)
            .unwrap();
        assert_eq!(
            (after_loop.arrangement_frame, after_loop.cycle_beats_raw),
            (end, None)
        );
    }

    struct RefuseTrack(TrackId);

    impl CaptureSink for RefuseTrack {
        fn selects(&self, id: OutputCaptureId) -> bool {
            id == OutputCaptureId::Track(self.0)
        }

        fn add(
            &mut self,
            id: OutputCaptureId,
            offset: usize,
            _: &[Frame],
        ) -> Result<(), SessionPlaybackError> {
            if self.selects(id) && offset == 13 {
                Err(SessionPlaybackError::DeviceRuntime)
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn resident_alignment_matches_scalar_automation_capture_seek_and_rollback() {
        use hex_lattice_contract::{
            DeviceAutomationTarget, DeviceTypeId, HostGain, NativeDeviceMetadata, ParameterValue,
        };
        use hex_project::{
            AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, GainInterpolation,
            GainPoint,
        };

        let mut f = fixture(&[&[], &[], &[]], 333);
        for audio in f.media.values_mut() {
            *audio = crate::Audio::new(48_000, vec![[0.125, -0.375]; 333]).unwrap();
        }
        f.editor
            .execute(hex_project::Command::SetLoopRegion {
                start: 49,
                end: 313,
                enabled: true,
            })
            .unwrap();
        let wet = f.tracks[1];
        for kind in ["hex.rattle", "hex.utility"] {
            f.editor
                .execute(hex_project::Command::InsertDevice {
                    track: wet,
                    before: None,
                    state: crate::default_native_device_state(DeviceTypeId::from_static(kind))
                        .unwrap(),
                })
                .unwrap();
        }
        // A long independent path must retain its scalar processing without
        // forcing the short-arrival native track onto that path.
        let limiter = &hex_devices::limiter::DEFINITION;
        let limiter_state = limiter
            .patch_parameter(
                &limiter.default_state().unwrap(),
                hex_devices::limiter::parameter::LOOKAHEAD,
                ParameterValue::Choice(2),
            )
            .unwrap();
        f.editor
            .execute(hex_project::Command::InsertDevice {
                track: f.tracks[2],
                before: None,
                state: limiter_state,
            })
            .unwrap();
        let second_stage = f.editor.project().track(wet).unwrap().devices()[1].id();
        let step = f.editor.project().tempo_map().frame_to_beat(170).unwrap();
        f.editor
            .execute(hex_project::Command::ReplaceDeviceAutomationTarget {
                device: second_stage,
                target: DeviceAutomationTarget::HostOutputGain,
                points: [
                    (BeatTime::ZERO, HostGain::ZERO),
                    (step, HostGain::from_millidb(-3000).unwrap()),
                ]
                .into_iter()
                .map(|(beat, gain)| AutomationPoint {
                    beat,
                    value: AutomationValue::new(gain.normalized()).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                })
                .collect(),
            })
            .unwrap();
        f.editor
            .execute(hex_project::Command::ReplaceTrackGainEnvelope {
                track: wet,
                points: [(BeatTime::ZERO, 0.3), (step, 1.1)]
                    .into_iter()
                    .map(|(beat, gain)| GainPoint {
                        incoming_gain: None,
                        beat,
                        gain,
                        outgoing: GainInterpolation::Linear,
                    })
                    .collect(),
            })
            .unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare(
                f.editor.project(),
                &f.media,
                Arc::new(crate::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        assert_eq!(revision.arrangement().main_latency_frames(), 288);
        let mut batched = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut scalar = SessionPlayback::new(revision).unwrap();
        scalar.resident_source_scratch.clear();
        for runtime in [&mut batched, &mut scalar] {
            runtime.track_faders[0].retarget([0.7, 0.2], 48_000);
            runtime.set_device_metering(true);
            runtime.apply(SessionPlaybackCommand::Play).unwrap();
        }
        let mut warmup = [[0.0; 2]; 32];
        let mut expected_warmup = warmup;
        batched.render(&mut warmup).unwrap();
        scalar.render(&mut expected_warmup).unwrap();
        assert_eq!(warmup, expected_warmup);
        assert_eq!(
            batched
                .prepare_native_origin_window(32, 32, 0, &mut [None; 4])
                .unwrap(),
            Some(32)
        );
        let mut output = [[9.0; 2]; 512];
        assert_eq!(
            batched.render_internal::<dyn MediaPages>(
                &mut output,
                None,
                None,
                Some(&mut RefuseTrack(wet)),
                None,
            ),
            Err(SessionPlaybackError::DeviceRuntime)
        );
        assert_eq!(output, [[0.0; 2]; 512]);
        let render = |runtime: &mut SessionPlayback, len| {
            let mut out = vec![[0.0; 2]; len];
            let mut track = out.clone();
            let selection = [wet.value()];
            let mut observer = |signal, offset, frames: &[Frame]| {
                if signal == crate::ProjectSignal::Track(wet.value()) {
                    track[offset..offset + frames.len()].copy_from_slice(frames);
                }
            };
            let mut capture = crate::ProjectOutputTap::new(&selection, &mut observer).unwrap();
            runtime
                .render_internal::<dyn MediaPages>(&mut out, None, None, Some(&mut capture), None)
                .unwrap();
            (out, track)
        };
        for size in [1, 7, 256, 512] {
            assert_eq!(render(&mut batched, size), render(&mut scalar, size));
        }
        assert_eq!(batched.track_faders[0], scalar.track_faders[0]);
        assert_eq!(batched.arrangement_frame, scalar.arrangement_frame);
        for runtime in [&mut batched, &mut scalar] {
            runtime.apply(SessionPlaybackCommand::Seek(61)).unwrap();
        }
        for size in [9, 128, 256] {
            assert_eq!(render(&mut batched, size), render(&mut scalar, size));
        }
    }
}
