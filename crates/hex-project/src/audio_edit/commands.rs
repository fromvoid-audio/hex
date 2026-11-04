//! Transactional edits of the canonical retained audio domain.
use super::*;
pub(crate) fn project_clip(clip: &mut Clip, tempo: &TempoMap) -> Result<(), Invalid> {
    let Some(edit) = &clip.edit else {
        return Ok(());
    };
    if edit.content_view.is_some()
        || edit.placement.is_some()
        || matches!(edit.timing, AudioClipTiming::Frozen { .. })
    {
        let (start, end) = AudioViewProjection::from_clip(clip, tempo)?.outer_frame_bounds();
        clip.start = start;
        clip.duration = end - start;
    } else if let Some(start) = edit.live_musical_start() {
        let end = start
            .raw()
            .checked_add(edit.view_end - edit.view_start)
            .ok_or(Invalid("musical audio end overflow"))?;
        clip.start = super::offset_frame(tempo.beat_to_frame(start)?, edit.placement_start_offset)?;
        clip.duration = super::offset_frame(
            tempo.beat_to_frame(BeatTime::from_raw(end))?,
            edit.placement_end_offset,
        )?
        .checked_sub(clip.start)
        .filter(|length| *length > 0)
        .ok_or(Invalid("musical audio placement has no frames"))?;
    } else {
        clip.duration = edit.view_end - edit.view_start;
    }
    clip.start
        .checked_add(clip.duration)
        .ok_or(Invalid("audio placement end overflow"))?;
    clip.fade = ClipFade::neutral(clip.duration)?;
    let (_, full, start, end) = edit.projected_domain(tempo)?;
    if full > MAX_AUDIO_DOMAIN_FRAMES {
        return Err(transform_domain_limit(tempo.sample_rate()));
    }
    if start >= end {
        return Err(EMPTY_DOMAIN_CROP);
    }
    if edit.processing_projection.is_some()
        && edit.placement.is_none()
        && end - start != clip.duration
    {
        return Err(Invalid(
            "retained audio processing crop differs from its exact placed sample span",
        ));
    }
    Ok(())
}
pub(crate) fn move_clip(clip: &mut Clip, start: u64, tempo: &TempoMap) -> Result<(), Invalid> {
    let previous_duration = clip.duration;
    if let Some(edit) = clip.edit.as_mut()
        && edit.live_musical_start().is_some()
    {
        let edit = Arc::make_mut(edit);
        edit.timing = AudioClipTiming::Musical(tempo.frame_to_beat(start)?);
        if edit.processing_projection.is_some() {
            edit.retain_frame_bounds(
                start,
                start
                    .checked_add(previous_duration)
                    .ok_or(Invalid("audio moved end overflow"))?,
                tempo,
            )?;
        }
    }
    clip.start = start;
    project_clip(clip, tempo)
}
/// Trim in the immutable source placement clock, then restore live destination
/// timing. The temporary Frozen carrier never escapes or enters validation/DSP;
/// its complete source coordinates retain repeats and directed windows alike.
fn trim_retained_outer(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    tempo: &TempoMap,
) -> Result<Clip, Invalid> {
    let old = clip.audio_edit().ok_or(Invalid("audio domain missing"))?;
    let projection = old
        .processing_projection
        .as_ref()
        .ok_or(Invalid("retained audio clock missing"))?;
    let mut source = clip.clone();
    let edit = source
        .audio_edit_mut()
        .ok_or(Invalid("audio domain missing"))?;
    edit.timing = AudioClipTiming::Frozen {
        reference_start_raw: old.reference_for_processing_origin(projection.origin_beat_raw)?,
        tempo: Arc::clone(&projection.tempo),
    };
    edit.processing_projection = None;
    edit.placement_start_offset = 0;
    edit.placement_end_offset = 0;
    let mut result = trim_view(&source, edge, frame, &projection.tempo)?;
    let start = if edge == TrimEdge::Left {
        frame
    } else {
        clip.start
    };
    let end = if edge == TrimEdge::Right {
        frame
    } else {
        clip.start
            .checked_add(clip.duration)
            .ok_or(Invalid("audio trim end overflow"))?
    };
    let edit = result
        .audio_edit_mut()
        .ok_or(Invalid("audio domain missing"))?;
    let origin = edit.projection_origin()?;
    edit.timing = AudioClipTiming::Musical(if edge == TrimEdge::Left {
        tempo.frame_to_beat(start)?
    } else {
        old.live_musical_start()
            .ok_or(Invalid("retained audio needs musical placement"))?
    });
    edit.processing_projection = Some(Arc::new(AudioProcessingProjection {
        tempo: Arc::clone(&projection.tempo),
        origin_beat_raw: origin,
    }));
    edit.retain_frame_bounds(start, end, tempo)?;
    result.start = start;
    project_clip(&mut result, tempo)?;
    if result.start != start || result.duration != end - start {
        return Err(Invalid("retained audio trim changed sample bounds"));
    }
    Ok(result)
}

