//! Compensated track, Return and Main routing. Once the transport history is
//! warm, current processors and retained bus tails read their delayed clocks
//! from the same upcoming block before its stamps enter the history ring.
//! Pre-roll and sources requiring frame routing use the scalar path.
use super::*;
use crate::compensation::{OriginWindow, TransportStamp};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ResidentRoute {
    Scalar,
    Passthrough,
    Unity,
}

#[derive(Clone, Copy)]
pub(super) struct BatchedSources<'a> {
    pub(super) index: usize,
    pub(super) audio: &'a [bool; MAX_TRACKS],
    pub(super) instrument: &'a [bool; MAX_TRACKS],
    pub(super) stamps: Option<&'a [Option<TransportStamp>]>,
    pub(super) aligned: &'a [bool; MAX_TRACKS],
    pub(super) passthrough: &'a [ResidentRoute; MAX_TRACKS],
    pub(super) batched_sends: bool,
    /// Track sum in canonical Main mix order, including the incoming frame.
    pub(super) main: Option<Frame>,
}

pub(super) struct ResidentSourceWindow {
    pub(super) performance_frame: u64,
    pub(super) arrangement_frame: u64,
    pub(super) frames: usize,
    pub(super) mutable_loop_pass: Option<u64>,
    pub(super) interactive_onset_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
    pub(super) chase_at_start: bool,
}

pub(super) fn add_main_track(main: &mut [Frame], track: &[Frame]) {
    for (mixed, sample) in main.iter_mut().zip(track) {
        mixed[0] += sample[0];
        mixed[1] += sample[1];
    }
}

fn render_isolated_mutable_window<T: crate::prepared_tempo::MidiTempoBreaks>(
    context: MutableInstrumentChunkContext<'_, T>,
    index: usize,
    track: &crate::Track,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    let start = context.arrangement_frame;
    let end = start
        .checked_add(output.len() as u64)
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    context
        .performance_frame
        .checked_add(output.len() as u64)
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    let mut note_boundaries = [false; 256];
    let note_boundaries = &mut note_boundaries[..output.len()];
    note_boundaries[0] = context.chase_at_start
        || (context.arrangement_loop_pass > 0
            && context
                .loop_region
                .is_some_and(|region| start == region.start()));
    track
        .for_each_mutable_midi_span(start, end, |span| {
            if (start..end).contains(&span.start) {
                note_boundaries[(span.start - start) as usize] = true;
            }
        })
        .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
    let mut offset = 0;
    while offset < output.len() {
        let performance = context.performance_frame + offset as u64;
        let next_onset = note_boundaries[offset..]
            .iter()
            .position(|&due| due)
            .unwrap_or(output.len() - offset);
        let runtime = context.runtimes[index]
            .as_ref()
            .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?;
        let width = runtime.isolated_segment_width(performance, next_onset.max(1));
        let segment_end = offset + width;
        render_mutable_instrument_chunk(
            MutableInstrumentChunkContext {
                runtimes: &mut *context.runtimes,
                onsets: &mut *context.onsets,
                events: &mut *context.events,
                spans: &mut *context.spans,
                performance_frame: performance,
                arrangement_frame: start + offset as u64,
                chase_at_start: context.chase_at_start && offset == 0,
                ..context
            },
            index,
            &mut output[offset..segment_end],
            None,
        )?;
        offset = segment_end;
    }
    Ok(())
}

fn resident_return_track_taps(
    track: &crate::Track,
    mut frame: Frame,
    route: ResidentRoute,
    metering: bool,
) -> Frame {
    // Unity scratch already preserves both tap magnitudes. Other tracks
    // retain their raw scratch so each tap can observe its own gain stage.
    if route != ResidentRoute::Unity {
        frame[0] += 0.0;
        frame[1] += 0.0;
        track.apply_instrument_input_gain(std::slice::from_mut(&mut frame));
    }
    if metering {
        track.instrument_meter.observe_input(&[frame]);
    }
    if route != ResidentRoute::Unity {
        frame[0] += 0.0;
        frame[1] += 0.0;
        track.apply_instrument_output_gain(std::slice::from_mut(&mut frame));
    }
    if metering {
        track.instrument_meter.observe_output(&[frame]);
    }
    frame
}

impl SessionPlayback {
    /// Identify resident tracks whose local processing is a pure pass-through;
    /// raw samples stay in scratch so meter observations remain per frame.
    pub(super) fn classify_resident_return_tracks(
        &self,
        eligible: &[bool; MAX_TRACKS],
        passthrough: &mut [ResidentRoute; MAX_TRACKS],
    ) {
        passthrough.fill(ResidentRoute::Scalar);
        if !self.release_scratch.is_empty() {
            return;
        }
        let Some(compensation) = self.compensation.as_ref() else {
            return;
        };
        if !compensation.resident_return_graph_is_passthrough() {
            return;
        }
        for (index, (route, done)) in self.route_scratch.iter().zip(passthrough).enumerate() {
            let track = &self.revision.arrangement().tracks()[index];
            if !eligible[index]
                || !matches!(
                    route.owner,
                    RouteOwner::Arrangement | RouteOwner::SessionAudio { .. }
                )
                || self.revision.gain_recording_track() == Some(route.id)
                || track.has_gain_automation()
                || track.instrument_id.is_some()
                || track.instrument.is_some()
                || track.foreign_instrument.is_some()
                || track.midi_source_index.is_some()
                || track.instrument_position() != 0
                || self.revision.source_routing().is_shared(index)
                || self.revision.source_routing().contributes(index)
                || self
                    .mutable_instruments
                    .get(index)
                    .is_some_and(Option::is_some)
                || self.source_effects.has_chain(index)
                || !compensation.resident_return_track_is_passthrough(index)
            {
                continue;
            }
            *done = ResidentRoute::Passthrough;
        }
    }
    /// Batch only a stationary unity fader whose post-fader samples retain
    /// each tap's magnitude. Publishing both taps still happens per frame.
    pub(super) fn prepare_unity_faders(
        &mut self,
        frames: usize,
        passthrough: &mut [ResidentRoute; MAX_TRACKS],
        aligned: &[bool; MAX_TRACKS],
    ) {
        for index in 0..self.route_scratch.len() {
            if passthrough[index] != ResidentRoute::Passthrough || aligned[index] {
                continue;
            }
            let fader = self.track_fader_scratch[index];
            if fader.active() || fader.current() != [1.0; 2] {
                continue;
            }
            let track = &self.revision.arrangement().tracks()[index];
            if track.instrument_input_gain != 1.0
                || track.instrument_output_gain != 1.0
                || track.mix().with_channel_gain(fader.current())
                    != crate::PreparedTrackMix::identity()
            {
                continue;
            }
            let start = self.render_work.track(index);
            // Validate both taps before rewriting any sample. This protects
            // against unusual floating-point modes that change subnormals.
            let equivalent = |a: f32, b: f32| {
                if a.is_finite() && b.is_finite() {
                    a.abs().to_bits() == b.abs().to_bits()
                } else {
                    !a.is_finite() && !b.is_finite()
                }
            };
            if !self.resident_source_scratch[index][..frames]
                .iter()
                .flatten()
                .all(|sample| {
                    let first = *sample + 0.0;
                    let second = first + 0.0;
                    let mixed = second * 1.0;
                    equivalent(second, mixed) && equivalent(first * 1.0, mixed * 1.0)
                })
            {
                self.render_work.finish(start);
                continue;
            }
            for frame in &mut self.resident_source_scratch[index][..frames] {
                frame[0] += 0.0;
                frame[1] += 0.0;
                frame[0] += 0.0;
                frame[1] += 0.0;
                frame[0] *= 1.0;
                frame[1] *= 1.0;
            }
            self.render_work.finish(start);
            passthrough[index] = ResidentRoute::Unity;
        }
    }
    /// Prepare Return inputs in graph owner order. Only zero-delay sends from
    /// completely eligible tracks may advance ahead of the per-frame route.
    pub(super) fn prepare_resident_return_sends(
        &mut self,
        frames: usize,
        routes: &[ResidentRoute; MAX_TRACKS],
    ) -> bool {
        let Some(compensation) = self.compensation.as_ref() else {
            return false;
        };
        if !compensation.resident_return_graph_is_passthrough()
            || !compensation.resident_return_sends_are_independent()
            || frames > 256
            || self.route_scratch.len() > routes.len()
            || self.route_scratch.len() != compensation.graph.tracks.len()
            || self.resident_source_scratch.len() != self.route_scratch.len()
            || self.return_scratch.len() != compensation.graph.returns.len()
            || compensation.graph.owner_order.iter().any(|owner| {
                matches!(*owner, crate::signal_graph::Owner::Track(index) if index >= self.route_scratch.len())
            })
            || routes[..self.route_scratch.len()]
                .iter()
                .enumerate()
                .any(|(index, route)| *route == ResidentRoute::Scalar || self.track_fader_scratch[index].active())
        {
            return false;
        }
        for scratch in &mut self.return_scratch {
            scratch[..frames].fill([0.0; 2]);
        }
        for owner in &compensation.graph.owner_order {
            if let crate::signal_graph::Owner::Track(index) = *owner {
                let work_start = self.render_work.track(index);
                let samples = &self.resident_source_scratch[index][..frames];
                let track = &self.revision.arrangement().tracks()[index];
                let mix = track
                    .mix()
                    .with_channel_gain(self.track_fader_scratch[index].current());
                for send in &compensation.graph.tracks[index].sends {
                    for (output, input) in self.return_scratch[send.target][..frames]
                        .iter_mut()
                        .zip(samples)
                    {
                        let mut value = *input;
                        if routes[index] == ResidentRoute::Passthrough {
                            // Match the per-frame input/output taps below exactly;
                            // the original raw sample remains for meter publication.
                            value = resident_return_track_taps(track, value, routes[index], false);
                            value = mix.apply_frame(value);
                        }
                        output[0] += value[0] * send.gain;
                        output[1] += value[1] * send.gain;
                    }
                }
                self.render_work.finish(work_start);
            }
        }
        true
    }

