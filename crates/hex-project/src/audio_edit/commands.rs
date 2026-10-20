//! Transactional edits of the canonical retained audio domain.
use super::*;
pub(crate) fn project_clip(clip: &mut Clip, tempo: &TempoMap) -> Result<(), Invalid> {
    crate::audio_preview::project_clip(clip, tempo)
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
    crate::audio_preview::materialize(
        clip,
        crate::AudioClipPreviewOperation::Trim(edge),
        frame,
        0,
        tempo,
    )
}

pub fn default_audio_warp(
    clip: &Clip,
    tempo: &TempoMap,
    mode: WarpMode,
) -> Result<WarpSettings, Invalid> {
    crate::audio_preview::default_audio_warp(clip, tempo, mode)
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
        *project.clip_mut(id)? = stretched;
        return Ok(crate::Created::None);
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
    domain.sample_bpm = old.domain.sample_bpm;
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
    set_clip_fade_envelope(&mut clip, fade, project.tempo_map())?;
    validate_clip(project, &clip)?;
    *project.clip_mut(id)? = clip;
    Ok(crate::Created::None)
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
    let projection = if edit.musical_start.is_some() {
        Some(AudioFadeProjection {
            tempo: tempo.clone(),
            origin_beat_raw: edit.projection_origin()? + i128::from(edit.view_start),
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
    let mut clip = project
        .clip(id)
        .cloned()
        .ok_or(Invalid("audio clip is missing"))?;
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
        *project.clip_mut(id)? = clip;
        return Ok(crate::Created::None);
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
    *project.clip_mut(id)? = clip;
    Ok(crate::Created::None)
}