pub(crate) fn trim_view(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    tempo: &TempoMap,
) -> Result<Clip, Invalid> {
    if edge == TrimEdge::Left
        && let Some(prefixed) = super::prepend_silence(clip, frame)?
    {
        return Ok(prefixed);
    }
    let edit = clip
        .edit
        .as_ref()
        .ok_or(Invalid("audio crop has no domain"))?;
    if edit.placement.is_some() && edit.processing_projection.is_some() {
        return trim_retained_outer(clip, edge, frame, tempo);
    }
    if edit.content_view.is_some() {
        return super::content::trim_content(clip, edge, frame, tempo);
    }
    if edit.placement.is_some() {
        return view::trim_placement(clip, edge, frame, tempo);
    }
    let (_, _, start_offset, _) = edit.projected_domain(tempo)?;
    let origin_frame = i128::from(clip.start) - i128::from(start_offset);
    let coordinate = if edit.processing_projection.is_some() {
        edit.crop_at_frame(clip, frame, tempo)?
    } else {
        match edit.domain.clock {
            AudioDomainClock::Frames(_) => u64::try_from(i128::from(frame) - origin_frame)
                .map_err(|_| Invalid("trim precedes retained audio domain"))?,
            AudioDomainClock::Beats(_) => u64::try_from(
                edit.reference_beat_at_destination(clip, frame, tempo)?
                    - edit.projection_origin()?,
            )
            .map_err(|_| Invalid("trim precedes retained audio domain"))?,
        }
    };
    if coordinate > edit.domain.clock.length() {
        return Err(Invalid(
            "trim exceeds retained audio domain; edit source region or length to create a new root",
        ));
    }
    let reference_start = edit
        .reference_start_raw()
        .map(|_| edit.reference_beat_at_destination(clip, frame, tempo))
        .transpose()?;
    let mut result = clip.clone();
    let view = result
        .audio_edit_mut()
        .ok_or(Invalid("audio crop has no domain"))?;
    match edge {
        TrimEdge::Left if coordinate < view.view_end => {
            view.view_start = coordinate;
            view.set_reference_start_raw(reference_start)?;
            result.start = frame;
        }
        TrimEdge::Right if coordinate > view.view_start => view.view_end = coordinate,
        _ => return Err(Invalid("audio crop must retain a positive interval")),
    }
    if edit.processing_projection.is_some() {
        let start = if edge == TrimEdge::Left {
            frame
        } else {
            clip.start
        };
        let end = if edge == TrimEdge::Right {
            frame
        } else {
            clip.start
                .checked_add(clip.duration)
                .ok_or(Invalid("audio trim end overflow"))?
        };
        result
            .audio_edit_mut()
            .ok_or(Invalid("audio crop has no domain"))?
            .retain_frame_bounds(start, end, tempo)?;
    }
    project_clip(&mut result, tempo)?;
    Ok(result)
}

pub fn default_audio_warp(
    clip: &Clip,
    tempo: &TempoMap,
    mode: WarpMode,
) -> Result<WarpSettings, Invalid> {
    let mut edit = clip
        .audio_edit()
        .cloned()
        .unwrap_or_else(|| AudioClipEdit::capture(clip));
    edit.anchor(clip.start, tempo)?;
    let origin = edit.projection_origin()?;
    let frame = audio_projection_frame(tempo, origin)?;
    let end = audio_projection_beat(tempo, frame + i128::from(edit.domain.source_length))?;
    let length = u64::try_from(end - origin)
        .map_err(|_| Invalid("warp source beat length overflow"))?
        .max(1);
    Ok(WarpSettings {
        mode,
        markers: vec![
            WarpMarker {
                source_frame: edit.domain.source_offset,
                beat: BeatTime::ZERO,
            },
            WarpMarker {
                source_frame: edit
                    .domain
                    .source_offset
                    .checked_add(edit.domain.source_length)
                    .ok_or(Invalid("warp source end overflow"))?,
                beat: BeatTime::from_raw(length),
            },
        ],
    })
}