    /// After independent sends are prepared, advance each track's dry Main
    /// edge in sample order. The wet Return processors still run per frame.
    /// Keep the undelayed source scratch for the original per-frame meters.
    pub(super) fn prepare_resident_return_main(
        &mut self,
        main: &mut [Frame],
        routes: &[ResidentRoute; MAX_TRACKS],
    ) -> Result<(), SessionPlaybackError> {
        let compensation = self
            .compensation
            .as_mut()
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        let mut aligned = [[0.0; 2]; 256];
        let aligned = &mut aligned[..main.len()];
        for (index, (samples, &route)) in
            self.resident_source_scratch.iter().zip(routes).enumerate()
        {
            let crate::signal_graph::TrackOutput::Main { edge } =
                compensation.graph.tracks[index].output
            else {
                return Err(SessionPlaybackError::DeviceRuntime);
            };
            let work_start = self.render_work.track(index);
            let samples = &samples[..aligned.len()];
            if route == ResidentRoute::Unity {
                aligned.copy_from_slice(samples);
            } else {
                let track = &self.revision.arrangement().tracks()[index];
                let mix = track
                    .mix()
                    .with_channel_gain(self.track_fader_scratch[index].current());
                for (output, &sample) in aligned.iter_mut().zip(samples) {
                    *output =
                        mix.apply_frame(resident_return_track_taps(track, sample, route, false));
                }
            }
            compensation
                .edge_block(edge, aligned)
                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
            add_main_track(main, aligned);
            self.render_work.finish(work_start);
        }
        Ok(())
    }

