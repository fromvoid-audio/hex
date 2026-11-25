//! Resident route source/owner chunk rendering, release playback, and loop cursor normalization.
use super::*;

pub(super) struct RouteSourceContext<'a, T> {
    pub(super) revision: &'a PreparedRevision,
    pub(super) midi: crate::session_inventory::EffectiveSessionMidi<'a>,
    pub(super) route: super::TrackRoute,
    pub(super) historical_floor: Option<BeatTime>,
    pub(super) interactive_onset_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
    pub(super) suppressions: &'a [MidiOccurrenceSuppression],
    pub(super) continuations: &'a [Option<hex_lattice::AnalyticContinuation>],
    pub(super) index: usize,
    pub(super) raw: bool,
    pub(super) loop_fade: Option<crate::loop_seam::LoopEndFade>,
    pub(super) timing: &'a T,
    pub(super) table: &'a hex_devices::SineTable,
}

#[derive(Clone, Copy)]
pub(super) enum SourceStage {
    Whole,
    Audio,
    Audition(browser_audition::Voices),
    Instrument,
}
impl SourceStage {
    pub(super) fn audio(audition: browser_audition::Voices) -> Self {
        if audition.is_empty() {
            Self::Audio
        } else {
            Self::Audition(audition)
        }
    }
    pub(super) fn is_audio(self) -> bool {
        matches!(self, Self::Audio | Self::Audition(_))
    }
}

pub(super) fn render_resident_session_audio(
    prepared: &crate::session_inventory::PreparedSessionAudioClip,
    audio: &crate::Audio,
    launch_frame: u64,
    performance_frame: u64,
    output: &mut [Frame],
    source_mix: crate::PreparedTrackMix,
) -> Result<(), SessionPlaybackError> {
    let frames = audio.frames();
    for (offset, frame) in output.iter_mut().enumerate() {
        let offset = u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let absolute = performance_frame
            .checked_add(offset)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let Some(elapsed) = absolute.checked_sub(launch_frame) else {
            continue;
        };
        let Some(source_frame) = prepared.source_frame(elapsed) else {
            continue;
        };
        let source_index =
            usize::try_from(source_frame).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let mut sample = *frames
            .get(source_index)
            .ok_or(SessionPlaybackError::UnknownClip)?;
        if prepared.gain != 1.0 {
            sample[0] *= prepared.gain;
            sample[1] *= prepared.gain;
        }
        source_mix.add_frame(frame, sample);
    }
    Ok(())
}

/// Renders the route's owner, then any outgoing Session audio release it
/// still carries, through the same stage, source mix and page provider.
pub(super) fn render_route_source_chunk<
    P: MediaPages + ?Sized,
    T: crate::prepared_tempo::MidiTempoBreaks,
>(
    context: RouteSourceContext<'_, T>,
    spans: &mut MidiSpanBuffer,
    performance_frame: u64,
    arrangement_frame: u64,
    output: &mut [Frame],
    mut pages: Option<&mut P>,
    stage: SourceStage,
) -> Result<(), SessionPlaybackError> {
    let (revision, route, index, raw) =
        (context.revision, context.route, context.index, context.raw);
    render_route_owner_chunk(
        context,
        spans,
        performance_frame,
        arrangement_frame,
        output,
        pages.as_deref_mut(),
        stage,
    )?;
    if let SourceStage::Audition(audition) = stage
        && !matches!(route.owner, RouteOwner::Arrangement)
        && let Some(track) = revision.arrangement().tracks().get(index)
        && track.audio_content_enabled()
    {
        audition.add(track, performance_frame, output);
    }
    let Some(release) = route.audio_release else {
        return Ok(());
    };
    if matches!(stage, SourceStage::Instrument) {
        return Ok(());
    }
    let track = revision
        .arrangement()
        .tracks()
        .get(index)
        .ok_or(SessionPlaybackError::UnknownTrack)?;
    if stage.is_audio() && !track.audio_content_enabled() {
        return Ok(());
    }
    let source_mix = if raw {
        crate::PreparedTrackMix::identity()
    } else {
        track.source_mix()
    };
    let mut scratch = [[0.0_f32; 2]; 256];
    let mut offset = 0_usize;
    while offset < output.len() {
        let start = performance_frame
            .checked_add(
                u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?,
            )
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        if !release.active_at(start) {
            break;
        }
        let remaining = usize::try_from(release.end_frame() - start).unwrap_or(usize::MAX);
        let count = (output.len() - offset).min(scratch.len()).min(remaining);
        let frames = &mut scratch[..count];
        frames.fill([0.0; 2]);
        render_session_audio_clip(
            revision,
            route.id,
            release.clip,
            release.launch_frame,
            start,
            frames,
            source_mix,
            pages.as_deref_mut(),
        )?;
        for (step, (frame, sample)) in output[offset..offset + count]
            .iter_mut()
            .zip(frames.iter())
            .enumerate()
        {
            let gain = release.gain_at(start + step as u64);
            frame[0] += sample[0] * gain;
            frame[1] += sample[1] * gain;
        }
        offset += count;
    }
    Ok(())
}

pub(super) fn render_route_owner_chunk<
    P: MediaPages + ?Sized,
    T: crate::prepared_tempo::MidiTempoBreaks,