pub(crate) fn apply(
    project: &mut crate::Project,
    command: crate::Command,
) -> Result<crate::Created, Invalid> {
    use crate::Command;
    let id = match &command {
        Command::SetAudioClipPitch { clip, .. }
        | Command::SetAudioClipMusical { clip, .. }
        | Command::SetAudioClipWarp { clip, .. }
        | Command::SetAudioClipSampleBpm { clip, .. }
        | Command::StretchAudioClipEdge { clip, .. }
        | Command::SetAudioClipSourceGain { clip, .. } => *clip,
        _ => return Err(Invalid("unsupported audio editing command")),
    };
    let mut clip = project.clip(id).cloned().ok_or(Invalid(
        "pitch, warp and source envelopes currently require an arrangement audio clip",
    ))?;
    if clip
        .audio_edit()
        .is_some_and(|edit| edit.processing_projection.is_some())
        && matches!(
            command,
            Command::StretchAudioClipEdge { .. }
                | Command::SetAudioClipSampleBpm { .. }
                | Command::SetAudioClipMusical { enabled: false, .. }
        )
    {
        return Err(Invalid(
            "audio timing edits after section reorder require an unsupported processing rebase; undo the reorder first",
        ));
    }
    if let Command::StretchAudioClipEdge { edge, frame, .. } = command {
        let frames = project
            .asset(clip.asset)
            .ok_or(Invalid("audio media missing"))?
            .playback_frames();
        let stretched = super::stretch::preview_audio_clip_stretch(
            &clip,
            edge,
            frame,
            project.tempo_map(),
            frames,
        )?;
        return crate::clip_group::replace_audio(project, stretched);
    }
    if clip.edit.is_none()
        && matches!(
            &command,
            Command::SetAudioClipPitch { cents: 0, .. }
                | Command::SetAudioClipMusical { enabled: false, .. }
                | Command::SetAudioClipWarp { settings: None, .. }
        )
    {
        return Ok(crate::Created::None);
    }
    if clip.edit.is_none()
        && matches!(&command, Command::SetAudioClipSourceGain { points, .. } if points.is_empty())
    {
        return Ok(crate::Created::None);
    }
    let mut owned_edit = clip
        .edit
        .clone()
        .unwrap_or_else(|| Arc::new(AudioClipEdit::capture(&clip)));
    let edit = Arc::make_mut(&mut owned_edit);
    match command {
        Command::SetAudioClipPitch { cents, .. } => {
            if !(-4_800..=4_800).contains(&cents) {
                return Err(Invalid("audio pitch must be within -4800..4800 cents"));
            }
            Arc::make_mut(&mut edit.domain).pitch_cents = cents;
        }
        Command::SetAudioClipMusical { enabled, .. } => {
            if enabled {
                edit.anchor(clip.start, project.tempo_map())?;
            } else {
                if project.clip_alias(id).is_some() {
                    super::aliases::freeze(edit, project.tempo_map())?;
                } else {
                    edit.unanchor(project.tempo_map())?;
                }
            }
        }
        Command::SetAudioClipWarp { settings, .. } => {
            if let Some(settings) = settings {
                edit.anchor(clip.start, project.tempo_map())?;
                let domain = Arc::make_mut(&mut edit.domain);
                domain.warp = Some(settings);
                domain.warp_enabled = true;
            } else {
                Arc::make_mut(&mut edit.domain).warp_enabled = false;
            }
        }
        Command::SetAudioClipSampleBpm { tempo, .. } => {
            if !edit.domain.warp_enabled {
                return Err(Invalid("enable warp before editing sample bpm"));
            }
            let previous = edit.domain.sample_bpm;
            let (numerator, denominator) = if let Some(previous) = previous {
                (
                    u64::from(tempo.micro_bpm()),
                    u64::from(previous.micro_bpm()),
                )
            } else {
                let source = TempoMap::constant(project.sample_rate(), tempo)?;
                let desired = source.frame_to_beat(edit.domain.source_length)?.raw();
                let current = edit
                    .domain
                    .warp
                    .as_ref()
                    .and_then(|w| w.markers.last())
                    .ok_or(Invalid("warp markers missing"))?
                    .beat
                    .raw();
                (desired, current)
            };
            super::stretch::retime(
                edit,
                numerator,
                denominator,
                clip.start,
                project.tempo_map(),
            )?;
            Arc::make_mut(&mut edit.domain).sample_bpm = Some(tempo);
        }
        Command::SetAudioClipSourceGain { points, .. } => {
            Arc::make_mut(&mut edit.domain).source_gain = points
        }
        _ => return Err(Invalid("unsupported audio editing command")),
    }
    clip.edit = Some(owned_edit);
    project_clip(&mut clip, project.tempo_map())?;
    validate_clip(project, &clip)?;
    crate::clip_group::replace_audio(project, clip)
}
pub(crate) fn validate_clip(project: &crate::Project, clip: &Clip) -> Result<(), Invalid> {
    let frames = project
        .asset(clip.asset)
        .ok_or(Invalid("audio domain media is missing"))?
        .playback_frames();
    if let Some(edit) = &clip.edit {
        edit.validate(clip, project.tempo_map(), frames)?;
    }
    Ok(())
}