    /// Prepare resident audio and eligible instrument PCM. An instrument's
    /// window must have no local audio in the same scratch. The caller bounds
    /// windows at route and transport cuts and admits mutable processing only
    /// on an independent dry graph with no live inputs or observable capture.
    /// Page demand and downstream DSP still advance through the scalar route.
    pub(super) fn prepare_resident_sources(
        &mut self,
        window: ResidentSourceWindow,
        eligible: &mut [bool; MAX_TRACKS],
        instruments: &mut [bool; MAX_TRACKS],
        live_track: Option<TrackId>,
    ) -> Result<(), SessionPlaybackError> {
        let ResidentSourceWindow {
            performance_frame,
            arrangement_frame,
            frames,
            mutable_loop_pass,
            interactive_onset_floor,
            chase_at_start,
        } = window;
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
        instruments.fill(false);
        let batch_analytic = self.suppression_scratch.is_empty()
            && self.analytic_continuations.iter().all(Option::is_none);
        let revision = Arc::clone(&self.revision);
        let timing = timing_view_for(revision.session(), self.periodic_tempo(), self.anchor);
        let active_loop = mutable_loop_pass.and_then(|_| self.active_loop_region());
        for (index, (route, scratch)) in self
            .route_scratch
            .iter()
            .zip(self.resident_source_scratch.iter_mut())
            .enumerate()
        {
            let work_start = self.render_work.track(index);
            let output = &mut scratch[..frames];
            output.fill([0.0; 2]);
            if live_track == Some(route.id)
                || !self.browser_audition.voices.for_track(route.id).is_empty()
            {
                // The ordinary per-frame route consumes the live clip clock.
                self.render_work.finish(work_start);
                continue;
            }
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
                            interactive_onset_floor,
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
                    *instruments
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
                    if let Some(arrangement_loop_pass) = mutable_loop_pass
                        && batch_analytic
                        && track.clips.is_empty()
                        && track.receives_source_audio()
                        && track.midi_source_index.is_none()
                        && !self.revision.source_routing().is_shared(index)
                        && track
                            .instrument
                            .as_ref()
                            .is_some_and(|device| device.mutable && device.volumes.is_none())
                        && track.instrument_id.is_some_and(|device| {
                            self.revision
                                .session()
                                .instrument_automation_at(index, device)
                                .is_none()
                        })
                        && self.mutable_instruments[index]
                            .as_ref()
                            .is_some_and(|runtime| runtime.can_batch_isolated_window(frames as u64))
                    {
                        render_isolated_mutable_window(
                            MutableInstrumentChunkContext {
                                revision: &self.revision,
                                midi: crate::session_inventory::EffectiveSessionMidi::new(
                                    self.revision.session(),
                                    self.midi_admission.as_ref(),
                                ),
                                sources: None,
                                routes: &self.route_scratch,
                                runtimes: &mut self.mutable_instruments,
                                onsets: &mut self.mutable_onsets,
                                suppressions: &self.suppression_scratch,
                                events: &mut self.mutable_events,
                                spans: &mut self.spans,
                                performance_frame,
                                arrangement_frame,
                                arrangement_loop_pass,
                                transient: None,
                                chase_at_start,
                                interactive_onset_floor,
                                pending_shared_chase: None,
                                loop_region: active_loop,
                                timing: &timing,
                            },
                            index,
                            track,
                            output,
                        )?;
                        instruments[index] = true;
                    } else if batch_analytic
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
                                (arrangement_frame, performance_frame),
                                output,
                                &self.suppression_scratch,
                                &self.analytic_continuations,
                                Some(self.revision.session().timing()),
                                interactive_onset_floor,
                            )
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        *instruments
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
                            interactive_onset_floor,
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
                    *instruments
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
        let session_alignment_blocked = !self.release_scratch.is_empty()
            || self.tail_output_scratch.iter().any(|active| *active)
            || self
                .native_parameter_state
                .as_deref()
                .is_some_and(|state| state.processing.is_some());
        for (index, ((route, scratch), aligned)) in self
            .route_scratch
            .iter()
            .zip(&mut self.resident_source_scratch)
            .zip(aligned)
            .enumerate()
        {
            if !eligible[index]
                || !matches!(
                    route.owner,
                    RouteOwner::Arrangement | RouteOwner::SessionAudio { .. }
                )
                || self.revision.gain_recording_track() == Some(route.id)
            {
                continue;
            }
            let track = &self.revision.arrangement().tracks()[index];
            if matches!(route.owner, RouteOwner::SessionAudio { .. })
                && (track.instrument.is_some()
                    || track.foreign_instrument.is_some()
                    || track.midi_source_index.is_some()
                    || self.revision.source_routing().is_shared(index)
                    || self.revision.source_routing().contributes(index)
                    || self
                        .mutable_instruments
                        .get(index)
                        .is_some_and(Option::is_some)
                    || self.source_effects.has_chain(index)
                    // Device taps must publish as each frame is rendered.
                    || (self.device_metering && !self.device_chains[index].is_empty())
                    || session_alignment_blocked)
            {
                continue;
            }
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
        let revision = Arc::clone(&self.revision);
        let timing = timing_view_for(revision.session(), self.periodic_tempo(), self.anchor);
        let beat = timing
            .frame_to_beat(performance_frame)
            .map_err(|_| SessionPlaybackError::TempoConversion)?;
        let session = self.revision.session();
        let arrangement_beat = session.timing().frame_to_beat(arrangement_frame).ok();
        Ok(TransportStamp {
            authored_clock_epoch: Some(self.authored_clock_epoch),
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

    pub(super) fn fill_compensated_stamps(
        &self,
        performance_frame: u64,
        arrangement_frame: u64,
        loop_pass: u64,
        out: &mut [Option<TransportStamp>],
    ) -> Result<(), SessionPlaybackError> {
        let count = out.len();
        if count == 0 {
            return Ok(());
        }
        performance_frame
            .checked_add((count - 1) as u64)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        arrangement_frame
            .checked_add((count - 1) as u64)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let session = self.revision.session();
        let timing = timing_view_for(session, self.periodic_tempo(), self.anchor);
        let mut perf_beats = [hex_project::BeatTime::ZERO; 256];
        let mut arr_beats = [hex_project::BeatTime::ZERO; 256];
        let cycle_first = self
            .periodic_tempo_at(arrangement_frame)
            .map(|cycle| cycle.beat_range_raw());
        let cycle_last = self
            .periodic_tempo_at(arrangement_frame + (count - 1) as u64)
            .map(|cycle| cycle.beat_range_raw());
        if count <= 256
            && cycle_first == cycle_last
            && let Some(micro_bpm) =
                timing.fill_constant_range_beats(performance_frame, &mut perf_beats[..count])
            && session
                .timing()
                .fill_constant_segment_beats(arrangement_frame, &mut arr_beats[..count])
                .is_some()
        {
            let meter_map = session.meter_map();
            let constant_meter =
                (meter_map.points().len() <= 1).then(|| meter_map.meter_at(arr_beats[0]));
            let epoch = Some(self.authored_clock_epoch);
            let playing = self.playing;
            for (i, slot) in out.iter_mut().enumerate() {
                let arr_beat = arr_beats[i];
                let meter = constant_meter.unwrap_or_else(|| meter_map.meter_at(arr_beat));
                *slot = Some(TransportStamp {
                    authored_clock_epoch: epoch,
                    tempo_micro_bpm: Some(micro_bpm),
                    performance_frame: performance_frame + i as u64,
                    transport_is_running: playing,
                    arrangement_frame: arrangement_frame + i as u64,
                    loop_pass,
                    performance_beat_raw: perf_beats[i].raw(),
                    arrangement_beat_raw: Some(arr_beat.raw()),
                    meter: Some(meter),
                    cycle_beats_raw: cycle_first,
                });
            }
            return Ok(());
        }
        for (offset, slot) in out.iter_mut().enumerate() {
            let perf = performance_frame + offset as u64;
            let arr = arrangement_frame + offset as u64;
            *slot = Some(self.compensated_stamp(perf, arr, loop_pass)?);
        }
        Ok(())
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
        external: Option<&dyn hex_lattice::NativeExternalAutomationSet>,
        resident_source: Option<BatchedSources<'_>>,
        live_audio: Option<LiveTrackAudio<'_>>,
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
        let mut detached_sends = if grouped && self.has_tail_route_sends {
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
                DetachedTailRouting {
                    routed: Some(detached),
                    routed_sends: detached_sends.as_mut().map(|sends| sends.as_mut_slice()),
                },
                foreign,
            )?;
        }
        let region = self.active_loop_region();
        let revision = Arc::clone(&self.revision);
        let timing = timing_view_for(revision.session(), self.periodic_tempo(), self.anchor);
        let batched_sends = resident_source.is_some_and(|batch| batch.batched_sends);
        let return_offset = resident_source
            .filter(|batch| batch.batched_sends)
            .map_or(0, |batch| batch.index);
        if !batched_sends {
            for scratch in &mut self.return_scratch {
                scratch[0] = [0.0; 2];
            }
        }
        let compensation = self
            .compensation
            .as_mut()
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        let batched_main = resident_source.and_then(|batch| batch.main);
        if batched_main.is_none() {
            compensation.inputs.fill([0.0; 2]);
            compensation.main.fill([0.0; 2]);
        }
        let owner_count = if batched_main.is_none() || batched_sends {
            compensation.graph.owner_order.len()
        } else {
            0
        };
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
                    if let Some(batch) = resident_source.filter(|batch| batch.main.is_some()) {
                        if self.device_metering {
                            let work_start = self.render_work.track(index);
                            resident_return_track_taps(
                                track,
                                self.resident_source_scratch[index][batch.index],
                                batch.passthrough[index],
                                true,
                            );
                            self.render_work.finish(work_start);
                        }
                        continue;
                    }
                    let route = self.route_scratch[index];
                    // The aligned owner was already timed with its batch.
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
                    let work_start = self.render_work.track(index);
                    let mixed = if let Some(batch) = resident_source
                        .filter(|batch| batch.passthrough[index] != ResidentRoute::Scalar)
                    {
                        let route = batch.passthrough[index];
                        let frame = resident_return_track_taps(
                            track,
                            self.resident_source_scratch[index][batch.index],
                            route,
                            self.device_metering,
                        );
                        if route == ResidentRoute::Unity {
                            frame
                        } else {
                            // The current stamp was just pushed. Admission proves
                            // zero fader arrival, no gain automation and no group
                            // mute, so its mix is exactly the static track mix.
                            track
                                .mix()
                                .with_channel_gain(self.track_fader_scratch[index].next())
                                .apply_frame(frame)
                        }
                    } else {
                        let mut audio = [[0.0; 2]];
                        let mut generator = [[0.0; 2]];
                        if self.playing {
                            let instrument_index = resident_source
                                .filter(|batch| batch.instrument[index])
                                .map(|batch| batch.index);
                            if instrument_index.is_some() {
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
                                        interactive_onset_floor: (loop_pass == 0)
                                            .then_some(self.interactive_onset_floor)
                                            .flatten(),
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
                                    SourceStage::audio(
                                        self.browser_audition.voices.for_track(route.id),
                                    ),
                                )?;
                            }
                            // An ordinary arrangement audio track without a MIDI
                            // source cannot contribute instrument samples. Shared
                            // sources and session routes must still use the normal
                            // scheduler, even when this track has no local instrument.
                            if let Some(sample_index) = instrument_index {
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
                                        interactive_onset_floor: (loop_pass == 0)
                                            .then_some(self.interactive_onset_floor)
                                            .flatten(),
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
                                let authored = self.authored_source_voices(index)?;
                                source_live::render_source_live_input(
                                    &self.revision,
                                    self.computer_keyboard.as_mut().map(Box::as_mut),
                                    self.mutable_instruments[index].as_mut(),
                                    &mut self.mutable_onsets,
                                    &mut self.mutable_events,
                                    authored,
                                    performance_frame,
                                    &mut generator,
                                    &timing,
                                    true,
                                    None,
                                    None,
                                )?;
                            }
                            if instrument_index.is_none()
                                && self
                                    .mutable_instruments
                                    .get(index)
                                    .and_then(Option::as_ref)
                                    .is_some()
                            {
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
                                        transient: None,
                                        chase_at_start: self.interactive_start_frame
                                            == Some(performance_frame),
                                        interactive_onset_floor: (loop_pass == 0)
                                            .then_some(self.interactive_onset_floor)
                                            .flatten(),
                                        pending_shared_chase: self.pending_shared_chase,
                                        loop_region: if self.linear_arrangement_export {
                                            None
                                        } else {
                                            region.filter(|region| region.enabled())
                                        },
                                        timing: &timing,
                                    },
                                    index,
                                    &mut generator,
                                    self.native_parameter_state
                                        .as_deref_mut()
                                        .and_then(|state| state.processing.as_mut()),
                                )?;
                            }
                        }
                        for release in &self.release_scratch {
                            if release.location.track_index() == index
                                && release.location.tail_group().is_none()
                            {
                                render_release_raw(
                                    release,
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
                                let parameter_context =
                                    instrument_automation::SourceAutomation::new(
                                        &self.revision,
                                        index,
                                        instrument_automation::Origin {
                                            arrangement: Some(arrangement_frame),
                                            performance: performance_frame,
                                            pass: loop_pass,
                                            running: false,
                                        },
                                        generator.len(),
                                    )?;
                                let parameter_block = parameter_context.block();
                                source_live::render_source_live_input(
                                    &self.revision,
                                    self.computer_keyboard.as_mut().map(Box::as_mut),
                                    self.mutable_instruments[index].as_mut(),
                                    &mut self.mutable_onsets,
                                    &mut self.mutable_events,
                                    0,
                                    performance_frame,
                                    &mut generator,
                                    &timing,
                                    false,
                                    parameter_block.as_ref().map(|values| {
                                        values as &dyn hex_lattice::NativeParameterAutomation
                                    }),
                                    None,
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
                                let parameter_context =
                                    instrument_automation::SourceAutomation::new(
                                        &self.revision,
                                        index,
                                        instrument_automation::Origin {
                                            arrangement: Some(arrangement_frame),
                                            performance: performance_frame,
                                            pass: loop_pass,
                                            running: false,
                                        },
                                        generator.len(),
                                    )?;
                                let parameter_block = parameter_context.block();
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
                                        parameter_block.as_ref().map(|values| {
                                            values as &dyn hex_lattice::NativeParameterAutomation
                                        }),
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
                            let clock = crate::signal_graph::OriginClock(
                                compensation.delays.stamp_at_delay_ref(0),
                            );
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
                                    interactive_onset_floor: (loop_pass == 0)
                                        .then_some(self.interactive_onset_floor)
                                        .flatten(),
                                },
                                foreign_arrangement::SessionRoute {
                                    route: effective_midi_route(
                                        &self.revision,
                                        &self.route_scratch,
                                        index,
                                    )?,
                                    midi: crate::session_inventory::EffectiveSessionMidi::new(
                                        self.revision.session(),
                                        self.midi_admission.as_ref(),
                                    ),
                                    timing: &timing,
                                    spans: &mut self.spans,
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
                        if self.source_effects.has_chain(index) {
                            let clock = crate::signal_graph::OriginClock(
                                compensation.delays.stamp_at_delay_ref(0),
                            );
                            self.source_effects
                                .process_with_live_input(
                                    index,
                                    &mut generator,
                                    &clock,
                                    frame_offset,
                                    self.native_parameter_state
                                        .as_deref_mut()
                                        .and_then(|state| state.processing.as_mut()),
                                )
                                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        }
                        if let Some(input) = live_audio.filter(|input| {
                            input.track == route.id
                                && Self::track_monitors_live_audio_for(
                                    track,
                                    route.owner,
                                    arrangement_frame,
                                    self.playing,
                                )
                        }) {
                            let source = input.frames[frame_offset];
                            audio[0][0] += source[0];
                            audio[0][1] += source[1];
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
                        let recording_track =
                            self.revision.gain_recording_track() == Some(route.id);
                        let override_gain =
                            self.gain_recording_override.filter(|_| recording_track);
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
                        if let Some(mix) = mix {
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
                        }
                    };
                    let compensation = self
                        .compensation
                        .as_mut()
                        .ok_or(SessionPlaybackError::DeviceRuntime)?;
                    // Only tails with an identical-graph handoff proof may
                    // re-enter current authored sends and feeds.
                    let routed = if let Some(detached) = detached.as_ref() {
                        [mixed[0] + detached[index][0], mixed[1] + detached[index][1]]
                    } else {
                        mixed
                    };
                    let send_mixed = if let Some(detached_sends) = detached_sends.as_ref() {
                        [
                            mixed[0] + detached_sends[index][0],
                            mixed[1] + detached_sends[index][1],
                        ]
                    } else {
                        mixed
                    };
                    let main = compensation
                        .route_track_output(index, routed)
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    if let crate::signal_graph::TrackOutput::Return { target, .. } =
                        compensation.graph.tracks[index].output
                    {
                        self.return_scratch[target][return_offset][0] += main[0];
                        self.return_scratch[target][return_offset][1] += main[1];
                    }
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
                            .edge(edge, [send_mixed[0] * gain, send_mixed[1] * gain])
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        compensation.inputs[target][0] += sent[0];
                        compensation.inputs[target][1] += sent[1];
                    }
                    if !batched_sends {
                        let send_frame = compensation.fader_content_frame(index);
                        let compiled_track = &self.revision.arrangement().tracks()[index];
                        for send_index in 0..compensation.graph.tracks[index].sends.len() {
                            let send = &compensation.graph.tracks[index].sends[send_index];
                            let gain = if let Some(frame) = send_frame
                                && compiled_track.has_send_automation()
                            {
                                compiled_track.send_gain_at(send_index, frame)
                            } else {
                                send.gain
                            };
                            let (target, edge) = (send.target, send.edge);
                            let sent = compensation
                                .edge(edge, [send_mixed[0] * gain, send_mixed[1] * gain])
                                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                            self.return_scratch[target][return_offset][0] += sent[0];
                            self.return_scratch[target][return_offset][1] += sent[1];
                        }
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
                    let mut frame = self.return_scratch[index][return_offset];
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
                    if let Some(fade) = self
                        .return_fade_scratch
                        .as_mut()
                        .filter(|fade| fade.id == bus.id)
                    {
                        let gain = fade.fader.next();
                        frame[0] *= gain[0];
                        frame[1] *= gain[1];
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
                        self.return_scratch[target][return_offset][0] += sent[0];
                        self.return_scratch[target][return_offset][1] += sent[1];
                    }
                    let main = compensation
                        .edge(compensation.graph.returns[index].main, frame)
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    self.return_scratch[index][return_offset] = main;
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
        if let Some(main) = batched_main {
            output[0] = main;
        } else {
            for main in &self
                .compensation
                .as_ref()
                .ok_or(SessionPlaybackError::DeviceRuntime)?
                .main
            {
                output[0][0] += main[0];
                output[0][1] += main[1];
            }
        }
        for scratch in &self.return_scratch {
            output[0][0] += scratch[return_offset][0];
            output[0][1] += scratch[return_offset][1];
        }
        // Grouped tails were routed through their track output above. Flat tails
        // enter Main directly after the handoff has proved their alignment.
        if !grouped {
            self.render_detached_tails(
                performance_frame,
                output,
                frame_offset,
                capture,
                DetachedTailRouting::UNROUTED,
                foreign,
            )?;
        }
        if self.playing {
            self.return_effect_tails
                .render_chunk(output, frame_offset, capture, foreign)?;
        }
        Ok(())
    }

    fn has_grouped_detached_tails(&self) -> bool {
        self.compensation
            .as_ref()
            .is_some_and(|runtime| runtime.graph.has_group_outputs())
            && (self.detached_tail_group_limit != 0
                || self
                    .release_scratch
                    .last()
                    .is_some_and(|entry| entry.location.track_index() >= self.route_scratch.len()))
    }

    /// Steady-state block execution for compensated graphs (including
    /// sidechains, return buses, native effects, VST3 effects/instruments,
    /// Samplers, automation, metering and observers).
    ///
    /// Requires the transport stamp ring to hold its retained horizon of
    /// consecutive running frames so every stage's `OriginWindow` is populated
    /// and running across the entire chunk `[0..count]`. Startup pre-roll
    /// retains the per-frame kernel. Track-owned MIDI releases
    /// are summed into their generators before source effects.
    pub(super) fn can_block_compensated_route(&self, count: usize) -> bool {
        if count <= 1
            || count > 256
            || !self.playing
            || self.resident_source_scratch.len() != self.route_scratch.len()
            || self.resident_source_scratch.is_empty()
            || !self.current_sampler_keyboards.is_empty()
            || self.current_sampler_console
            || !self.browser_audition.is_idle()
            || !self.suppression_scratch.is_empty()
            || self.analytic_continuations.iter().any(Option::is_some)
            || self.revision.gain_recording_track().is_some()
            || self
                .route_scratch
                .iter()
                .any(|route| route.audio_release.is_some())
        {
            return false;
        }
        let Some(compensation) = self.compensation.as_ref() else {
            return false;
        };
        if !compensation.graph.supports_block_compensated_routing()
            || self.return_scratch.len() != compensation.graph.returns.len()
        {
            return false;
        }
        compensation.delays.running_stamp_streak() >= compensation.horizon()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_compensated_route_block<P: MediaPages + ?Sized>(
        &mut self,
        performance_frame: u64,
        arrangement_frame: u64,
        loop_pass: u64,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
        frame_offset: usize,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        capture: &mut Option<&mut dyn CaptureSink>,
        wave: &mut Option<&mut dyn crate::wave_executor::WaveExecutor>,
        inputs: RenderBlockInputs<'_>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        tail_scratch: &mut [[Frame; 256]],
    ) -> Result<(), SessionPlaybackError> {
        let count = output.len();
        if count == 0 || count > 256 {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        let mut stamps = [None; 256];
        self.fill_compensated_stamps(
            performance_frame,
            arrangement_frame,
            loop_pass,
            &mut stamps[..count],
        )?;
        let stamps = &stamps[..count];
        for scratch in &mut self.return_scratch {
            scratch[..count].fill([0.0; 2]);
        }
        let (detached, detached_sends) = if self.has_grouped_detached_tails() {
            let cells = self.route_scratch.len() * count;
            let planes = tail_scratch
                .as_flattened_mut()
                .get_mut(..2 * cells)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let (output_plane, send_plane) = planes.split_at_mut(cells);
            output_plane.fill([0.0; 2]);
            let mut sends = self.has_tail_route_sends.then_some(send_plane);
            if let Some(plane) = sends.as_deref_mut() {
                plane.fill([0.0; 2]);
            }
            self.render_detached_tails(
                performance_frame,
                output,
                frame_offset,
                capture,
                DetachedTailRouting {
                    routed: Some(output_plane),
                    routed_sends: sends.as_deref_mut(),
                },
                foreign,
            )?;
            (Some(output_plane), sends)
        } else {
            (None, None)
        };
        let region = self.active_loop_region();
        let loop_fade = self
            .active_loop_region_at(arrangement_frame)
            .and_then(|region| {
                crate::loop_seam::LoopEndFade::new(
                    region,
                    self.revision.arrangement().sample_rate(),
                )
            });
        let revision = Arc::clone(&self.revision);
        let timing = timing_view_for(revision.session(), self.periodic_tempo(), self.anchor);
        let wave_count = self
            .compensation
            .as_ref()
            .ok_or(SessionPlaybackError::DeviceRuntime)?
            .graph
            .owner_waves()
            .len();
        let mut has_group_input = [false; MAX_TRACKS];
        if let Some(compensation) = self.compensation.as_ref()
            && compensation.graph.has_group_outputs()
        {
            for track in &compensation.graph.tracks {
                if let crate::signal_graph::TrackOutput::Group { target, .. } = track.output
                    && !has_group_input[target]
                {
                    has_group_input[target] = true;
                    self.resident_source_scratch[target][..count].fill([0.0; 2]);
                }
            }
        }
        let mut audio_buf = [[0.0; 2]; 256];
        let mut generator_buf = [[0.0; 2]; 256];
        let mut key_buf = [[0.0; 2]; 256];
        let mut send_buf = [[0.0; 2]; 256];

        let source_input_receiver = self.source_input_receiver();
        let parallel_wave_allowed = self
            .native_parameter_state
            .as_deref()
            .is_none_or(|state| state.processing.is_none())
            && wave.as_deref().is_some_and(|exec| exec.worker_count() > 0);
        for wave_index in 0..wave_count {
            let owner_wave = self
                .compensation
                .as_ref()
                .ok_or(SessionPlaybackError::DeviceRuntime)?
                .graph
                .owner_waves()[wave_index];
            let wave_len = owner_wave.end - owner_wave.start;
            let mut staged_slots = [None; crate::wave_executor::MAX_WAVE_TASKS];
            let mut staged_owners = [None; crate::wave_executor::MAX_WAVE_TASKS];
            let mut used_parallel_wave = false;
            if parallel_wave_allowed
                && (crate::wave_executor::PARALLEL_WAVE_MIN_TASKS
                    ..=crate::wave_executor::MAX_WAVE_TASKS)
                    .contains(&wave_len)
            {
                let mut eligible = [false; crate::wave_executor::MAX_WAVE_TASKS];
                let mut eligible_count = 0usize;
                let mut max_wave_delay = 0usize;
                {
                    let compensation = self
                        .compensation
                        .as_ref()
                        .ok_or(SessionPlaybackError::DeviceRuntime)?;
                    let max_delay_u32 = crate::wave_executor::WAVE_CLOCK_HISTORY_FRAMES as u32;
                    for (local, is_eligible) in eligible.iter_mut().enumerate().take(wave_len) {
                        let owner = compensation.graph.owner_order[owner_wave.start + local];
                        match owner {
                            crate::signal_graph::Owner::Track(index) => {
                                let track = &self.revision.arrangement().tracks()[index];
                                let route = self.route_scratch[index];
                                let observed = observer.as_deref().is_some_and(|obs| {
                                    obs.observes_owner(crate::ProcessorChainOwner::Track(
                                        route.id.value(),
                                    ))
                                });
                                let keyboard_track =
                                    self.computer_keyboard.as_ref().is_some_and(|keyboard| {
                                        !keyboard.is_source_input() && keyboard.track() == route.id
                                    });
                                let devices = &compensation.graph.tracks[index].devices;
                                if !observed
                                    && track.instrument_position() == 0
                                    && !keyboard_track
                                    && source_input_receiver != Some(index)
                                    && track.foreign_instrument.is_none()
                                    && !self.source_effects.has_chain(index)
                                    && !devices.is_empty()
                                    && devices.len() <= crate::wave_executor::MAX_WAVE_STAGES
                                    && self.device_chains[index]
                                        .foreign_bindings()
                                        .next()
                                        .is_none()
                                {
                                    let mut ok = true;
                                    let mut track_max_delay = 0usize;
                                    for &node in devices {
                                        let planned = &compensation.graph.plan.nodes()[node];
                                        if compensation.graph.stage_has_key(node)
                                            || planned.input_arrival_frames > max_delay_u32
                                            || planned.output_arrival_frames > max_delay_u32
                                        {
                                            ok = false;
                                            break;
                                        }
                                        track_max_delay = track_max_delay
                                            .max(planned.input_arrival_frames as usize)
                                            .max(planned.output_arrival_frames as usize);
                                    }
                                    if ok {
                                        *is_eligible = true;
                                        eligible_count += 1;
                                        max_wave_delay = max_wave_delay.max(track_max_delay);
                                    }
                                }
                            }
                            crate::signal_graph::Owner::Return(index) => {
                                let bus_id =
                                    self.revision.arrangement().return_buses()[index].id.value();
                                let observed = observer.as_deref().is_some_and(|obs| {
                                    obs.observes_owner(crate::ProcessorChainOwner::Return(bus_id))
                                });
                                let devices = &compensation.graph.returns[index].devices;
                                if !observed
                                    && !devices.is_empty()
                                    && devices.len() <= crate::wave_executor::MAX_WAVE_STAGES
                                    && self.return_chains[index]
                                        .foreign_bindings()
                                        .next()
                                        .is_none()
                                {
                                    let mut ok = true;
                                    let mut ret_max_delay = 0usize;
                                    for &node in devices {
                                        let planned = &compensation.graph.plan.nodes()[node];
                                        if compensation.graph.stage_has_key(node)
                                            || planned.input_arrival_frames > max_delay_u32
                                            || planned.output_arrival_frames > max_delay_u32
                                        {
                                            ok = false;
                                            break;
                                        }
                                        ret_max_delay = ret_max_delay
                                            .max(planned.input_arrival_frames as usize)
                                            .max(planned.output_arrival_frames as usize);
                                    }
                                    if ok {
                                        *is_eligible = true;
                                        eligible_count += 1;
                                        max_wave_delay = max_wave_delay.max(ret_max_delay);
                                    }
                                }
                            }
                        }
                    }
                }
                if eligible_count >= crate::wave_executor::PARALLEL_WAVE_MIN_TASKS
                    && let Some(exec) = wave.as_deref_mut()
                    && let Some(clock_window) = exec.prepare_clock()
                {
                    let Some(compensation) = self.compensation.as_ref() else {
                        exec.finish_wave();
                        return Err(SessionPlaybackError::DeviceRuntime);
                    };
                    clock_window.fill_from_compensated(
                        &compensation.delays,
                        stamps,
                        max_wave_delay,
                    );
                    let mut prep_err: Option<SessionPlaybackError> = None;
                    for local in 0..wave_len {
                        if !eligible[local] {
                            continue;
                        }
                        let Some(owner) = self
                            .compensation
                            .as_ref()
                            .map(|c| c.graph.owner_order[owner_wave.start + local])
                        else {
                            prep_err = Some(SessionPlaybackError::DeviceRuntime);
                            break;
                        };
                        match owner {
                            crate::signal_graph::Owner::Track(index) => {
                                let work_start = self.render_work.track(index);
                                let route = self.route_scratch[index];
                                let audio = &mut audio_buf[..count];
                                let generator = &mut generator_buf[..count];
                                audio.fill([0.0; 2]);
                                let (has_instrument, has_midi_source, device_count) = {
                                    let track = &self.revision.arrangement().tracks()[index];
                                    let Some(compensation) = self.compensation.as_ref() else {
                                        self.render_work.finish(work_start);
                                        prep_err = Some(SessionPlaybackError::DeviceRuntime);
                                        break;
                                    };
                                    (
                                        track.instrument.is_some(),
                                        track.midi_source_index.is_some(),
                                        compensation.graph.tracks[index].devices.len(),
                                    )
                                };
                                let has_mutable_instrument = self
                                    .mutable_instruments
                                    .get(index)
                                    .and_then(Option::as_ref)
                                    .is_some();
                                let has_analytic_source = has_instrument
                                    || has_midi_source
                                    || self.revision.source_routing().is_shared(index)
                                    || !matches!(route.owner, RouteOwner::Arrangement);
                                let first_release = self
                                    .release_scratch
                                    .partition_point(|entry| entry.location.track_index() < index);
                                let release_end = first_release
                                    + self.release_scratch[first_release..].partition_point(
                                        |entry| entry.location.track_index() == index,
                                    );
                                let has_generator = first_release < release_end
                                    || has_analytic_source
                                    || has_mutable_instrument;

                                let prep_res: Result<(), SessionPlaybackError> = (|| {
                                    render_route_source_chunk(
                                        RouteSourceContext {
                                            revision: &self.revision,
                                            midi:
                                                crate::session_inventory::EffectiveSessionMidi::new(
                                                    self.revision.session(),
                                                    self.midi_admission.as_ref(),
                                                ),
                                            route,
                                            historical_floor: self.historical_floor_for(index),
                                            interactive_onset_floor: (loop_pass == 0)
                                                .then_some(self.interactive_onset_floor)
                                                .flatten(),
                                            suppressions: &self.suppression_scratch,
                                            continuations: &self.analytic_continuations,
                                            index,
                                            raw: true,
                                            loop_fade,
                                            timing: &timing,
                                            table: self.revision.session().sine_table(),
                                        },
                                        &mut self.spans,
                                        performance_frame,
                                        arrangement_frame,
                                        audio,
                                        pages.as_deref_mut(),
                                        SourceStage::audio(
                                            self.browser_audition.voices.for_track(route.id),
                                        ),
                                    )?;
                                    if let Some(input) =
                                        inputs.live_audio.filter(|input| input.track == route.id)
                                    {
                                        for (i, (dst, src)) in audio
                                            .iter_mut()
                                            .zip(&input.frames[frame_offset..frame_offset + count])
                                            .enumerate()
                                        {
                                            if self.track_monitors_live_audio(
                                                index,
                                                arrangement_frame + i as u64,
                                                true,
                                            ) {
                                                dst[0] += src[0];
                                                dst[1] += src[1];
                                            }
                                        }
                                    }
                                    if has_generator {
                                        generator.fill([0.0; 2]);
                                    }
                                    if has_analytic_source {
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
                                                interactive_onset_floor: (loop_pass == 0)
                                                    .then_some(self.interactive_onset_floor)
                                                    .flatten(),
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
                                            generator,
                                        )?;
                                    }
                                    if has_mutable_instrument {
                                        render_mutable_instrument_chunk(
                                            MutableInstrumentChunkContext {
                                                revision: &self.revision,
                                                midi: crate::session_inventory::EffectiveSessionMidi::new(
                                                    self.revision.session(),
                                                    self.midi_admission.as_ref(),
                                                ),
                                                sources: inputs.sources,
                                                routes: &self.route_scratch,
                                                runtimes: &mut self.mutable_instruments,
                                                onsets: &mut self.mutable_onsets,
                                                suppressions: &self.suppression_scratch,
                                                events: &mut self.mutable_events,
                                                spans: &mut self.spans,
                                                performance_frame,
                                                arrangement_frame,
                                                arrangement_loop_pass: loop_pass,
                                                transient: None,
                                                chase_at_start: self.interactive_start_frame
                                                    == Some(performance_frame),
                                                interactive_onset_floor: (loop_pass == 0)
                                                    .then_some(self.interactive_onset_floor)
                                                    .flatten(),
                                                pending_shared_chase: self.pending_shared_chase,
                                                loop_region: if self.linear_arrangement_export {
                                                    None
                                                } else {
                                                    region.filter(|region| region.enabled())
                                                },
                                                timing: &timing,
                                            },
                                            index,
                                            generator,
                                            None,
                                        )?;
                                    }
                                    for release in &self.release_scratch[first_release..release_end]
                                    {
                                        let remaining =
                                            release.end_frame().saturating_sub(performance_frame);
                                        let frames = count
                                            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
                                        render_release_raw(
                                            release,
                                            self.revision.arrangement().shared_contexts(),
                                            &self.retained_contexts,
                                            &self.analytic_continuations,
                                            performance_frame,
                                            true,
                                            &mut generator[..frames],
                                        )?;
                                    }
                                    let compensation = self
                                        .compensation
                                        .as_mut()
                                        .ok_or(SessionPlaybackError::DeviceRuntime)?;
                                    let track = &self.revision.arrangement().tracks()[index];
                                    let source_input_edge =
                                        compensation.graph.tracks[index].source_input;
                                    let audio_merge_edge =
                                        compensation.graph.tracks[index].audio_merge;
                                    let generator_merge_edge =
                                        compensation.graph.tracks[index].generator_merge;
                                    compensation.edge_block(source_input_edge, audio).map_err(
                                        |error| SessionPlaybackError::Arrangement(error.0),
                                    )?;
                                    if has_group_input[index] {
                                        add_main_track(
                                            audio,
                                            &self.resident_source_scratch[index][..count],
                                        );
                                    }
                                    for frame in audio.iter_mut() {
                                        frame[0] += 0.0;
                                        frame[1] += 0.0;
                                    }
                                    track.apply_instrument_input_gain(audio);
                                    if self.device_metering {
                                        track.instrument_meter.observe_input(audio);
                                    }
                                    compensation.edge_block(audio_merge_edge, audio).map_err(
                                        |error| SessionPlaybackError::Arrangement(error.0),
                                    )?;
                                    if let Some(edge) = generator_merge_edge {
                                        if !has_generator {
                                            generator.fill([0.0; 2]);
                                        }
                                        compensation.edge_block(edge, generator).map_err(
                                            |error| SessionPlaybackError::Arrangement(error.0),
                                        )?;
                                        for i in 0..count {
                                            audio[i][0] += generator[i][0];
                                            audio[i][1] += generator[i][1];
                                        }
                                    } else if has_generator {
                                        for i in 0..count {
                                            audio[i][0] += generator[i][0];
                                            audio[i][1] += generator[i][1];
                                        }
                                    }
                                    track.apply_instrument_output_gain(audio);
                                    if self.device_metering {
                                        track.instrument_meter.observe_output(audio);
                                    }
                                    Ok(())
                                })(
                                );
                                self.render_work.finish(work_start);
                                if let Err(err) = prep_res {
                                    prep_err = Some(err);
                                    break;
                                }
                                let mut arrivals =
                                    [crate::wave_executor::WaveStageArrival::default();
                                        crate::wave_executor::MAX_WAVE_STAGES];
                                {
                                    let Some(compensation) = self.compensation.as_ref() else {
                                        prep_err = Some(SessionPlaybackError::DeviceRuntime);
                                        break;
                                    };
                                    for (s, &node) in
                                        compensation.graph.tracks[index].devices.iter().enumerate()
                                    {
                                        let planned = &compensation.graph.plan.nodes()[node];
                                        arrivals[s] = crate::wave_executor::WaveStageArrival {
                                            input_delay: planned.input_arrival_frames,
                                            output_delay: planned.output_arrival_frames,
                                        };
                                    }
                                }
                                let moved = std::mem::replace(
                                    &mut self.device_chains[index],
                                    hex_lattice::NativeRuntimeChain::empty(),
                                );
                                match exec.push_task(
                                    index,
                                    moved,
                                    audio,
                                    frame_offset,
                                    0,
                                    device_count,
                                    true,
                                    &arrivals[..device_count],
                                ) {
                                    Ok(slot) => {
                                        staged_slots[local] = Some(slot);
                                        staged_owners[local] = Some(owner);
                                    }
                                    Err(returned) => {
                                        self.device_chains[index] = returned;
                                        prep_err = Some(SessionPlaybackError::DeviceRuntime);
                                        break;
                                    }
                                }
                            }
                            crate::signal_graph::Owner::Return(index) => {
                                let mut arrivals =
                                    [crate::wave_executor::WaveStageArrival::default();
                                        crate::wave_executor::MAX_WAVE_STAGES];
                                let device_count = {
                                    let Some(compensation) = self.compensation.as_ref() else {
                                        prep_err = Some(SessionPlaybackError::DeviceRuntime);
                                        break;
                                    };
                                    let devices = &compensation.graph.returns[index].devices;
                                    for (s, &node) in devices.iter().enumerate() {
                                        let planned = &compensation.graph.plan.nodes()[node];
                                        arrivals[s] = crate::wave_executor::WaveStageArrival {
                                            input_delay: planned.input_arrival_frames,
                                            output_delay: planned.output_arrival_frames,
                                        };
                                    }
                                    devices.len()
                                };
                                let moved = std::mem::replace(
                                    &mut self.return_chains[index],
                                    hex_lattice::NativeRuntimeChain::empty(),
                                );
                                match exec.push_task(
                                    index,
                                    moved,
                                    &self.return_scratch[index][..count],
                                    frame_offset,
                                    0,
                                    device_count,
                                    true,
                                    &arrivals[..device_count],
                                ) {
                                    Ok(slot) => {
                                        staged_slots[local] = Some(slot);
                                        staged_owners[local] = Some(owner);
                                    }
                                    Err(returned) => {
                                        self.return_chains[index] = returned;
                                        prep_err = Some(SessionPlaybackError::DeviceRuntime);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if let Some(err) = prep_err {
                        let mut discard = [[0.0; 2]; 256];
                        for (local, staged_slot) in staged_slots.iter().enumerate().take(wave_len) {
                            if let Some(slot) = *staged_slot {
                                let (owner_index, chain, _) =
                                    exec.reclaim_task(slot, &mut discard[..1]);
                                match staged_owners[local] {
                                    Some(crate::signal_graph::Owner::Track(_)) => {
                                        self.device_chains[owner_index] = chain;
                                    }
                                    Some(crate::signal_graph::Owner::Return(_)) => {
                                        self.return_chains[owner_index] = chain;
                                    }
                                    None => {}
                                }
                            }
                        }
                        exec.finish_wave();
                        return Err(err);
                    }
                    exec.execute_staged();
                    used_parallel_wave = true;
                }
            }

            let mut wave_err: Option<SessionPlaybackError> = None;
            for (local, staged_slot) in staged_slots.iter().enumerate().take(wave_len) {
                let order = owner_wave.start + local;
                if used_parallel_wave
                    && let Some(slot) = *staged_slot
                    && let Some(owner) = staged_owners[local]
                {
                    match owner {
                        crate::signal_graph::Owner::Track(index) => {
                            let audio = &mut audio_buf[..count];
                            let Some(exec) = wave.as_deref_mut() else {
                                if wave_err.is_none() {
                                    wave_err = Some(SessionPlaybackError::DeviceRuntime);
                                }
                                continue;
                            };
                            let (owner_index, chain, err) = exec.reclaim_task(slot, audio);
                            self.device_chains[owner_index] = chain;
                            if wave_err.is_some() {
                                continue;
                            }
                            if err.is_some() {
                                wave_err = Some(SessionPlaybackError::Arrangement(
                                    "native effect processing failed",
                                ));
                                continue;
                            }
                            let work_start = self.render_work.track(index);
                            let route = self.route_scratch[index];
                            let generator = &mut generator_buf[..count];
                            let res: Result<(), SessionPlaybackError> = (|| {
                                let compensation = self
                                    .compensation
                                    .as_mut()
                                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                                let track = &self.revision.arrangement().tracks()[index];
                                let fader = &mut self.track_fader_scratch[index];
                                let ranges = &compensation.graph.tracks[index].group_output_mutes;
                                if !track.has_gain_automation() && ranges.is_empty() {
                                    let base_mix = track.mix();
                                    if !fader.active() {
                                        base_mix
                                            .with_channel_gain(fader.current())
                                            .apply_block(audio);
                                    } else {
                                        for frame in audio.iter_mut() {
                                            *frame = base_mix
                                                .with_channel_gain(fader.next())
                                                .apply_frame(*frame);
                                        }
                                    }
                                } else {
                                    let fader_arrival = compensation.graph.plan.nodes()
                                        [compensation.graph.tracks[index].fader]
                                        .output_arrival_frames;
                                    let fader_origin = OriginWindow {
                                        history: &compensation.delays,
                                        incoming: stamps,
                                        delay: fader_arrival,
                                    };
                                    let automated = track.has_gain_automation();
                                    for (i, frame) in audio.iter_mut().enumerate() {
                                        let stamp = fader_origin
                                            .stamp_ref(i)
                                            .ok_or(SessionPlaybackError::DeviceRuntime)?;
                                        let mut mix = track.mix_at(stamp.arrangement_frame);
                                        if !ranges.is_empty() {
                                            let next = ranges.partition_point(|range| {
                                                range.end() <= stamp.arrangement_frame
                                            });
                                            if ranges.get(next).is_some_and(|range| {
                                                range.contains(stamp.arrangement_frame)
                                            }) {
                                                mix.muted = true;
                                            }
                                        }
                                        let mix = if automated {
                                            fader.follow(mix.channel_gain());
                                            mix
                                        } else {
                                            mix.with_channel_gain(fader.next())
                                        };
                                        *frame = mix.apply_frame(*frame);
                                    }
                                }
                                self.resident_source_scratch[index][..count].copy_from_slice(audio);
                                let sends = &compensation.graph.tracks[index].sends;
                                let send_audio = if let Some(tails) = &detached_sends
                                    && !sends.is_empty()
                                {
                                    generator.copy_from_slice(audio);
                                    add_main_track(
                                        generator,
                                        &tails[index * count..(index + 1) * count],
                                    );
                                    &*generator
                                } else {
                                    &*audio
                                };
                                let fader_arrival_send = compensation.graph.plan.nodes()
                                    [compensation.graph.tracks[index].fader]
                                    .output_arrival_frames;
                                let send_automated = track.has_send_automation();
                                for send_index in 0..sends.len() {
                                    let send = &compensation.graph.tracks[index].sends[send_index];
                                    let (target, static_gain, edge) =
                                        (send.target, send.gain, send.edge);
                                    let buf = &mut send_buf[..count];
                                    if send_automated {
                                        let send_origin = OriginWindow {
                                            history: &compensation.delays,
                                            incoming: stamps,
                                            delay: fader_arrival_send,
                                        };
                                        for (i, (dst, &src)) in
                                            buf.iter_mut().zip(send_audio).enumerate()
                                        {
                                            let gain =
                                                send_origin.stamp_ref(i).map_or(static_gain, |s| {
                                                    track.send_gain_at(
                                                        send_index,
                                                        s.arrangement_frame,
                                                    )
                                                });
                                            *dst = [src[0] * gain, src[1] * gain];
                                        }
                                    } else {
                                        for (dst, &src) in buf.iter_mut().zip(send_audio) {
                                            *dst = [src[0] * static_gain, src[1] * static_gain];
                                        }
                                    }
                                    compensation.edge_block(edge, buf).map_err(|error| {
                                        SessionPlaybackError::Arrangement(error.0)
                                    })?;
                                    add_main_track(&mut self.return_scratch[target][..count], buf);
                                }
                                if let crate::signal_graph::TrackOutput::Group { target, edge } =
                                    compensation.graph.tracks[index].output
                                {
                                    let routed = &mut send_buf[..count];
                                    routed.copy_from_slice(audio);
                                    if let Some(tails) = &detached {
                                        add_main_track(
                                            routed,
                                            &tails[index * count..(index + 1) * count],
                                        );
                                    }
                                    compensation.edge_block(edge, routed).map_err(|error| {
                                        SessionPlaybackError::Arrangement(error.0)
                                    })?;
                                    if let Some(capture) = capture.as_mut() {
                                        capture.add(
                                            OutputCaptureId::Track(route.id),
                                            frame_offset,
                                            routed,
                                        )?;
                                    }
                                    add_main_track(
                                        &mut self.resident_source_scratch[target][..count],
                                        routed,
                                    );
                                }
                                Ok(())
                            })(
                            );
                            self.render_work.finish(work_start);
                            if let Err(err) = res {
                                wave_err = Some(err);
                            }
                            continue;
                        }
                        crate::signal_graph::Owner::Return(index) => {
                            let buf = &mut send_buf[..count];
                            let Some(exec) = wave.as_deref_mut() else {
                                if wave_err.is_none() {
                                    wave_err = Some(SessionPlaybackError::DeviceRuntime);
                                }
                                continue;
                            };
                            let (owner_index, chain, err) = exec.reclaim_task(slot, buf);
                            self.return_chains[owner_index] = chain;
                            if wave_err.is_some() {
                                continue;
                            }
                            if err.is_some() {
                                wave_err = Some(SessionPlaybackError::Arrangement(
                                    "native effect processing failed",
                                ));
                                continue;
                            }
                            let work_start = self.render_work.return_bus(index);
                            let res: Result<(), SessionPlaybackError> = (|| {
                                let compensation = self
                                    .compensation
                                    .as_mut()
                                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                                let bus = &self.revision.arrangement().return_buses()[index];
                                if let Some(matrix) = bus.matrix {
                                    for frame in buf.iter_mut() {
                                        *frame = matrix.apply(*frame);
                                    }
                                }
                                if let Some(fade) = self
                                    .return_fade_scratch
                                    .as_mut()
                                    .filter(|fade| fade.id == bus.id)
                                {
                                    for frame in buf.iter_mut() {
                                        let gain = fade.fader.next();
                                        frame[0] *= gain[0];
                                        frame[1] *= gain[1];
                                    }
                                }
                                if bus.muted {
                                    buf.fill([0.0; 2]);
                                } else {
                                    for frame in buf.iter_mut() {
                                        frame[0] *= bus.gain;
                                        frame[1] *= bus.gain;
                                    }
                                }
                                for feed_index in 0..compensation.graph.returns[index].feeds.len() {
                                    let feed = &compensation.graph.returns[index].feeds[feed_index];
                                    let (target, gain, edge) = (feed.target, feed.gain, feed.edge);
                                    let fed = &mut audio_buf[..count];
                                    for (dst, &src) in fed.iter_mut().zip(buf.iter()) {
                                        *dst = [src[0] * gain, src[1] * gain];
                                    }
                                    compensation.edge_block(edge, fed).map_err(|error| {
                                        SessionPlaybackError::Arrangement(error.0)
                                    })?;
                                    add_main_track(&mut self.return_scratch[target][..count], fed);
                                }
                                let main_edge = compensation.graph.returns[index].main;
                                compensation
                                    .edge_block(main_edge, buf)
                                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                                self.return_scratch[index][..count].copy_from_slice(buf);
                                if let Some(capture) = capture.as_mut() {
                                    capture.add(
                                        OutputCaptureId::Return(bus.id),
                                        frame_offset,
                                        buf,
                                    )?;
                                }
                                Ok(())
                            })(
                            );
                            self.render_work.finish(work_start);
                            if let Err(err) = res {
                                wave_err = Some(err);
                            }
                            continue;
                        }
                    }
                }
                if wave_err.is_some() {
                    continue;
                }
                let Some(owner) = self
                    .compensation
                    .as_ref()
                    .map(|c| c.graph.owner_order[order])
                else {
                    wave_err = Some(SessionPlaybackError::DeviceRuntime);
                    continue;
                };
                match owner {
                    crate::signal_graph::Owner::Track(index) => {
                        let work_start = self.render_work.track(index);
                        let inline_res: Result<(), SessionPlaybackError> = (|| {
                            let route = self.route_scratch[index];
                            let audio = &mut audio_buf[..count];
                            let generator = &mut generator_buf[..count];
                            audio.fill([0.0; 2]);

                            let (
                                has_instrument,
                                has_midi_source,
                                has_foreign_instrument,
                                slot,
                                device_count,
                            ) = {
                                let track = &self.revision.arrangement().tracks()[index];
                                (
                                    track.instrument.is_some(),
                                    track.midi_source_index.is_some(),
                                    track.foreign_instrument.is_some(),
                                    track.instrument_position(),
                                    self.compensation
                                        .as_ref()
                                        .ok_or(SessionPlaybackError::DeviceRuntime)?
                                        .graph
                                        .tracks[index]
                                        .devices
                                        .len(),
                                )
                            };
                            let has_mutable_instrument = self
                                .mutable_instruments
                                .get(index)
                                .and_then(Option::as_ref)
                                .is_some();
                            let has_source_effects = self.source_effects.has_chain(index);
                            let has_analytic_source = has_instrument
                                || has_midi_source
                                || self.revision.source_routing().is_shared(index)
                                || !matches!(route.owner, RouteOwner::Arrangement);
                            // Release entries are sorted by track, tail group and voice order.
                            // The graph visits tracks in topological rather than index order.
                            let first_release = self
                                .release_scratch
                                .partition_point(|entry| entry.location.track_index() < index);
                            let remaining = &self.release_scratch[first_release..];
                            let releases = &remaining[..remaining
                                .partition_point(|entry| entry.location.track_index() == index)];
                            let keyboard_track =
                                self.computer_keyboard.as_ref().is_some_and(|keyboard| {
                                    !keyboard.is_source_input() && keyboard.track() == route.id
                                });
                            let has_generator = keyboard_track
                                || source_input_receiver == Some(index)
                                || !releases.is_empty()
                                || has_analytic_source
                                || has_mutable_instrument
                                || has_foreign_instrument
                                || has_source_effects;

                            render_route_source_chunk(
                                RouteSourceContext {
                                    revision: &self.revision,
                                    midi: crate::session_inventory::EffectiveSessionMidi::new(
                                        self.revision.session(),
                                        self.midi_admission.as_ref(),
                                    ),
                                    route,
                                    historical_floor: self.historical_floor_for(index),
                                    interactive_onset_floor: (loop_pass == 0)
                                        .then_some(self.interactive_onset_floor)
                                        .flatten(),
                                    suppressions: &self.suppression_scratch,
                                    continuations: &self.analytic_continuations,
                                    index,
                                    raw: true,
                                    loop_fade,
                                    timing: &timing,
                                    table: self.revision.session().sine_table(),
                                },
                                &mut self.spans,
                                performance_frame,
                                arrangement_frame,
                                audio,
                                pages.as_deref_mut(),
                                SourceStage::audio(
                                    self.browser_audition.voices.for_track(route.id),
                                ),
                            )?;

                            if let Some(input) =
                                inputs.live_audio.filter(|input| input.track == route.id)
                            {
                                for (i, (dst, src)) in audio
                                    .iter_mut()
                                    .zip(&input.frames[frame_offset..frame_offset + count])
                                    .enumerate()
                                {
                                    if self.track_monitors_live_audio(
                                        index,
                                        arrangement_frame + i as u64,
                                        true,
                                    ) {
                                        dst[0] += src[0];
                                        dst[1] += src[1];
                                    }
                                }
                            }

                            if has_generator {
                                generator.fill([0.0; 2]);
                            }

                            if has_analytic_source {
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
                                        interactive_onset_floor: (loop_pass == 0)
                                            .then_some(self.interactive_onset_floor)
                                            .flatten(),
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
                                    generator,
                                )?;
                            }

                            if source_input_receiver == Some(index) {
                                let authored = self.authored_source_voices(index)?;
                                source_live::render_source_live_input(
                                    &self.revision,
                                    self.computer_keyboard.as_deref_mut(),
                                    self.mutable_instruments[index].as_mut(),
                                    &mut self.mutable_onsets,
                                    &mut self.mutable_events,
                                    authored,
                                    performance_frame,
                                    generator,
                                    &timing,
                                    true,
                                    None,
                                    self.pending_shared_live,
                                )?;
                            }

                            if has_mutable_instrument {
                                render_mutable_instrument_chunk(
                                    MutableInstrumentChunkContext {
                                        revision: &self.revision,
                                        midi: crate::session_inventory::EffectiveSessionMidi::new(
                                            self.revision.session(),
                                            self.midi_admission.as_ref(),
                                        ),
                                        sources: inputs.sources,
                                        routes: &self.route_scratch,
                                        runtimes: &mut self.mutable_instruments,
                                        onsets: &mut self.mutable_onsets,
                                        suppressions: &self.suppression_scratch,
                                        events: &mut self.mutable_events,
                                        spans: &mut self.spans,
                                        performance_frame,
                                        arrangement_frame,
                                        arrangement_loop_pass: loop_pass,
                                        transient: None,
                                        chase_at_start: self.interactive_start_frame
                                            == Some(performance_frame),
                                        interactive_onset_floor: (loop_pass == 0)
                                            .then_some(self.interactive_onset_floor)
                                            .flatten(),
                                        pending_shared_chase: self.pending_shared_chase,
                                        loop_region: if self.linear_arrangement_export {
                                            None
                                        } else {
                                            region.filter(|region| region.enabled())
                                        },
                                        timing: &timing,
                                    },
                                    index,
                                    generator,
                                    self.native_parameter_state
                                        .as_deref_mut()
                                        .and_then(|state| state.processing.as_mut()),
                                )?;
                            }

                            for release in releases {
                                let remaining =
                                    release.end_frame().saturating_sub(performance_frame);
                                let frames =
                                    count.min(usize::try_from(remaining).unwrap_or(usize::MAX));
                                render_release_raw(
                                    release,
                                    self.revision.arrangement().shared_contexts(),
                                    &self.retained_contexts,
                                    &self.analytic_continuations,
                                    performance_frame,
                                    true,
                                    &mut generator[..frames],
                                )?;
                            }

                            if let Some(keyboard) = self
                                .computer_keyboard
                                .as_deref_mut()
                                .filter(|_| keyboard_track)
                            {
                                let clock = keyboard
                                    .musical_clock(&timing, performance_frame, true)
                                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                                keyboard
                                    .render(generator, Some(&clock))
                                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                            }

                            if has_foreign_instrument {
                                let track = &self.revision.arrangement().tracks()[index];
                                let midi_route = effective_midi_route(
                                    &self.revision,
                                    &self.route_scratch,
                                    index,
                                )?;
                                for i in 0..count {
                                    let clock =
                                        crate::signal_graph::OriginClock(stamps[i].as_ref());
                                    let mut wet = [[0.0; 2]];
                                    self.foreign_arrangement.render_frame(
                                        index,
                                        track,
                                        foreign_arrangement::FrameContext {
                                            performance: performance_frame + i as u64,
                                            arrangement: arrangement_frame + i as u64,
                                            loop_pass,
                                            playing: true,
                                            chase: self.interactive_start_frame
                                                == Some(performance_frame + i as u64),
                                            interactive_onset_floor: (loop_pass == 0)
                                                .then_some(self.interactive_onset_floor)
                                                .flatten(),
                                        },
                                        foreign_arrangement::SessionRoute {
                                            route: midi_route,
                                            midi:
                                                crate::session_inventory::EffectiveSessionMidi::new(
                                                    self.revision.session(),
                                                    self.midi_admission.as_ref(),
                                                ),
                                            timing: &timing,
                                            spans: &mut self.spans,
                                        },
                                        &mut wet,
                                        &mut self.mutable_onsets,
                                        frame_offset + i,
                                        foreign,
                                        &clock,
                                    )?;
                                    generator[i][0] += wet[0][0];
                                    generator[i][1] += wet[0][1];
                                }
                            }

                            let compensation = self
                                .compensation
                                .as_mut()
                                .ok_or(SessionPlaybackError::DeviceRuntime)?;

                            if has_source_effects {
                                let clock = OriginWindow {
                                    history: &compensation.delays,
                                    incoming: stamps,
                                    delay: 0,
                                };
                                self.source_effects
                                    .process_with_live_input(
                                        index,
                                        generator,
                                        &clock,
                                        frame_offset,
                                        self.native_parameter_state
                                            .as_deref_mut()
                                            .and_then(|state| state.processing.as_mut()),
                                    )
                                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                            }

                            let track = &self.revision.arrangement().tracks()[index];
                            let source_input_edge = compensation.graph.tracks[index].source_input;
                            let audio_merge_edge = compensation.graph.tracks[index].audio_merge;
                            let generator_merge_edge =
                                compensation.graph.tracks[index].generator_merge;

                            compensation
                                .edge_block(source_input_edge, audio)
                                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                            if has_group_input[index] {
                                add_main_track(
                                    audio,
                                    &self.resident_source_scratch[index][..count],
                                );
                            }
                            if device_count == 0
                                && !has_generator
                                && !has_group_input[index]
                                && generator_merge_edge.is_none()
                                && audio.iter().all(|f| (f[0].to_bits() | f[1].to_bits()) == 0)
                            {
                                compensation
                                    .edge_block(audio_merge_edge, audio)
                                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                            } else {
                                for frame in audio.iter_mut() {
                                    frame[0] += 0.0;
                                    frame[1] += 0.0;
                                }
                                for stage in 0..slot {
                                    let node = compensation.graph.tracks[index].devices[stage];
                                    compensation
                                        .process_effect_stage_block(
                                            node,
                                            stage,
                                            &mut self.device_chains[index],
                                            audio,
                                            frame_offset,
                                            stamps,
                                            &self.resident_source_scratch,
                                            &mut key_buf,
                                            foreign,
                                            observer,
                                            self.native_parameter_state
                                                .as_deref_mut()
                                                .and_then(|state| state.processing.as_mut()),
                                            None,
                                        )
                                        .map_err(|error| {
                                            SessionPlaybackError::Arrangement(error.0)
                                        })?;
                                }
                                track.apply_instrument_input_gain(audio);
                                if self.device_metering {
                                    track.instrument_meter.observe_input(audio);
                                }
                                compensation
                                    .edge_block(audio_merge_edge, audio)
                                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                                if let Some(edge) = generator_merge_edge {
                                    if !has_generator {
                                        generator.fill([0.0; 2]);
                                    }
                                    compensation.edge_block(edge, generator).map_err(|error| {
                                        SessionPlaybackError::Arrangement(error.0)
                                    })?;
                                    for i in 0..count {
                                        audio[i][0] += generator[i][0];
                                        audio[i][1] += generator[i][1];
                                    }
                                } else if has_generator {
                                    for i in 0..count {
                                        audio[i][0] += generator[i][0];
                                        audio[i][1] += generator[i][1];
                                    }
                                }
                                track.apply_instrument_output_gain(audio);
                                if self.device_metering {
                                    track.instrument_meter.observe_output(audio);
                                }
                                for stage in slot..device_count {
                                    let node = compensation.graph.tracks[index].devices[stage];
                                    compensation
                                        .process_effect_stage_block(
                                            node,
                                            stage,
                                            &mut self.device_chains[index],
                                            audio,
                                            frame_offset,
                                            stamps,
                                            &self.resident_source_scratch,
                                            &mut key_buf,
                                            foreign,
                                            observer,
                                            self.native_parameter_state
                                                .as_deref_mut()
                                                .and_then(|state| state.processing.as_mut()),
                                            None,
                                        )
                                        .map_err(|error| {
                                            SessionPlaybackError::Arrangement(error.0)
                                        })?;
                                }
                            }

                            let fader = &mut self.track_fader_scratch[index];
                            let ranges = &compensation.graph.tracks[index].group_output_mutes;
                            if !track.has_gain_automation() && ranges.is_empty() {
                                let base_mix = track.mix();
                                if !fader.active() {
                                    base_mix
                                        .with_channel_gain(fader.current())
                                        .apply_block(audio);
                                } else {
                                    for frame in audio.iter_mut() {
                                        *frame = base_mix
                                            .with_channel_gain(fader.next())
                                            .apply_frame(*frame);
                                    }
                                }
                            } else {
                                let fader_arrival = compensation.graph.plan.nodes()
                                    [compensation.graph.tracks[index].fader]
                                    .output_arrival_frames;
                                let fader_origin = OriginWindow {
                                    history: &compensation.delays,
                                    incoming: stamps,
                                    delay: fader_arrival,
                                };
                                let automated = track.has_gain_automation();
                                for (i, frame) in audio.iter_mut().enumerate() {
                                    let stamp = fader_origin
                                        .stamp_ref(i)
                                        .ok_or(SessionPlaybackError::DeviceRuntime)?;
                                    let mut mix = track.mix_at(stamp.arrangement_frame);
                                    if !ranges.is_empty() {
                                        let next = ranges.partition_point(|range| {
                                            range.end() <= stamp.arrangement_frame
                                        });
                                        if ranges.get(next).is_some_and(|range| {
                                            range.contains(stamp.arrangement_frame)
                                        }) {
                                            mix.muted = true;
                                        }
                                    }
                                    let mix = if automated {
                                        fader.follow(mix.channel_gain());
                                        mix
                                    } else {
                                        mix.with_channel_gain(fader.next())
                                    };
                                    *frame = mix.apply_frame(*frame);
                                }
                            }
                            self.resident_source_scratch[index][..count].copy_from_slice(audio);

                            let sends = &compensation.graph.tracks[index].sends;
                            let send_audio = if let Some(tails) = &detached_sends
                                && !sends.is_empty()
                            {
                                generator.copy_from_slice(audio);
                                add_main_track(
                                    generator,
                                    &tails[index * count..(index + 1) * count],
                                );
                                &*generator
                            } else {
                                &*audio
                            };
                            let fader_arrival_send = compensation.graph.plan.nodes()
                                [compensation.graph.tracks[index].fader]
                                .output_arrival_frames;
                            let send_automated = track.has_send_automation();
                            for send_index in 0..sends.len() {
                                let send = &compensation.graph.tracks[index].sends[send_index];
                                let (target, static_gain, edge) =
                                    (send.target, send.gain, send.edge);
                                let buf = &mut send_buf[..count];
                                if send_automated {
                                    let send_origin = OriginWindow {
                                        history: &compensation.delays,
                                        incoming: stamps,
                                        delay: fader_arrival_send,
                                    };
                                    for (i, (dst, &src)) in
                                        buf.iter_mut().zip(send_audio).enumerate()
                                    {
                                        let gain =
                                            send_origin.stamp_ref(i).map_or(static_gain, |s| {
                                                track.send_gain_at(send_index, s.arrangement_frame)
                                            });
                                        *dst = [src[0] * gain, src[1] * gain];
                                    }
                                } else {
                                    for (dst, &src) in buf.iter_mut().zip(send_audio) {
                                        *dst = [src[0] * static_gain, src[1] * static_gain];
                                    }
                                }
                                compensation
                                    .edge_block(edge, buf)
                                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                                add_main_track(&mut self.return_scratch[target][..count], buf);
                            }
                            if let crate::signal_graph::TrackOutput::Group { target, edge } =
                                compensation.graph.tracks[index].output
                            {
                                let routed = &mut send_buf[..count];
                                routed.copy_from_slice(audio);
                                if let Some(tails) = &detached {
                                    add_main_track(
                                        routed,
                                        &tails[index * count..(index + 1) * count],
                                    );
                                }
                                compensation
                                    .edge_block(edge, routed)
                                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                                if let Some(capture) = capture.as_mut() {
                                    capture.add(
                                        OutputCaptureId::Track(route.id),
                                        frame_offset,
                                        routed,
                                    )?;
                                }
                                add_main_track(
                                    &mut self.resident_source_scratch[target][..count],
                                    routed,
                                );
                            }
                            Ok(())
                        })(
                        );
                        self.render_work.finish(work_start);
                        if let Err(err) = inline_res {
                            wave_err = Some(err);
                        }
                    }
                    crate::signal_graph::Owner::Return(index) => {
                        let work_start = self.render_work.return_bus(index);
                        let inline_res: Result<(), SessionPlaybackError> = (|| {
                            let compensation = self
                                .compensation
                                .as_mut()
                                .ok_or(SessionPlaybackError::DeviceRuntime)?;
                            let bus = &self.revision.arrangement().return_buses()[index];
                            let buf = &mut send_buf[..count];
                            buf.copy_from_slice(&self.return_scratch[index][..count]);
                            let device_count = compensation.graph.returns[index].devices.len();
                            for stage in 0..device_count {
                                let node = compensation.graph.returns[index].devices[stage];
                                compensation
                                    .process_effect_stage_block(
                                        node,
                                        stage,
                                        &mut self.return_chains[index],
                                        buf,
                                        frame_offset,
                                        stamps,
                                        &self.resident_source_scratch,
                                        &mut key_buf,
                                        foreign,
                                        observer,
                                        self.native_parameter_state
                                            .as_deref_mut()
                                            .and_then(|state| state.processing.as_mut()),
                                        None,
                                    )
                                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                            }
                            if let Some(matrix) = bus.matrix {
                                for frame in buf.iter_mut() {
                                    *frame = matrix.apply(*frame);
                                }
                            }
                            if let Some(fade) = self
                                .return_fade_scratch
                                .as_mut()
                                .filter(|fade| fade.id == bus.id)
                            {
                                for frame in buf.iter_mut() {
                                    let gain = fade.fader.next();
                                    frame[0] *= gain[0];
                                    frame[1] *= gain[1];
                                }
                            }
                            if bus.muted {
                                buf.fill([0.0; 2]);
                            } else {
                                for frame in buf.iter_mut() {
                                    frame[0] *= bus.gain;
                                    frame[1] *= bus.gain;
                                }
                            }
                            for feed_index in 0..compensation.graph.returns[index].feeds.len() {
                                let feed = &compensation.graph.returns[index].feeds[feed_index];
                                let (target, gain, edge) = (feed.target, feed.gain, feed.edge);
                                let fed = &mut audio_buf[..count];
                                for (dst, &src) in fed.iter_mut().zip(buf.iter()) {
                                    *dst = [src[0] * gain, src[1] * gain];
                                }
                                compensation
                                    .edge_block(edge, fed)
                                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                                add_main_track(&mut self.return_scratch[target][..count], fed);
                            }
                            let main_edge = compensation.graph.returns[index].main;
                            compensation
                                .edge_block(main_edge, buf)
                                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                            self.return_scratch[index][..count].copy_from_slice(buf);
                            if let Some(capture) = capture.as_mut() {
                                capture.add(OutputCaptureId::Return(bus.id), frame_offset, buf)?;
                            }
                            Ok(())
                        })(
                        );
                        self.render_work.finish(work_start);
                        if let Err(err) = inline_res {
                            wave_err = Some(err);
                        }
                    }
                }
            }
            if used_parallel_wave && let Some(exec) = wave.as_deref_mut() {
                exec.finish_wave();
            }
            if let Some(err) = wave_err {
                return Err(err);
            }
        }

        // Advance each track's Main alignment edge and sum in canonical track
        // index order, keeping `resident_source_scratch` intact for any Main
        // sidechain key tap below.
        let track_count = self.route_scratch.len();
        for index in 0..track_count {
            let edge = {
                let compensation = self
                    .compensation
                    .as_ref()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let crate::signal_graph::TrackOutput::Main { edge } =
                    compensation.graph.tracks[index].output
                else {
                    continue;
                };
                edge
            };
            let work_start = self.render_work.track(index);
            let compensation = self
                .compensation
                .as_mut()
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let routed = &mut audio_buf[..count];
            routed.copy_from_slice(&self.resident_source_scratch[index][..count]);
            if let Some(tails) = &detached {
                add_main_track(routed, &tails[index * count..(index + 1) * count]);
            }
            compensation
                .edge_block(edge, routed)
                .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
            if let Some(capture) = capture.as_mut() {
                capture.add(
                    OutputCaptureId::Track(self.route_scratch[index].id),
                    frame_offset,
                    routed,
                )?;
            }
            add_main_track(output, routed);
            self.render_work.finish(work_start);
        }
        for scratch in &self.return_scratch {
            add_main_track(output, &scratch[..count]);
        }

        // The handoff keeps flat detached track output post-fader. It joins
        // Main after current Returns and keeps its original track capture.
        if self
            .compensation
            .as_ref()
            .is_some_and(|runtime| !runtime.graph.has_group_outputs())
        {
            self.render_detached_tails(
                performance_frame,
                output,
                frame_offset,
                capture,
                DetachedTailRouting::UNROUTED,
                foreign,
            )?;
        }
        self.return_effect_tails
            .render_chunk(output, frame_offset, capture, foreign)?;

        self.render_compensated_main_block(
            output,
            frame_offset,
            stamps,
            &mut key_buf,
            foreign,
            observer,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_compensated_main_block(
        &mut self,
        output: &mut [Frame],
        frame_offset: usize,
        stamps: &[Option<TransportStamp>],
        key_buf: &mut [Frame; 256],
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        external: Option<&dyn hex_lattice::NativeExternalAutomationSet>,
    ) -> Result<(), SessionPlaybackError> {
        let compensation = self
            .compensation
            .as_mut()
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        compensation
            .process_main_block(
                &mut self.main_chain,
                output,
                frame_offset,
                stamps,
                &self.resident_source_scratch,
                key_buf,
                foreign,
                observer,
                self.native_parameter_state
                    .as_deref_mut()
                    .and_then(|state| state.processing.as_mut()),
                external,
            )
            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
        self.return_effect_tails.render_main_chunk(
            output,
            MainTailClock::Window(compensation, stamps),
            frame_offset,
            foreign,
        )?;
        // Both current Main and any retained suffix read the same history.
        for stamp in stamps {
            let stamp = stamp.ok_or(SessionPlaybackError::DeviceRuntime)?;
            compensation
                .delays
                .push_transport_stamp(stamp)
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
