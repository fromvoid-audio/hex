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

#[allow(clippy::too_many_arguments)]
pub(super) fn render_release_raw(
    release: &super::ReleaseEntry,
    _table: &hex_devices::SineTable,
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