>(
    context: RouteSourceContext<'_, T>,
    spans: &mut MidiSpanBuffer,
    performance_frame: u64,
    arrangement_frame: u64,
    output: &mut [Frame],
    mut pages: Option<&mut P>,
    stage: SourceStage,
) -> Result<(), SessionPlaybackError> {
    let RouteSourceContext {
        revision,
        midi,
        route,
        historical_floor,
        interactive_onset_floor,
        suppressions,
        continuations,
        index,
        raw,
        loop_fade,
        timing,
        table,
    } = context;
    let track = revision
        .arrangement()
        .tracks()
        .get(index)
        .ok_or(SessionPlaybackError::UnknownTrack)?;
    if stage.is_audio() && !track.audio_content_enabled() {
        return Ok(());
    }
    let source_mix = if raw {
        crate::PreparedTrackMix::identity()
    } else {
        track.source_mix()
    };
    match route.owner {
        RouteOwner::Arrangement => {
            if let SourceStage::Audition(audition) = stage {
                return audition
                    .arrangement(
                        track,
                        arrangement_frame,
                        performance_frame,
                        output,
                        pages,
                        loop_fade,
                    )
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0));
            }
            if let Some(fade) =
                loop_fade.filter(|fade| fade.intersects(arrangement_frame, output.len()))
            {
                if !matches!(stage, SourceStage::Instrument) {
                    if output.len() > 256 {
                        return Err(SessionPlaybackError::DeviceRuntime);
                    }
                    let mut scratch = [[0.0; 2]; 256];
                    let audio = &mut scratch[..output.len()];
                    if raw {
                        track.render_raw_audio(arrangement_frame, audio, pages.as_deref_mut())
                    } else if !source_mix.muted {
                        if let Some(page_provider) = pages.as_deref_mut() {
                            track.render_audio_additive_with_pages(
                                arrangement_frame,
                                audio,
                                page_provider,
                            )
                        } else {
                            track.render_audio_additive(arrangement_frame, audio)
                        }
                    } else {
                        Ok(())
                    }
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    fade.apply(arrangement_frame, audio);
                    for (target, sample) in output.iter_mut().zip(audio.iter()) {
                        target[0] += sample[0];
                        target[1] += sample[1];
                    }
                }
                if matches!(stage, SourceStage::Whole) && !source_mix.muted {
                    track
                        .render_instrument_additive_with_suppressions(
                            arrangement_frame,
                            performance_frame,
                            output,
                            suppressions,
                            continuations,
                            Some(revision.session().timing()),
                            interactive_onset_floor,
                        )
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                }
                if !matches!(stage, SourceStage::Instrument) {
                    return Ok(());
                }
            }
            if raw && matches!(stage, SourceStage::Audio) {
                track
                    .render_raw_audio(arrangement_frame, output, pages.as_deref_mut())
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                return Ok(());
            }
            if raw && matches!(stage, SourceStage::Instrument) {
                return track
                    .render_raw_instrument(
                        (arrangement_frame, performance_frame),
                        output,
                        suppressions,
                        continuations,
                        Some(revision.session().timing()),
                        interactive_onset_floor,
                    )
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0));
            }
            let has_active_suppression = suppressions
                .iter()
                .any(|entry| entry.track == route.id && entry.until_frame > performance_frame);
            match stage {
                SourceStage::Whole => {
                    if has_active_suppression
                        || interactive_onset_floor.is_some()
                        || continuations.iter().any(Option::is_some)
                    {
                        track.render_additive_with_suppressions_and_pages(
                            arrangement_frame,
                            performance_frame,
                            output,
                            suppressions,
                            pages.as_deref_mut(),
                            continuations,
                            Some(revision.session().timing()),
                            interactive_onset_floor,
                        )
                    } else if let Some(page_provider) = pages.as_deref_mut() {
                        track.render_additive_with_pages(
                            arrangement_frame,
                            output,
                            page_provider,
                            Some(revision.session().timing()),
                        )
                    } else {
                        // Arrangement volume uses the canonical clip clock,
                        // including the whole-track fast path. Buffered callers
                        // still own applying any track gain automation once.
                        track.render_additive_with_midi_floor(
                            arrangement_frame,
                            output,
                            None,
                            Some(revision.session().timing()),
                        )
                    }
                }
                SourceStage::Audio | SourceStage::Audition(_) => {
                    if let Some(page_provider) = pages.as_deref_mut() {
                        track.render_audio_additive_with_pages(
                            arrangement_frame,
                            output,
                            page_provider,
                        )
                    } else {
                        track.render_audio_additive(arrangement_frame, output)
                    }
                }
                SourceStage::Instrument => track.render_instrument_additive_with_suppressions(
                    arrangement_frame,
                    performance_frame,
                    output,
                    suppressions,
                    continuations,
                    Some(revision.session().timing()),
                    interactive_onset_floor,
                ),
            }
            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
        }
        RouteOwner::Silence => {}
        RouteOwner::Session { clip, origin, .. } => {
            if stage.is_audio() {
                return Ok(());
            }
            let prepared = midi
                .midi_clip_for_track(clip, route.id)
                .ok_or(SessionPlaybackError::UnknownClip)?;
            if prepared.track != route.id {
                return Err(SessionPlaybackError::ClipTrackMismatch);
            }
            if prepared.instrument_enabled() && !prepared.instrument_template().is_event_source() {
                let context =
                    crate::MidiInstrumentRenderContext::with_origin(origin, timing, table)
                        .with_note_chase(route.owner.note_chase());
                let context = historical_floor.map_or(context, |floor| context.after_onset(floor));
                let context = context
                    .with_suppressions(suppressions)
                    .with_continuations(continuations);
                prepared
                    .instrument_template()
                    .render_additive_with_volume(
                        context,
                        source_mix,
                        performance_frame,
                        output,
                        spans,
                        prepared.volume_program(),
                    )
                    .map_err(SessionPlaybackError::Midi)?;
            }
        }
        RouteOwner::SessionAudio { clip, launch_frame } => {
            if matches!(stage, SourceStage::Instrument) {
                return Ok(());
            }
            render_session_audio_clip(
                revision,
                route.id,
                clip,
                launch_frame,
                performance_frame,
                output,
                source_mix,
                pages,
            )?;
        }
    }
    Ok(())
}

