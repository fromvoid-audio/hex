//! Transactional edits of the canonical retained audio domain.
use super::*;
pub(crate) fn project_clip(clip: &mut Clip, tempo: &TempoMap) -> Result<(), Invalid> {
    let Some(edit) = &clip.edit else {
        return Ok(());
    };
    if let Some(start) = edit.musical_start {
        let end = start
            .raw()
            .checked_add(edit.view_end - edit.view_start)
            .ok_or(Invalid("musical audio end overflow"))?;
        clip.start = tempo.beat_to_frame(start)?;
        clip.duration = tempo
            .beat_to_frame(BeatTime::from_raw(end))?
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
    if full > MAX_AUDIO_DOMAIN_FRAMES || start >= end {
        return Err(Invalid(
            "full audio domain exceeds its 16777216 frame limit or has an empty crop",
        ));
    }
    Ok(())
}
pub(crate) fn move_clip(clip: &mut Clip, start: u64, tempo: &TempoMap) -> Result<(), Invalid> {
    if let Some(edit) = clip.edit.as_mut()
        && edit.musical_start.is_some()
    {
        Arc::make_mut(edit).musical_start = Some(tempo.frame_to_beat(start)?);
    }
    clip.start = start;
    project_clip(clip, tempo)
}
pub(crate) fn trim_view(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    tempo: &TempoMap,
) -> Result<Clip, Invalid> {
    let edit = clip
        .edit
        .as_ref()
        .ok_or(Invalid("audio crop has no domain"))?;
    let (_, _, start_offset, _) = edit.projected_domain(tempo)?;
    let origin_frame = i128::from(clip.start) - i128::from(start_offset);
    let coordinate = match edit.domain.clock {
        AudioDomainClock::Frames(_) => u64::try_from(i128::from(frame) - origin_frame)
            .map_err(|_| Invalid("trim precedes retained audio domain"))?,
        AudioDomainClock::Beats(_) => {
            u64::try_from(i128::from(tempo.frame_to_beat(frame)?.raw()) - edit.projection_origin()?)
                .map_err(|_| Invalid("trim precedes retained audio domain"))?
        }
    };
    if coordinate > edit.domain.clock.length() {
        return Err(Invalid(
            "trim exceeds retained audio domain; edit source region or length to create a new root",
        ));
    }
    let mut result = clip.clone();
    let view = result
        .audio_edit_mut()
        .ok_or(Invalid("audio crop has no domain"))?;
    match edge {
        TrimEdge::Left if coordinate < view.view_end => {
            view.view_start = coordinate;
            if view.musical_start.is_some() {
                view.musical_start = Some(tempo.frame_to_beat(frame)?);
            }
            result.start = frame;
        }
        TrimEdge::Right if coordinate > view.view_start => view.view_end = coordinate,
        _ => return Err(Invalid("audio crop must retain a positive interval")),
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
        | Command::SetAudioClipSourceGain { clip, .. } => *clip,
        _ => return Err(Invalid("unsupported audio editing command")),
    };
    let mut clip = project.clip(id).cloned().ok_or(Invalid(
        "pitch, warp and source envelopes currently require an arrangement audio clip",
    ))?;
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
                edit.unanchor(project.tempo_map())?;
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
        Command::SetAudioClipSourceGain { points, .. } => {
            Arc::make_mut(&mut edit.domain).source_gain = points
        }
        _ => return Err(Invalid("unsupported audio editing command")),
    }
    clip.edit = Some(owned_edit);
    project_clip(&mut clip, project.tempo_map())?;
    validate_clip(project, &clip)?;
    *project.clip_mut(id)? = clip;
    Ok(crate::Created::None)
}
fn validate_clip(project: &crate::Project, clip: &Clip) -> Result<(), Invalid> {
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
    domain.source_gain = old.domain.source_gain.clone();
    domain.warp = old.domain.warp.clone();
    domain.warp_enabled = old.domain.warp_enabled;
    if old.musical_start.is_some() {
        edit.anchor(start, project.tempo_map())?;
    }
    clip.edit = Some(Arc::new(edit));
    project_clip(&mut clip, project.tempo_map())?;
    validate_clip(project, &clip)?;
    *project.clip_mut(id)? = clip;
    Ok(crate::Created::None)
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
    let mut owned_edit = clip
        .edit
        .clone()
        .ok_or(Invalid("audio domain is missing"))?;
    let edit = Arc::make_mut(&mut owned_edit);
    let domain = Arc::make_mut(&mut edit.domain);
    domain.fade = fade;
    domain.fade_start = edit.view_start;
    domain.fade_projection = edit.musical_start.map(|beat| AudioFadeProjection {
        tempo: project.tempo_map().clone(),
        origin_beat_raw: i128::from(beat.raw()),
        local_start_raw: edit.view_start,
    });
    clip.edit = Some(owned_edit);
    project_clip(&mut clip, project.tempo_map())?;
    validate_clip(project, &clip)?;
    *project.clip_mut(id)? = clip;
    Ok(crate::Created::None)
}

pub(crate) fn source_policy(
    project: &mut crate::Project,
    id: crate::ClipId,
    looping: Option<bool>,
    phase: Option<u64>,
) -> Result<crate::Created, Invalid> {
    let mut clip = project
        .clip(id)
        .cloned()
        .ok_or(Invalid("audio clip is missing"))?;
    let mut owned_edit = clip
        .edit
        .clone()
        .ok_or(Invalid("audio domain is missing"))?;
    let edit = Arc::make_mut(&mut owned_edit);
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
    *project.clip_mut(id)? = clip;
    Ok(crate::Created::None)
}