pub(crate) fn reset_region(
    project: &mut crate::Project,
    id: crate::ClipId,
    start: u64,
    duration: u64,
    source_offset: u64,
    source_length: u64,
) -> Result<crate::Created, Invalid> {
    let mut clip = project
        .clip(id)
        .cloned()
        .ok_or(Invalid("audio clip is missing"))?;
    if clip.has_retained_audio_roots() {
        return Err(Invalid(
            "source-region replacement cannot discard retained audio roots; use crop handles",
        ));
    }
    let old = clip
        .edit
        .clone()
        .ok_or(Invalid("audio domain is missing"))?;
    let changed_source =
        source_offset != old.domain.source_offset || source_length != old.domain.source_length;
    if changed_source && old.domain.warp.is_some() {
        return Err(Invalid(
            "source-region changes require matching explicit warp markers; crop handles retain the source domain",
        ));
    }
    clip.start = start;
    clip.duration = duration;
    clip.source_offset = source_offset;
    clip.source_length = source_length;
    if changed_source {
        clip.source_phase = 0;
    }
    clip.fade = old.domain.fade.reanchor(duration)?;
    let mut edit = AudioClipEdit::capture(&clip);
    let domain = Arc::make_mut(&mut edit.domain);
    domain.pitch_cents = old.domain.pitch_cents;
    domain.sample_bpm = old.domain.sample_bpm;
    domain.source_gain = old.domain.source_gain.clone();
    domain.warp = old.domain.warp.clone();
    domain.warp_enabled = old.domain.warp_enabled;
    if old.live_musical_start().is_some() {
        edit.anchor(start, project.tempo_map())?;
    }
    if let Some(view) = old.content_view {
        edit.content_view = Some(AudioContentView {
            restore_plain: false,
            base_view_start: edit.view_start,
            base_view_end: edit.view_end,
            base_placement: edit.placement,
            base_start_delta: 0,
            window: crate::ContentWindow::new(
                0,
                edit.placement
                    .map_or(edit.view_end - edit.view_start, |p| p.outer_length.length()),
                view.window.reversed,
            )?,
            read_resampler: view.read_resampler,
        });
    }
    clip.edit = Some(Arc::new(edit));
    project_clip(&mut clip, project.tempo_map())?;
    validate_clip(project, &clip)?;
    crate::clip_group::replace_audio(project, clip)
}

pub(crate) fn set_fades(
    project: &mut crate::Project,
    id: crate::ClipId,
    fade_in: u64,
    fade_out: u64,
) -> Result<crate::Created, Invalid> {
    let duration = project
        .clip(id)
        .ok_or(Invalid("audio clip is missing"))?
        .duration;
    set_fade_envelope(project, id, ClipFade::new(duration, 0, fade_in, fade_out)?)
}