/// Adds one Session audio clip on its launch clock to `output`.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_session_audio_clip<P: MediaPages + ?Sized>(
    revision: &PreparedRevision,
    track: TrackId,
    clip: ClipId,
    launch_frame: u64,
    performance_frame: u64,
    output: &mut [Frame],
    source_mix: crate::PreparedTrackMix,
    pages: Option<&mut P>,
) -> Result<(), SessionPlaybackError> {
    let prepared = revision
        .session()
        .audio_clip(clip)
        .ok_or(SessionPlaybackError::UnknownClip)?;
    if prepared.track != track {
        return Err(SessionPlaybackError::ClipTrackMismatch);
    }
    if !prepared.active {
        return Ok(());
    }
    match prepared.source() {
        PreparedAudioSource::Resident(audio) => {
            render_resident_session_audio(
                prepared,
                audio,
                launch_frame,
                performance_frame,
                output,
                source_mix,
            )?;
        }
        PreparedAudioSource::Streamed(source) => {
            let page_provider = pages.ok_or(SessionPlaybackError::MediaPagesUnavailable)?;
            let Some(mut elapsed) = performance_frame.checked_sub(launch_frame) else {
                return Ok(());
            };
            let mut output_offset = 0_usize;
            // Every iteration renders at least one frame or stops. A
            // one-frame cycle is bounded by the admitted block length.
            while output_offset < output.len() {
                let Some(source_frame) = prepared.source_frame(elapsed) else {
                    break;
                };
                let remaining =
                    usize::try_from(prepared.playback_end().saturating_sub(source_frame))
                        .unwrap_or(usize::MAX);
                let count = (output.len() - output_offset).min(remaining);
                let offset = u64::try_from(output_offset)
                    .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
                let timeline_frame = performance_frame
                    .checked_add(offset)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let rendered = render_streamed_page_span(
                    source_mix,
                    source,
                    StreamedPageSpan {
                        source_frame,
                        reversed: false,
                        timeline_frame,
                        output_offset,
                        count,
                        gain: prepared.gain,
                        fade: None,
                    },
                    output,
                    page_provider,
                );
                if rendered == 0 {
                    break;
                }
                let rendered = u64::try_from(rendered)
                    .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
                elapsed = elapsed
                    .checked_add(rendered)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                output_offset = output_offset
                    .checked_add(
                        usize::try_from(rendered)
                            .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?,
                    )
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_release_captured(
    release: &super::ReleaseEntry,
    _table: &hex_devices::SineTable,
    contexts: &crate::shared_analytic::SharedAnalyticContextTable,
    retained: &[Option<Arc<crate::shared_analytic::SharedContextOwner>>],
    continuations: &[Option<hex_lattice::AnalyticContinuation>],
    performance_frame: u64,
    parameters_running: bool,
    output: &mut [Frame],
    mut capture: Option<&mut [Frame]>,
) -> Result<(), SessionPlaybackError> {
    for (offset, frame) in output.iter_mut().enumerate() {
        let relative =
            u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let absolute = release
            .sample_frame(performance_frame)?
            .checked_add(relative)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let sample = if parameters_running && release.monitor_offset == 0 {
            release
                .voice
                .sample_at_with_continuation(absolute, contexts, retained, continuations)
        } else {
            release.voice.sample_at_with_clock(
                absolute,
                performance_frame
                    .checked_add(if parameters_running { relative } else { 0 })
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
                contexts,
                retained,
                continuations,
            )
        }
        .map_err(|_| SessionPlaybackError::SharedAnalyticContext)?;
        if let Some(sample) = sample {
            let sample = sample.map(|v| (f64::from(v) * release.clip_gain) as f32);
            let contribution = release.mix.apply_frame(sample);
            frame[0] += contribution[0];
            frame[1] += contribution[1];
            if let Some(captured) = capture
                .as_deref_mut()
                .and_then(|frames| frames.get_mut(offset))
            {
                captured[0] += contribution[0];
                captured[1] += contribution[1];
            }
        }
    }
    Ok(())
}

pub(super) fn render_release_raw(
    release: &super::ReleaseEntry,
    contexts: &crate::shared_analytic::SharedAnalyticContextTable,
    retained: &[Option<Arc<crate::shared_analytic::SharedContextOwner>>],
    continuations: &[Option<hex_lattice::AnalyticContinuation>],
    performance_frame: u64,
    parameters_running: bool,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    for (offset, frame) in output.iter_mut().enumerate() {
        let relative =
            u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let absolute = release
            .sample_frame(performance_frame)?
            .checked_add(relative)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let sample = if parameters_running && release.monitor_offset == 0 {
            release
                .voice
                .sample_at_with_continuation(absolute, contexts, retained, continuations)
        } else {
            release.voice.sample_at_with_clock(
                absolute,
                performance_frame
                    .checked_add(if parameters_running { relative } else { 0 })
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
                contexts,
                retained,
                continuations,
            )
        }
        .map_err(|_| SessionPlaybackError::SharedAnalyticContext)?;
        if let Some(sample) = sample {
            frame[0] += (f64::from(sample[0]) * release.clip_gain) as f32;
            frame[1] += (f64::from(sample[1]) * release.clip_gain) as f32;
        }
    }
    Ok(())
}

pub(super) fn next_pending_frame(routes: &[TrackRoute]) -> Option<u64> {
    routes
        .iter()
        .filter_map(|route| route.pending.map(|pending| pending.frame))
        .min()
}

pub(in crate::session_playback) fn normalize_loop_cursor(
    frame: u64,
    region: LoopRegion,
    loop_pass: u64,
) -> Result<(u64, u64), SessionPlaybackError> {
    if frame < region.end() {
        return Ok((frame, loop_pass));
    }
    let length = region
        .end()
        .checked_sub(region.start())
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    if length == 0 {
        return Err(SessionPlaybackError::ArithmeticOverflow);
    }
    let wraps = (frame - region.start()) / length;
    let loop_pass = loop_pass
        .checked_add(wraps)
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    Ok((
        region.start() + (frame - region.start()) % length,
        loop_pass,
    ))
}

impl SessionPlayback {
    pub(super) fn render_route_chunk<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
        chunk_timing: RouteChunkTiming,
        inputs: RenderBlockInputs<'_>,
        sinks: &mut RenderSinks<'_, '_, '_, '_, '_>,
        stage: ForeignRenderStage<'_>,
    ) -> Result<(), SessionPlaybackError> {
        let RouteChunkTiming {
            performance_frame,
            arrangement_frame,
            arrangement_loop_pass,
            frame_offset,
        } = chunk_timing;
        let RenderBlockInputs {
            sources,
            live_audio,
            ..
        } = inputs;
        let observer = &mut *sinks.observer;
        let capture = &mut *sinks.capture;
        let wave = &mut *sinks.wave;
        let ForeignRenderStage { foreign, external } = stage;
        let region = self.active_loop_region();
        let loop_fade = region.and_then(|region| {
            crate::loop_seam::LoopEndFade::new(region, self.revision.arrangement().sample_rate())
        });
        let revision = Arc::clone(&self.revision);
        let tracks = revision.arrangement().tracks();
        for scratch in &mut self.return_scratch {
            scratch[..output.len()].fill([0.0; 2]);
        }
        let timing = timing_view_for(revision.session(), self.periodic_tempo(), self.anchor);
        let effect_clock = crate::effect_clock::ArrangementEffectClock::session(
            revision.session().timing(),
            arrangement_frame,
            performance_frame,
            arrangement_loop_pass,
            self.playing,
        );
        let table = revision.session().sine_table().as_ref();

        // Phase 0: Check whether this flat track wave qualifies for parallel native DSP execution.
        let mut staged_slots = [None; crate::wave_executor::MAX_WAVE_TASKS];
        let mut used_parallel_wave = false;
        let can_parallel_tracks = observer.is_none()
            && external.is_none()
            && self
                .native_parameter_state
                .as_deref()
                .is_none_or(|state| state.processing.is_none())
            && output.len() <= crate::wave_executor::WAVE_MAX_FRAMES
            && (crate::wave_executor::PARALLEL_WAVE_MIN_TASKS
                ..=crate::wave_executor::MAX_WAVE_TASKS)
                .contains(&tracks.len())
            && wave.as_deref().is_some_and(|exec| exec.worker_count() > 0);
        if can_parallel_tracks {
            let mut eligible = [false; crate::wave_executor::MAX_WAVE_TASKS];
            let mut eligible_count = 0usize;
            let mut scan_release_cursor = 0usize;
            for (index, track) in tracks.iter().enumerate() {
                while self
                    .release_scratch
                    .get(scan_release_cursor)
                    .is_some_and(|entry| entry.location.track_index() < index)
                {
                    scan_release_cursor += 1;
                }
                let first_rel = scan_release_cursor;
                while self
                    .release_scratch
                    .get(scan_release_cursor)
                    .is_some_and(|entry| entry.location.track_index() == index)
                {
                    scan_release_cursor += 1;
                }
                let chain = &self.device_chains[index];
                if chain.is_empty()
                    || chain.len() > crate::wave_executor::MAX_WAVE_STAGES
                    || chain.foreign_bindings().next().is_some()
                    || track.instrument_position() != 0
                {
                    continue;
                }
                let audition = self
                    .browser_audition
                    .voices
                    .for_track(self.route_scratch[index].id);
                let live_target = self.computer_keyboard.as_ref().is_some_and(|keyboard| {
                    !keyboard.is_source_input() && keyboard.track() == self.route_scratch[index].id
                }) || self.source_input_receiver() == Some(index)
                    || !audition.is_empty()
                    || live_audio.is_some_and(|input| input.track == self.route_scratch[index].id)
                    || self
                        .current_sampler_keyboards
                        .iter()
                        .any(|k| k.track() == self.route_scratch[index].id);
                let shared_source = revision.source_routing().is_shared(index);
                let has_source_effects = self.source_effects.has_chain(index);
                let power_off_slot = self.receiver_power_off_slots[index]
                    .map(usize::from)
                    .filter(|&slot| {
                        self.tail_mutable_instruments[slot]
                            .as_ref()
                            .is_some_and(|tail| !tail.speculative_is_quiescent())
                    });
                if live_target || shared_source || has_source_effects || power_off_slot.is_some() {
                    continue;
                }
                let mutable_active = self.mutable_instruments.get(index).is_some_and(|runtime| {
                    runtime
                        .as_ref()
                        .is_some_and(|runtime| !runtime.committed_is_quiescent())
                });
                let has_current_release = self.release_scratch[first_rel..scan_release_cursor]
                    .iter()
                    .any(|entry| entry.location.tail_group().is_none());
                let midi_route = effective_midi_route(&self.revision, &self.route_scratch, index)?;
                let has_current_route = mutable_active
                    || !matches!(self.route_scratch[index].owner, RouteOwner::Silence)
                    || self.route_scratch[index].audio_release.is_some()
                    || !matches!(midi_route.owner, RouteOwner::Silence);
                let has_current_effect_work =
                    !has_current_route && !has_current_release && chain.needs_zero_input_render();
                if has_current_route || has_current_release || has_current_effect_work {
                    eligible[index] = true;
                    eligible_count += 1;
                }
            }
            if eligible_count >= crate::wave_executor::PARALLEL_WAVE_MIN_TASKS
                && let Some(exec) = wave.as_deref_mut()
                && let Some(clock_window) = exec.prepare_clock()
            {
                clock_window.fill_from_clock(&effect_clock, output.len());
                let mut prep_err: Option<SessionPlaybackError> = None;
                let mut prep_release_cursor = 0usize;
                for (index, track) in tracks.iter().enumerate() {
                    while self
                        .release_scratch
                        .get(prep_release_cursor)
                        .is_some_and(|entry| entry.location.track_index() < index)
                    {
                        prep_release_cursor += 1;
                    }
                    let first_release = prep_release_cursor;
                    while self
                        .release_scratch
                        .get(prep_release_cursor)
                        .is_some_and(|entry| entry.location.track_index() == index)
                    {
                        prep_release_cursor += 1;
                    }
                    if !eligible[index] {
                        continue;
                    }
                    let work_start = self.render_work.track(index);
                    let recording_track =
                        self.revision.gain_recording_track() == Some(self.route_scratch[index].id);
                    let recording_override =
                        self.gain_recording_override.filter(|_| recording_track);
                    let smoothing = self.track_fader_scratch[index].active()
                        && !track.has_gain_automation()
                        && recording_override.is_none();
                    let ordered = smoothing || track.requires_ordered_processing();
                    let metered_instrument = self.device_metering && track.instrument_id.is_some();
                    let mut track_buffer = [[0.0; 2]; 256];
                    let render_output = &mut track_buffer[..output.len()];
                    let prep_res: Result<(), SessionPlaybackError> = if ordered {
                        (|| -> Result<(), SessionPlaybackError> {
                            let midi_route =
                                effective_midi_route(&self.revision, &self.route_scratch, index)?;
                            render_route_source_chunk(
                                RouteSourceContext {
                                    revision: &self.revision,
                                    midi: crate::session_inventory::EffectiveSessionMidi::new(
                                        self.revision.session(),
                                        self.midi_admission.as_ref(),
                                    ),
                                    route: self.route_scratch[index],
                                    historical_floor: self.historical_floor_for(index),
                                    interactive_onset_floor: (arrangement_loop_pass == 0)
                                        .then_some(self.interactive_onset_floor)
                                        .flatten(),
                                    suppressions: &self.suppression_scratch,
                                    continuations: &self.analytic_continuations,
                                    index,
                                    raw: smoothing,
                                    loop_fade,
                                    timing: &timing,
                                    table,
                                },
                                &mut self.spans,
                                performance_frame,
                                arrangement_frame,
                                render_output,
                                pages.as_deref_mut(),
                                SourceStage::Audio,
                            )?;
                            track.apply_instrument_input_gain(render_output);
                            if self.device_metering {
                                track.instrument_meter.observe_input(render_output);
                            }
                            for release in &self.release_scratch[first_release..prep_release_cursor]
                            {
                                if release.location.tail_group().is_none() {
                                    render_release_raw(
                                        release,
                                        self.revision.arrangement().shared_contexts(),
                                        &self.retained_contexts,
                                        &self.analytic_continuations,
                                        performance_frame,
                                        self.playing,
                                        render_output,
                                    )?;
                                }
                            }
                            render_source_instrument_chunk(
                                RouteSourceContext {
                                    revision: &self.revision,
                                    midi: crate::session_inventory::EffectiveSessionMidi::new(
                                        self.revision.session(),
                                        self.midi_admission.as_ref(),
                                    ),
                                    route: midi_route,
                                    historical_floor: self.historical_floor_for(index),
                                    interactive_onset_floor: (arrangement_loop_pass == 0)
                                        .then_some(self.interactive_onset_floor)
                                        .flatten(),
                                    suppressions: &self.suppression_scratch,
                                    continuations: &self.analytic_continuations,
                                    index,
                                    raw: smoothing,
                                    loop_fade,
                                    timing: &timing,
                                    table,
                                },
                                &self.route_scratch,
                                self.historical_floor,
                                &self.route_historical_floors,
                                &mut self.spans,
                                performance_frame,
                                arrangement_frame,
                                render_output,
                            )?;
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
                                    arrangement_loop_pass,
                                    transient: None,
                                    chase_at_start: self.interactive_start_frame
                                        == Some(performance_frame),
                                    interactive_onset_floor: (arrangement_loop_pass == 0)
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
                                render_output,
                                None,
                            )?;
                            track.apply_instrument_output_gain(render_output);
                            if self.device_metering {
                                track.instrument_meter.observe_output(render_output);
                            }
                            Ok(())
                        })()
                    } else {
                        (|| -> Result<(), SessionPlaybackError> {
                            for release in &self.release_scratch[first_release..prep_release_cursor]
                            {
                                if release.location.tail_group().is_none() {
                                    render_release_raw(
                                        release,
                                        self.revision.arrangement().shared_contexts(),
                                        &self.retained_contexts,
                                        &self.analytic_continuations,
                                        performance_frame,
                                        self.playing,
                                        render_output,
                                    )?;
                                }
                            }
                            if metered_instrument {
                                let mut input = [[0.0; 2]; 256];
                                let input = &mut input[..output.len()];
                                render_route_source_chunk(
                                    RouteSourceContext {
                                        revision: &self.revision,
                                        midi: crate::session_inventory::EffectiveSessionMidi::new(
                                            self.revision.session(),
                                            self.midi_admission.as_ref(),
                                        ),
                                        route: self.route_scratch[index],
                                        historical_floor: self.historical_floor_for(index),
                                        interactive_onset_floor: (arrangement_loop_pass == 0)
                                            .then_some(self.interactive_onset_floor)
                                            .flatten(),
                                        suppressions: &self.suppression_scratch,
                                        continuations: &self.analytic_continuations,
                                        index,
                                        raw: false,
                                        loop_fade,
                                        timing: &timing,
                                        table,
                                    },
                                    &mut self.spans,
                                    performance_frame,
                                    arrangement_frame,
                                    input,
                                    pages.as_deref_mut(),
                                    SourceStage::Audio,
                                )?;
                                track.instrument_meter.observe_mixed(
                                    input,
                                    track.source_mix().channel_gain(),
                                    true,
                                );
                            }
                            render_route_source_chunk(
                                RouteSourceContext {
                                    revision: &self.revision,
                                    midi: crate::session_inventory::EffectiveSessionMidi::new(
                                        self.revision.session(),
                                        self.midi_admission.as_ref(),
                                    ),
                                    route: self.route_scratch[index],
                                    historical_floor: self.historical_floor_for(index),
                                    interactive_onset_floor: (arrangement_loop_pass == 0)
                                        .then_some(self.interactive_onset_floor)
                                        .flatten(),
                                    suppressions: &self.suppression_scratch,
                                    continuations: &self.analytic_continuations,
                                    index,
                                    raw: false,
                                    loop_fade,
                                    timing: &timing,
                                    table,
                                },
                                &mut self.spans,
                                performance_frame,
                                arrangement_frame,
                                render_output,
                                pages.as_deref_mut(),
                                SourceStage::Whole,
                            )?;
                            if metered_instrument {
                                track.instrument_meter.observe_mixed(
                                    render_output,
                                    track.source_mix().channel_gain(),
                                    false,
                                );
                            }
                            Ok(())
                        })()
                    };
                    self.render_work.finish(work_start);
                    if let Err(err) = prep_res {
                        prep_err = Some(err);
                        break;
                    }
                    let effect_count = self.device_chains[index].len();
                    let moved = std::mem::replace(
                        &mut self.device_chains[index],
                        hex_lattice::NativeRuntimeChain::empty(),
                    );
                    match exec.push_task(
                        index,
                        moved,
                        render_output,
                        frame_offset,
                        0,
                        effect_count,
                        false,
                        &[],
                    ) {
                        Ok(slot) => staged_slots[index] = Some(slot),
                        Err(returned) => {
                            self.device_chains[index] = returned;
                            prep_err = Some(SessionPlaybackError::DeviceRuntime);
                            break;
                        }
                    }
                }
                if let Some(err) = prep_err {
                    let mut discard = [[0.0; 2]; 256];
                    for slot in staged_slots[..tracks.len()].iter().flatten() {
                        let (owner_index, chain, _) = exec.reclaim_task(*slot, &mut discard[..1]);
                        self.device_chains[owner_index] = chain;
                    }
                    exec.finish_wave();
                    return Err(err);
                }
                exec.execute_staged();
                used_parallel_wave = true;
            }
        }

        let mut release_cursor = 0;
        let mut wave_err: Option<SessionPlaybackError> = None;
        for (index, staged_slot) in staged_slots.iter().enumerate().take(tracks.len()) {
            if used_parallel_wave && let Some(slot) = *staged_slot {
                let mut track_buffer = [[0.0; 2]; 256];
                let render_output = &mut track_buffer[..output.len()];
                let (owner_index, chain, err) = wave
                    .as_deref_mut()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?
                    .reclaim_task(slot, render_output);
                self.device_chains[owner_index] = chain;
                while self
                    .release_scratch
                    .get(release_cursor)
                    .is_some_and(|entry| entry.location.track_index() <= index)
                {
                    release_cursor += 1;
                }
                if wave_err.is_some() {
                    continue;
                }
                if err.is_some() {
                    wave_err = Some(SessionPlaybackError::DeviceRuntime);
                    continue;
                }
                let work_start = self.render_work.track(index);
                let track = tracks
                    .get(index)
                    .ok_or(SessionPlaybackError::UnknownTrack)?;
                let capture_id = OutputCaptureId::Track(self.route_scratch[index].id);
                let recording_track =
                    self.revision.gain_recording_track() == Some(self.route_scratch[index].id);
                let recording_override = self.gain_recording_override.filter(|_| recording_track);
                let smoothing = self.track_fader_scratch[index].active()
                    && !track.has_gain_automation()
                    && recording_override.is_none();
                let finish_res: Result<(), SessionPlaybackError> = (|| {
                    for (frame_index, source) in render_output.iter_mut().enumerate() {
                        let content_frame = arrangement_frame
                            .checked_add(frame_index as u64)
                            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                        let live_gain = if recording_track {
                            self.live_gain_scratch.next(
                                track,
                                content_frame,
                                recording_override,
                                revision.arrangement().sample_rate(),
                            )
                        } else {
                            None
                        };
                        let mix = if let Some(gain) = live_gain {
                            track.recording_mix(gain)
                        } else if track.has_gain_automation() {
                            track.mix_at(content_frame)
                        } else {
                            track.mix()
                        };
                        let mix = if track.has_gain_automation() || recording_override.is_some() {
                            self.track_fader_scratch[index].follow(mix.channel_gain());
                            mix
                        } else if smoothing {
                            mix.with_channel_gain(self.track_fader_scratch[index].next())
                        } else {
                            mix
                        };
                        let contribution = if track.sends().is_empty() {
                            mix.add_frame(&mut output[frame_index], *source);
                            mix.apply_frame(*source)
                        } else {
                            let contribution = mix.apply_frame(*source);
                            output[frame_index][0] += contribution[0];
                            output[frame_index][1] += contribution[1];
                            for (send_index, send) in track.sends().iter().enumerate() {
                                let send_gain = track.send_gain_at(send_index, content_frame);
                                let target = self
                                    .return_scratch
                                    .get_mut(send.return_index)
                                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                                target[frame_index][0] += contribution[0] * send_gain;
                                target[frame_index][1] += contribution[1] * send_gain;
                            }
                            contribution
                        };
                        if capture.is_some() || self.device_metering {
                            *source = contribution;
                        }
                    }
                    if let Some(capture) = capture.as_mut() {
                        capture.add(capture_id, frame_offset, render_output)?;
                    }
                    Ok(())
                })();
                if let Err(err) = finish_res {
                    wave_err = Some(err);
                }
                self.render_work.finish(work_start);
                continue;
            }
            if wave_err.is_some() {
                continue;
            }
            let work_start = self.render_work.track(index);
            let track = tracks
                .get(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let capture_id = OutputCaptureId::Track(self.route_scratch[index].id);
            let recording_track =
                self.revision.gain_recording_track() == Some(self.route_scratch[index].id);
            let recording_override = self.gain_recording_override.filter(|_| recording_track);
            let stateful_effects = self
                .device_chains
                .get(index)
                .is_some_and(|chain| !chain.is_empty());
            let mutable_active = self.mutable_instruments.get(index).is_some_and(|runtime| {
                runtime
                    .as_ref()
                    .is_some_and(|runtime| !runtime.committed_is_quiescent())
            });
            let metered_instrument = self.device_metering && track.instrument_id.is_some();
            let audition = self
                .browser_audition
                .voices
                .for_track(self.route_scratch[index].id);
            let live_target = self.computer_keyboard.as_ref().is_some_and(|keyboard| {
                !keyboard.is_source_input() && keyboard.track() == self.route_scratch[index].id
            }) || self.source_input_receiver() == Some(index)
                || !audition.is_empty()
                || live_audio.is_some_and(|input| input.track == self.route_scratch[index].id);
            let live_target = live_target
                || self
                    .current_sampler_keyboards
                    .iter()
                    .any(|k| k.track() == self.route_scratch[index].id);
            let shared_source = revision.source_routing().is_shared(index);
            let smoothing = self.track_fader_scratch[index].active()
                && !track.has_gain_automation()
                && recording_override.is_none();
            let has_source_effects = self.source_effects.has_chain(index);
            let power_off_slot = self.receiver_power_off_slots[index]
                .map(usize::from)
                .filter(|&slot| {
                    self.tail_mutable_instruments[slot]
                        .as_ref()
                        .is_some_and(|tail| !tail.speculative_is_quiescent())
                });
            let buffered = has_source_effects
                || power_off_slot.is_some()
                || shared_source
                || live_target
                || stateful_effects
                || mutable_active
                || !track.sends().is_empty()
                || track.has_gain_automation()
                || smoothing
                || self.device_metering;
            let ordered = has_source_effects
                || power_off_slot.is_some()
                || shared_source
                || live_target
                || smoothing
                || track.requires_ordered_processing();
            let already_mixed = self.device_metering
                && !ordered
                && !stateful_effects
                && track.sends().is_empty()
                && !track.has_gain_automation();
            while self
                .release_scratch
                .get(release_cursor)
                .is_some_and(|entry| entry.location.track_index() < index)
            {
                release_cursor += 1;
            }
            let first_release = release_cursor;
            while self
                .release_scratch
                .get(release_cursor)
                .is_some_and(|entry| entry.location.track_index() == index)
            {
                release_cursor += 1;
            }
            let has_current_release = self.release_scratch[first_release..release_cursor]
                .iter()
                .any(|entry| entry.location.tail_group().is_none());
            let midi_route = effective_midi_route(&self.revision, &self.route_scratch, index)?;
            let has_current_route = (shared_source
                && shared_source_has_route(&self.revision, &self.route_scratch, index)?)
                || mutable_active
                || power_off_slot.is_some()
                || live_target
                || !matches!(self.route_scratch[index].owner, RouteOwner::Silence)
                || self.route_scratch[index].audio_release.is_some()
                || !matches!(midi_route.owner, RouteOwner::Silence);
            let has_current_effect_work = !has_current_route
                && !has_current_release
                && (self.source_effects.needs_zero_input_render(index)
                    || self
                        .device_chains
                        .get(index)
                        .is_some_and(|chain| chain.needs_zero_input_render()));
            if (buffered || ordered)
                && (has_current_route
                    || has_current_release
                    || has_current_effect_work
                    || live_target)
            {
                let mut track_buffer = [[0.0; 2]; 256];
                let render_output = &mut track_buffer[..output.len()];
                if ordered {
                    render_route_source_chunk(
                        RouteSourceContext {
                            revision: &self.revision,
                            midi: crate::session_inventory::EffectiveSessionMidi::new(
                                self.revision.session(),
                                self.midi_admission.as_ref(),
                            ),
                            route: self.route_scratch[index],
                            historical_floor: self.historical_floor_for(index),
                            interactive_onset_floor: (arrangement_loop_pass == 0)
                                .then_some(self.interactive_onset_floor)
                                .flatten(),
                            suppressions: &self.suppression_scratch,
                            continuations: &self.analytic_continuations,
                            index,
                            raw: has_source_effects || live_target || shared_source || smoothing,
                            loop_fade,
                            timing: &timing,
                            table,
                        },
                        &mut self.spans,
                        performance_frame,
                        arrangement_frame,
                        render_output,
                        pages.as_deref_mut(),
                        SourceStage::audio(audition),
                    )?;
                    if let Some(input) =
                        live_audio.filter(|input| input.track == self.route_scratch[index].id)
                    {
                        for (frame, source) in render_output
                            .iter_mut()
                            .zip(&input.frames[frame_offset..frame_offset + output.len()])
                        {
                            frame[0] += source[0];
                            frame[1] += source[1];
                        }
                    }
                    if stateful_effects {
                        crate::native_parameter_input::process_range_with_external(
                            self.native_parameter_state
                                .as_deref_mut()
                                .and_then(|state| state.processing.as_mut()),
                            external,
                            &mut self.device_chains[index],
                            0..track.instrument_position(),
                            render_output,
                            frame_offset,
                            Some(&effect_clock),
                            Some(&effect_clock),
                            0,
                            0,
                            &mut hex_lattice::RefuseForeign,
                            observer,
                        )
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    } else {
                        track.apply_ordered_prefix(render_output);
                    }
                    track.apply_instrument_input_gain(render_output);
                    if self.device_metering {
                        track.instrument_meter.observe_input(render_output);
                    }
                    // For a prepared source unit, isolate the complete generator
                    // sum from the receiver's own clips before its effects run.
                    let mut generator_buffer;
                    let mut isolated = if has_source_effects {
                        generator_buffer = [[0.0; 2]; 256];
                        Some(&mut generator_buffer[..output.len()])
                    } else {
                        None
                    };
                    let generator = isolated.as_deref_mut().unwrap_or(&mut *render_output);
                    for release in &self.release_scratch[first_release..release_cursor] {
                        if release.location.tail_group().is_none() {
                            render_release_raw(
                                release,
                                self.revision.arrangement().shared_contexts(),
                                &self.retained_contexts,
                                &self.analytic_continuations,
                                performance_frame,
                                self.playing,
                                generator,
                            )?;
                        }
                    }
                    render_source_instrument_chunk(
                        RouteSourceContext {
                            revision: &self.revision,
                            midi: crate::session_inventory::EffectiveSessionMidi::new(
                                self.revision.session(),
                                self.midi_admission.as_ref(),
                            ),
                            route: midi_route,
                            historical_floor: self.historical_floor_for(index),
                            interactive_onset_floor: (arrangement_loop_pass == 0)
                                .then_some(self.interactive_onset_floor)
                                .flatten(),
                            suppressions: &self.suppression_scratch,
                            continuations: &self.analytic_continuations,
                            index,
                            raw: has_source_effects || live_target || shared_source || smoothing,
                            loop_fade,
                            timing: &timing,
                            table,
                        },
                        &self.route_scratch,
                        self.historical_floor,
                        &self.route_historical_floors,
                        &mut self.spans,
                        performance_frame,
                        arrangement_frame,
                        generator,
                    )?;
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
                            generator,
                            &timing,
                            true,
                            None,
                            self.pending_shared_live,
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
                            arrangement_loop_pass,
                            transient: None,
                            chase_at_start: self.interactive_start_frame == Some(performance_frame),
                            interactive_onset_floor: (arrangement_loop_pass == 0)
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
                    if let Some(keyboard) = self.computer_keyboard.as_mut().filter(|keyboard| {
                        !keyboard.is_source_input()
                            && keyboard.track() == self.route_scratch[index].id
                    }) {
                        let clock = keyboard
                            .musical_clock(&timing, performance_frame, true)
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        keyboard
                            .render(generator, Some(&clock))
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    }
                    for keyboard in self
                        .current_sampler_keyboards
                        .iter_mut()
                        .filter(|k| k.track() == self.route_scratch[index].id)
                    {
                        let source = sources.ok_or(SessionPlaybackError::DeviceRuntime)?;
                        let resources = source
                            .resources(keyboard.track())
                            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                        keyboard
                            .render_with_resources(generator, None, &resources)
                            .map_err(|e| SessionPlaybackError::Arrangement(e.0))?;
                    }
                    if let Some(slot) = power_off_slot
                        && let Some(tail) = self.tail_mutable_instruments[slot].as_mut()
                    {
                        process_mutable_tail(
                            tail,
                            &mut self.mutable_onsets,
                            &mut self.mutable_events,
                            performance_frame,
                            generator,
                            &timing,
                            true,
                        )?;
                    }
                    if let Some(generated) = isolated {
                        self.source_effects
                            .process_with_live_input(
                                index,
                                generated,
                                &effect_clock,
                                frame_offset,
                                self.native_parameter_state
                                    .as_deref_mut()
                                    .and_then(|state| state.processing.as_mut()),
                            )
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        for (target, source) in render_output.iter_mut().zip(generated) {
                            target[0] += source[0];
                            target[1] += source[1];
                        }
                    }
                    track.apply_instrument_output_gain(render_output);
                    if self.device_metering {
                        track.instrument_meter.observe_output(render_output);
                    }
                    if stateful_effects {
                        let chain = self
                            .device_chains
                            .get_mut(index)
                            .ok_or(SessionPlaybackError::UnknownTrack)?;
                        let effect_count = chain.len();
                        crate::native_parameter_input::process_range_with_external(
                            self.native_parameter_state
                                .as_deref_mut()
                                .and_then(|state| state.processing.as_mut()),
                            external,
                            chain,
                            track.instrument_position()..effect_count,
                            render_output,
                            frame_offset,
                            Some(&effect_clock),
                            Some(&effect_clock),
                            0,
                            0,
                            &mut hex_lattice::RefuseForeign,
                            observer,
                        )
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    } else {
                        track.apply_ordered_suffix(render_output);
                    }
                } else {
                    for release in &self.release_scratch[first_release..release_cursor] {
                        if release.location.tail_group().is_none() {
                            if already_mixed {
                                render_release_captured(
                                    release,
                                    table,
                                    self.revision.arrangement().shared_contexts(),
                                    &self.retained_contexts,
                                    &self.analytic_continuations,
                                    performance_frame,
                                    self.playing,
                                    render_output,
                                    None,
                                )?;
                                continue;
                            }
                            render_release_raw(
                                release,
                                self.revision.arrangement().shared_contexts(),
                                &self.retained_contexts,
                                &self.analytic_continuations,
                                performance_frame,
                                self.playing,
                                render_output,
                            )?;
                        }
                    }
                    if metered_instrument {
                        let mut input = [[0.0; 2]; 256];
                        let input = &mut input[..output.len()];
                        render_route_source_chunk(
                            RouteSourceContext {
                                revision: &self.revision,
                                midi: crate::session_inventory::EffectiveSessionMidi::new(
                                    self.revision.session(),
                                    self.midi_admission.as_ref(),
                                ),
                                route: self.route_scratch[index],
                                historical_floor: self.historical_floor_for(index),
                                interactive_onset_floor: (arrangement_loop_pass == 0)
                                    .then_some(self.interactive_onset_floor)
                                    .flatten(),
                                suppressions: &self.suppression_scratch,
                                continuations: &self.analytic_continuations,
                                index,
                                raw: false,
                                loop_fade,
                                timing: &timing,
                                table,
                            },
                            &mut self.spans,
                            performance_frame,
                            arrangement_frame,
                            input,
                            pages.as_deref_mut(),
                            SourceStage::Audio,
                        )?;
                        track.instrument_meter.observe_mixed(
                            input,
                            track.source_mix().channel_gain(),
                            true,
                        );
                    }
                    render_route_source_chunk(
                        RouteSourceContext {
                            revision: &self.revision,
                            midi: crate::session_inventory::EffectiveSessionMidi::new(
                                self.revision.session(),
                                self.midi_admission.as_ref(),
                            ),
                            route: self.route_scratch[index],
                            historical_floor: self.historical_floor_for(index),
                            interactive_onset_floor: (arrangement_loop_pass == 0)
                                .then_some(self.interactive_onset_floor)
                                .flatten(),
                            suppressions: &self.suppression_scratch,
                            continuations: &self.analytic_continuations,
                            index,
                            raw: false,
                            loop_fade,
                            timing: &timing,
                            table,
                        },
                        &mut self.spans,
                        performance_frame,
                        arrangement_frame,
                        render_output,
                        pages.as_deref_mut(),
                        SourceStage::Whole,
                    )?;
                    if metered_instrument {
                        track.instrument_meter.observe_mixed(
                            render_output,
                            track.source_mix().channel_gain(),
                            false,
                        );
                    }
                    let chain = self
                        .device_chains
                        .get_mut(index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    let range = 0..chain.len();
                    crate::native_parameter_input::process_range_with_external(
                        self.native_parameter_state
                            .as_deref_mut()
                            .and_then(|state| state.processing.as_mut()),
                        external,
                        chain,
                        range,
                        render_output,
                        frame_offset,
                        Some(&effect_clock),
                        Some(&effect_clock),
                        0,
                        0,
                        &mut hex_lattice::RefuseForeign,
                        observer,
                    )
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                }
                for (frame_index, source) in render_output.iter_mut().enumerate() {
                    let content_frame = arrangement_frame
                        .checked_add(frame_index as u64)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    let live_gain = if recording_track {
                        self.live_gain_scratch.next(
                            track,
                            content_frame,
                            recording_override,
                            self.revision.arrangement().sample_rate(),
                        )
                    } else {
                        None
                    };
                    let mix = if let Some(gain) = live_gain {
                        track.recording_mix(gain)
                    } else if track.has_gain_automation() {
                        track.mix_at(content_frame)
                    } else {
                        track.mix()
                    };
                    let mix = if track.has_gain_automation() || recording_override.is_some() {
                        self.track_fader_scratch[index].follow(mix.channel_gain());
                        mix
                    } else if smoothing {
                        mix.with_channel_gain(self.track_fader_scratch[index].next())
                    } else {
                        mix
                    };
                    let mix = if live_target {
                        mix.without_effects()
                    } else {
                        mix
                    };
                    let contribution = if already_mixed {
                        // Preserve the analytic per-voice fader summation order.
                        output[frame_index][0] += source[0];
                        output[frame_index][1] += source[1];
                        *source
                    } else if track.sends().is_empty() {
                        mix.add_frame(&mut output[frame_index], *source);
                        mix.apply_frame(*source)
                    } else {
                        let contribution = mix.apply_frame(*source);
                        output[frame_index][0] += contribution[0];
                        output[frame_index][1] += contribution[1];
                        for (send_index, send) in track.sends().iter().enumerate() {
                            let send_gain = track.send_gain_at(send_index, content_frame);
                            let target = self
                                .return_scratch
                                .get_mut(send.return_index)
                                .ok_or(SessionPlaybackError::DeviceRuntime)?;
                            target[frame_index][0] += contribution[0] * send_gain;
                            target[frame_index][1] += contribution[1] * send_gain;
                        }
                        contribution
                    };
                    if capture.is_some() || self.device_metering {
                        *source = contribution;
                    }
                }
                if let Some(capture) = capture.as_mut() {
                    capture.add(capture_id, frame_offset, render_output)?;
                }
            } else {
                let selected = capture
                    .as_ref()
                    .is_some_and(|capture| capture.selects(capture_id))
                    && matches!(self.route_scratch[index].owner, RouteOwner::Arrangement);
                if selected {
                    let mut captured = [[0.0; 2]; 256];
                    for release in &self.release_scratch[first_release..release_cursor] {
                        if release.location.tail_group().is_none() {
                            render_release_captured(
                                release,
                                table,
                                self.revision.arrangement().shared_contexts(),
                                &self.retained_contexts,
                                &self.analytic_continuations,
                                performance_frame,
                                self.playing,
                                output,
                                Some(&mut captured[..output.len()]),
                            )?;
                        }
                    }

                    track
                        .render_additive_captured(
                            arrangement_frame,
                            performance_frame,
                            output,
                            &self.suppression_scratch,
                            pages.as_deref_mut(),
                            &mut captured[..output.len()],
                            &self.analytic_continuations,
                            Some(self.revision.session().timing()),
                            loop_fade,
                            (arrangement_loop_pass == 0)
                                .then_some(self.interactive_onset_floor)
                                .flatten(),
                        )
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    if let Some(capture) = capture.as_mut() {
                        capture.add(capture_id, frame_offset, &captured[..output.len()])?;
                    }
                    self.render_work.finish(work_start);
                    continue;
                }
                for release in &self.release_scratch[first_release..release_cursor] {
                    if release.location.tail_group().is_none() {
                        render_release_captured(
                            release,
                            table,
                            self.revision.arrangement().shared_contexts(),
                            &self.retained_contexts,
                            &self.analytic_continuations,
                            performance_frame,
                            self.playing,
                            output,
                            None,
                        )?;
                    }
                }
                render_route_source_chunk(
                    RouteSourceContext {
                        revision: &self.revision,
                        midi: crate::session_inventory::EffectiveSessionMidi::new(
                            self.revision.session(),
                            self.midi_admission.as_ref(),
                        ),
                        route: self.route_scratch[index],
                        historical_floor: self.historical_floor_for(index),
                        interactive_onset_floor: (arrangement_loop_pass == 0)
                            .then_some(self.interactive_onset_floor)
                            .flatten(),
                        suppressions: &self.suppression_scratch,
                        continuations: &self.analytic_continuations,
                        index,
                        raw: false,
                        loop_fade,
                        timing: &timing,
                        table,
                    },
                    &mut self.spans,
                    performance_frame,
                    arrangement_frame,
                    output,
                    pages.as_deref_mut(),
                    SourceStage::Whole,
                )?;
            }
            self.render_work.finish(work_start);
        }
        if used_parallel_wave && let Some(exec) = wave.as_deref_mut() {
            exec.finish_wave();
        }
        if let Some(err) = wave_err {
            return Err(err);
        }
        self.render_detached_tails(
            performance_frame,
            output,
            frame_offset,
            capture,
            DetachedTailRouting::UNROUTED,
            foreign,
        )?;
        self.render_return_buses(
            output,
            frame_offset,
            Some(&effect_clock),
            observer,
            capture,
            wave,
            ForeignRenderStage { foreign, external },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_pending_frame_returns_none_for_empty_routes_and_wraps_loop_cursor() {
        assert_eq!(next_pending_frame(&[]), None);
        let region = hex_project::LoopRegion::new(100, 200, true).unwrap();
        let (frame, pass) = normalize_loop_cursor(250, region, 0).unwrap();
        assert_eq!(frame, 150);
        assert_eq!(pass, 1);
    }
}
