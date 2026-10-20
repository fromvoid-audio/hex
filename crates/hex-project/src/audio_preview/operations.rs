//! Shared trim, lower-handle and stretch planning.
use super::plan::{ProjectionPlan, default_warp_markers};
use super::*;
impl Plan {
    pub fn calculate(
        clip: &Clip,
        operation: AudioClipPreviewOperation,
        frame: u64,
        asset_frames: u64,
        tempo: &TempoMap,
    ) -> Result<Self, Invalid> {
        match operation {
            AudioClipPreviewOperation::Trim(edge) => {
                Self::trim(clip, edge, frame, asset_frames, tempo)
            }
            AudioClipPreviewOperation::FadeTrim(edge) => {
                Self::fade_trim(clip, edge, frame, asset_frames, tempo)
            }
            AudioClipPreviewOperation::Stretch(edge) => {
                Self::stretch(clip, edge, frame, asset_frames, tempo)
            }
        }
    }
    fn trim(
        clip: &Clip,
        edge: TrimEdge,
        frame: u64,
        asset_frames: u64,
        tempo: &TempoMap,
    ) -> Result<Self, Invalid> {
        let Some(original) = clip.audio_edit() else {
            return Ok(Self {
                facts: plain::plain_trim(clip, edge, frame, asset_frames)?,
                edit: None,
            });
        };
        let mut plan = Self::original(clip);
        let mut edit = EditPlan::original(original);
        if let Some(mut placement) = edit.placement {
            AudioViewProjection::new(clip, tempo)?;
            let old_start = edit.musical_start.map_or(clip.start, BeatTime::raw);
            let position = if edit.musical_start.is_some() {
                tempo.frame_to_beat(frame)?.raw()
            } else {
                frame
            };
            let old_end = old_start
                .checked_add(placement.outer_length.length())
                .ok_or(Invalid("audio outer end overflow"))?;
            let (new_start, new_length) = match edge {
                TrimEdge::Right => (
                    old_start,
                    position
                        .checked_sub(old_start)
                        .filter(|n| *n > 0)
                        .ok_or(Invalid("audio trim must retain positive length"))?,
                ),
                TrimEdge::Left => {
                    let length = old_end
                        .checked_sub(position)
                        .filter(|n| *n > 0)
                        .ok_or(Invalid("audio trim must retain positive length"))?;
                    let phase = i128::from(placement.cycle_phase) + i128::from(position)
                        - i128::from(old_start);
                    let cycle = edit.view_end - edit.view_start;
                    placement.cycle_phase = if placement.mode == AudioViewMode::Repeat {
                        phase.rem_euclid(i128::from(cycle)) as u64
                    } else {
                        u64::try_from(phase)
                            .map_err(|_| Invalid("once trim consumed offset exceeds range"))?
                    };
                    (position, length)
                }
            };
            placement.outer_length = match edit.clock {
                AudioDomainClock::Frames(_) => AudioDomainClock::Frames(new_length),
                AudioDomainClock::Beats(_) => {
                    AudioDomainClock::Beats(BeatTime::from_raw(new_length))
                }
            };
            edit.placement = Some(placement);
            if edit.musical_start.is_some() {
                edit.musical_start = Some(BeatTime::from_raw(new_start));
            } else {
                plan.facts.start = new_start;
            }
        } else {
            let (_, _, start_offset, _) = edit.projected_domain(tempo)?;
            let origin_frame = i128::from(clip.start) - i128::from(start_offset);
            let coordinate = match edit.clock {
                AudioDomainClock::Frames(_) => u64::try_from(i128::from(frame) - origin_frame)
                    .map_err(|_| Invalid("trim precedes retained audio domain"))?,
                AudioDomainClock::Beats(_) => u64::try_from(
                    i128::from(tempo.frame_to_beat(frame)?.raw()) - edit.projection_origin()?,
                )
                .map_err(|_| Invalid("trim precedes retained audio domain"))?,
            };
            if coordinate > edit.clock.length() {
                return Err(Invalid(
                    "trim exceeds retained audio domain; edit source region or length to create a new root",
                ));
            }
            match edge {
                TrimEdge::Left if coordinate < edit.view_end => {
                    edit.view_start = coordinate;
                    if edit.musical_start.is_some() {
                        edit.musical_start = Some(tempo.frame_to_beat(frame)?);
                    }
                    plan.facts.start = frame;
                }
                TrimEdge::Right if coordinate > edit.view_start => edit.view_end = coordinate,
                _ => return Err(Invalid("audio crop must retain a positive interval")),
            }
        }
        plan.edit = Some(edit);
        plan.project(clip, tempo)?;
        Ok(plan)
    }
    fn fade_trim(
        clip: &Clip,
        edge: TrimEdge,
        frame: u64,
        asset_frames: u64,
        tempo: &TempoMap,
    ) -> Result<Self, Invalid> {
        if clip
            .source_offset
            .checked_add(clip.source_length)
            .is_none_or(|end| end > asset_frames)
        {
            return Err(Invalid("clip source interval exceeds media"));
        }
        let end = clip
            .start
            .checked_add(clip.duration)
            .ok_or(Invalid("timeline end overflow"))?;
        let original_edge = match edge {
            TrimEdge::Left => clip.start,
            TrimEdge::Right => end,
        };
        let fade = clip.retained_fade().reanchor(clip.duration)?;
        let endpoint_aligned = match edge {
            TrimEdge::Left => fade.in_start() == 0,
            TrimEdge::Right => fade.out_end_offset() == 0,
        };
        if frame == original_edge && endpoint_aligned {
            return Ok(Self::original(clip));
        }
        let in_top = clip.start + fade.in_start() + fade.fade_in().saturating_sub(1);
        let out_top = end - fade.out_end_offset() - fade.fade_out().max(1);
        let frame = match edge {
            TrimEdge::Left if fade.fade_in() > 0 => frame.min(in_top),
            TrimEdge::Right if fade.fade_out() > 0 => frame.max(out_top + 1),
            _ => frame,
        };
        let frame = if let Some(edit) = clip.audio_edit().filter(|edit| edit.placement.is_none()) {
            let (_, full, start, _) = edit.projected_domain(tempo)?;
            let origin = i128::from(clip.start) - i128::from(start);
            let first = origin.max(0) as u64;
            let last = (origin + i128::from(full)).clamp(0, i128::from(u64::MAX)) as u64;
            match edge {
                TrimEdge::Left => frame.clamp(first, end - 1),
                TrimEdge::Right => frame.clamp(clip.start + 1, last),
            }
        } else {
            frame
        };
        let mut plan = Self::trim(clip, edge, frame, asset_frames, tempo)?;
        if plan.facts.start == clip.start
            && plan.facts.duration == clip.duration
            && endpoint_aligned
        {
            return Ok(Self::original(clip));
        }
        let fitted = fade.reanchor(plan.facts.duration)?;
        let (fade_in, fade_out, in_start, out_end_offset) = match edge {
            TrimEdge::Left => (
                if fade.fade_in() == 0 {
                    0
                } else {
                    in_top
                        .saturating_sub(plan.facts.start)
                        .saturating_add(1)
                        .min(plan.facts.duration)
                },
                fitted.fade_out(),
                0,
                fitted.out_end_offset(),
            ),
            TrimEdge::Right => (
                fitted.fade_in(),
                if fade.fade_out() == 0 {
                    0
                } else {
                    (plan.facts.start + plan.facts.duration)
                        .saturating_sub(out_top)
                        .min(plan.facts.duration)
                },
                fitted.in_start(),
                0,
            ),
        };
        let next = ClipFade::new(plan.facts.duration, 0, fade_in, fade_out)?.with_shape(
            in_start,
            out_end_offset,
            fade.in_curve(),
            fade.out_curve(),
        )?;
        if let Some(edit) = &mut plan.edit {
            edit.fade = next;
            edit.fade_start = edit.view_start;
            edit.projection = if edit.musical_start.is_some() {
                ProjectionPlan::Current {
                    origin_beat_raw: edit.projection_origin()? + i128::from(edit.view_start),
                    local_start_raw: edit.view_start,
                }
            } else {
                ProjectionPlan::None
            };
            plan.project(clip, tempo)?;
            validation::validate(plan, clip, tempo, asset_frames)?;
        } else {
            plan.facts.fade = next;
            plan.facts.retained_fade = next;
        }
        Ok(plan)
    }
    fn stretch(
        clip: &Clip,
        edge: TrimEdge,
        frame: u64,
        asset_frames: u64,
        tempo: &TempoMap,
    ) -> Result<Self, Invalid> {
        let end = clip
            .start
            .checked_add(clip.duration)
            .ok_or(Invalid("clip end overflow"))?;
        if (edge == TrimEdge::Left && frame >= end)
            || (edge == TrimEdge::Right && frame <= clip.start)
        {
            return Err(Invalid("stretch must retain a positive interval"));
        }
        if (edge == TrimEdge::Left && frame == clip.start)
            || (edge == TrimEdge::Right && frame == end)
        {
            return Ok(Self::original(clip));
        }
        let start = if edge == TrimEdge::Left {
            frame
        } else {
            clip.start
        };
        let end = if edge == TrimEdge::Right { frame } else { end };
        let mut plan = Self::original(clip);
        let mut edit = plan.edit.unwrap_or_else(|| EditPlan::capture(clip));
        edit.anchor(
            clip.start,
            clip.audio_edit().is_some_and(|e| e.domain.events.is_some()),
            tempo,
        )?;
        if clip.audio_edit().is_none_or(|e| e.domain.warp.is_none()) {
            edit.warp = WarpPlan::Default {
                mode: WarpMode::Beats {
                    preserve: crate::BeatsPreserve::Transients,
                    tail: crate::BeatsTail::Forward,
                    decay: 100,
                },
                markers: default_warp_markers(edit, plan.source(clip), tempo)?,
            };
        }
        edit.warp_enabled = true;
        let old_span = edit
            .placement
            .map_or(edit.view_end - edit.view_start, |p| p.outer_length.length());
        let new_span = tempo
            .frame_to_beat(end)?
            .raw()
            .checked_sub(tempo.frame_to_beat(start)?.raw())
            .ok_or(Invalid("stretch beat span invalid"))?;
        edit.retime(
            Scale {
                numerator: new_span,
                denominator: old_span,
            },
            start,
            clip.audio_edit(),
            tempo,
        )?;
        plan.edit = Some(edit);
        plan.facts.start = start;
        plan.project(clip, tempo)?;
        validation::validate(plan, clip, tempo, asset_frames)?;
        Ok(plan)
    }
}