pub(crate) fn set_fade_envelope(
    project: &mut crate::Project,
    id: crate::ClipId,
    fade: ClipFade,
) -> Result<crate::Created, Invalid> {
    let mut clip = project
        .clip(id)
        .cloned()
        .ok_or(Invalid("audio clip is missing"))?;
    set_clip_fade_envelope(&mut clip, fade, project.tempo_map())?;
    validate_clip(project, &clip)?;
    crate::clip_group::replace_audio(project, clip)
}

pub(crate) fn set_clip_fade_envelope(
    clip: &mut Clip,
    fade: ClipFade,
    tempo: &TempoMap,
) -> Result<(), Invalid> {
    let mut owned_edit = clip
        .edit
        .clone()
        .ok_or(Invalid("audio domain is missing"))?;
    let edit = Arc::make_mut(&mut owned_edit);
    let projection = if edit.reference_start_raw().is_some() {
        Some(AudioFadeProjection {
            tempo: edit.processing_tempo(tempo).clone(),
            origin_beat_raw: edit.processing_origin()? + i128::from(edit.view_start),
            local_start_raw: edit.view_start,
        })
    } else {
        None
    };
    let domain = Arc::make_mut(&mut edit.domain);
    domain.fade = fade;
    domain.fade_start = edit.view_start;
    domain.fade_projection = projection;
    clip.edit = Some(owned_edit);
    project_clip(clip, tempo)
}

pub(crate) fn source_policy(
    project: &mut crate::Project,
    id: crate::ClipId,
    looping: Option<bool>,
    phase: Option<u64>,
) -> Result<crate::Created, Invalid> {
    let direct = looping.is_some()
        && project
            .clip(id)
            .and_then(|c| c.audio_edit())
            .is_some_and(|e| e.placement.is_some());
    let clip = preview_source_policy(project, id, looping, phase)?;
    if direct {
        *project.clip_mut(id)? = clip;
        Ok(crate::Created::None)
    } else {
        crate::clip_group::replace_audio(project, clip)
    }
}
impl crate::Project {
    /// Metadata-only preview of SetClipLooping, including legacy source loops
    /// and retained Once/Repeat views. Uses the canonical edit law without I/O
    /// or project/history mutation; controls cannot publish this replacement.
    pub fn preview_audio_clip_looping(
        &self,
        clip: crate::ClipId,
        looping: bool,
    ) -> Result<Clip, Invalid> {
        preview_source_policy(self, clip, Some(looping), None)
    }
}
fn preview_source_policy(
    project: &crate::Project,
    id: crate::ClipId,
    looping: Option<bool>,
    phase: Option<u64>,
) -> Result<Clip, Invalid> {
    let mut clip = project
        .clip(id)
        .cloned()
        .ok_or(Invalid("audio clip is missing"))?;
    if clip.edit.is_none() {
        if let Some(looping) = looping {
            clip.looping = looping;
            if !looping {
                clip.source_phase = 0;
            }
        }
        validate_clip(project, &clip)?;
        return Ok(clip);
    }
    let mut owned_edit = clip
        .edit
        .clone()
        .ok_or(Invalid("audio domain is missing"))?;
    let edit = Arc::make_mut(&mut owned_edit);
    if let (Some(placement), Some(looping)) = (&mut edit.placement, looping) {
        if looping {
            placement.cycle_phase %= edit.view_end - edit.view_start;
        }
        placement.mode = if looping {
            AudioViewMode::Repeat
        } else {
            AudioViewMode::Once
        };
        // The retained processing root keeps its own source loop and phase.
        // Turning the view off must not reveal another portion of that root.
        clip.edit = Some(owned_edit);
        validate_clip(project, &clip)?;
        return Ok(clip);
    }
    let domain = Arc::make_mut(&mut edit.domain);
    if let Some(looping) = looping {
        domain.looping = looping;
        if !looping {
            domain.source_phase = 0;
        }
    }
    if let Some(phase) = phase {
        if !domain.looping || phase >= domain.source_length {
            return Err(Invalid(
                "source phase must be inside a looping source region",
            ));
        }
        domain.source_phase = phase;
    }
    clip.looping = domain.looping;
    clip.source_phase = domain.source_phase;
    clip.edit = Some(owned_edit);
    validate_clip(project, &clip)?;
    Ok(clip)
}
