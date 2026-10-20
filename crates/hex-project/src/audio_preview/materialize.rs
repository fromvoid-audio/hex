//! Owning control-side application, deliberately outside the scoped evaluator.
use super::plan::ProjectionPlan;
use super::*;
use std::sync::Arc;

pub(crate) fn materialize(
    clip: &Clip,
    operation: AudioClipPreviewOperation,
    frame: u64,
    asset_frames: u64,
    tempo: &TempoMap,
) -> Result<Clip, Invalid> {
    let plan = AudioClipPreviewRead::new(clip, operation, frame, asset_frames, tempo)?.plan;
    let mut result = clip.clone();
    if !plan.facts.unchanged {
        if let Some(edit) = plan.edit {
            let mut owned = clip
                .audio_edit()
                .cloned()
                .unwrap_or_else(|| AudioClipEdit::capture(clip));
            edit.apply(&mut owned, tempo)?;
            result.edit = Some(Arc::new(owned));
        }
        plan.facts.write(&mut result);
    }
    Ok(result)
}

pub(crate) fn materialize_plain(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    asset_frames: u64,
) -> Result<Clip, Invalid> {
    let facts = plain::plain_trim(clip, edge, frame, asset_frames)?;
    let mut result = clip.clone();
    facts.write(&mut result);
    Ok(result)
}

pub(crate) fn project_clip(clip: &mut Clip, tempo: &TempoMap) -> Result<(), Invalid> {
    if let Some(edit) = clip.audio_edit() {
        let (start, duration, fade) =
            plan::project_fields(clip, clip.start, EditPlan::original(edit), tempo)?;
        clip.start = start;
        clip.duration = duration;
        clip.fade = fade;
    }
    Ok(())
}

pub(crate) fn anchor_edit(
    edit: &mut AudioClipEdit,
    clip_start: u64,
    tempo: &TempoMap,
) -> Result<(), Invalid> {
    if edit.musical_start.is_some() {
        return Ok(());
    }
    let mut plan = EditPlan::original(edit);
    plan.anchor(clip_start, edit.domain.events.is_some(), tempo)?;
    plan.apply(edit, tempo)
}

pub(crate) fn retime_edit(
    edit: &mut AudioClipEdit,
    numerator: u64,
    denominator: u64,
    start: u64,
    tempo: &TempoMap,
) -> Result<(), Invalid> {
    if numerator == denominator && numerator != 0 {
        return Ok(());
    }
    let mut plan = EditPlan::original(edit);
    plan.retime(
        Scale {
            numerator,
            denominator,
        },
        start,
        Some(edit),
        tempo,
    )?;
    plan.apply(edit, tempo)
}

pub(crate) fn default_audio_warp(
    clip: &Clip,
    tempo: &TempoMap,
    mode: WarpMode,
) -> Result<crate::WarpSettings, Invalid> {
    Ok(crate::WarpSettings {
        mode,
        markers: default_audio_warp_markers(clip, tempo)?.to_vec(),
    })
}

impl EditPlan {
    pub fn apply(self, edit: &mut AudioClipEdit, tempo: &TempoMap) -> Result<(), Invalid> {
        edit.view_start = self.view_start;
        edit.view_end = self.view_end;
        edit.musical_start = self.musical_start;
        edit.placement = self.placement;
        // A crop changes only the small view. Do not detach its accepted
        // processing root merely to write identical domain fields.
        let projection_changed = match self.projection {
            ProjectionPlan::Retained => false,
            ProjectionPlan::None => edit.domain.fade_projection.is_some(),
            ProjectionPlan::Current {
                origin_beat_raw,
                local_start_raw,
            } => edit.domain.fade_projection.as_ref().is_none_or(|p| {
                p.origin_beat_raw != origin_beat_raw
                    || p.local_start_raw != local_start_raw
                    || &p.tempo != tempo
            }),
        };
        if self.clock == edit.domain.clock
            && self.fade == edit.domain.fade
            && self.fade_start == edit.domain.fade_start
            && self.warp_enabled == edit.domain.warp_enabled
            && self.warp == WarpPlan::Retained
            && self.retime.is_none()
            && !projection_changed
        {
            return Ok(());
        }
        let domain = Arc::make_mut(&mut edit.domain);
        domain.clock = self.clock;
        domain.fade = self.fade;
        domain.fade_start = self.fade_start;
        domain.warp_enabled = self.warp_enabled;
        if let WarpPlan::Default { mode, markers } = self.warp {
            domain.warp = Some(crate::WarpSettings {
                mode,
                markers: markers.to_vec(),
            });
        }
        if let Some(scale) = self.retime {
            if let Some(warp) = &mut domain.warp {
                for marker in &mut warp.markers {
                    marker.beat = BeatTime::from_raw(scale.at(marker.beat.raw())?);
                }
            }
            if let Some(events) = &mut domain.events {
                for event in &mut Arc::make_mut(events).events {
                    *event = scale.event(*event)?;
                }
            }
        }
        match self.projection {
            ProjectionPlan::None => domain.fade_projection = None,
            ProjectionPlan::Retained => {}
            ProjectionPlan::Current {
                origin_beat_raw,
                local_start_raw,
            } => {
                domain.fade_projection = Some(AudioFadeProjection {
                    tempo: tempo.clone(),
                    origin_beat_raw,
                    local_start_raw,
                })
            }
        }
        Ok(())
    }
}
