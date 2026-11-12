//! Selection-fitted fades and source-handle crossfades retain canonical clip
//! identities and source alignment. The model owns eligibility and atomic edits;
//! Session owns preparation and history, and the existing renderer sums voices.

use crate::{
    AudioEventEdit, AudioEventFade, Clip, ClipFade, ClipId, Created, Invalid, Project, TempoMap,
    TimeRange, TrimEdge,
};
use std::sync::Arc;

fn side(clip_start: u64, clip_end: u64, start: u64, end: u64) -> Result<bool, Invalid> {
    match (start == clip_start, end == clip_end) {
        (true, false) => Ok(true),
        (false, true) => Ok(false),
        (true, true) => Err(Invalid(
            "select one clip edge or a join; selecting both edges is ambiguous",
        )),
        _ => Err(Invalid("fade selection must include a clip edge")),
    }
}

fn fitted(shape: ClipFade, first: u64, end: u64, incoming: bool) -> Result<ClipFade, Invalid> {
    let length = end
        .checked_sub(first)
        .filter(|length| *length > 0)
        .ok_or(Invalid("fade selection has no source interval"))?;
    let span = shape.span().max(end);
    let (fade_in, fade_out, in_start, out_end, in_curve, out_curve, in_position, out_position) =
        if incoming {
            (
                length,
                shape.fade_out(),
                first,
                shape.span() - shape.out_end_offset(),
                0,
                shape.out_curve(),
                500,
                shape.out_position(),
            )
        } else {
            (
                shape.fade_in(),
                length,
                shape.in_start(),
                end,
                shape.in_curve(),
                0,
                shape.in_position(),
                500,
            )
        };
    let mut laws = shape.curve_laws();
    laws[usize::from(!incoming)] = ClipFade::DEFAULT_S_CURVE;
    Ok(ClipFade::new(span, shape.offset(), fade_in, fade_out)?
        .with_shape(
            in_start,
            span.checked_sub(out_end)
                .ok_or(Invalid("fade endpoint exceeds its domain"))?,
            in_curve,
            out_curve,
        )?
        .with_curve_positions(in_position, out_position)?
        .with_curve_laws(laws))
}
fn edge(
    clip: &Clip,
    first: u64,
    end: u64,
    incoming: bool,
    tempo: &TempoMap,
) -> Result<Clip, Invalid> {
    let mut next = clip.clone();
    if let Some(edit) = clip.audio_edit() {
        if edit.placement.is_some() || edit.content_view.is_some() {
            return Err(Invalid(
                "selection fades for directed outer audio views are not supported yet",
            ));
        }
        let (_, _, view, _) = edit.projected_domain(tempo)?;
        let projection = edit.projected_fade(tempo)?;
        let at = |local: u64| -> Result<u64, Invalid> {
            projection
                .domain_position_at_frame(
                    view.checked_add(local)
                        .ok_or(Invalid("fade frame overflow"))?,
                )?
                .ok_or(Invalid(
                    "fade selection precedes its retained reference domain",
                ))
        };
        let shape = fitted(edit.domain.fade, at(first)?, at(end)?, incoming)?;
        let edit = Arc::make_mut(next.edit.as_mut().ok_or(Invalid("audio domain missing"))?);
        Arc::make_mut(&mut edit.domain).fade = shape;
    } else {
        next.fade = fitted(
            clip.fade(),
            clip.fade()
                .offset()
                .checked_add(first)
                .ok_or(Invalid("fade frame overflow"))?,
            clip.fade()
                .offset()
                .checked_add(end)
                .ok_or(Invalid("fade frame overflow"))?,
            incoming,
        )?;
    }
    Ok(next)
}
fn clip_end(clip: &Clip) -> Result<u64, Invalid> {
    clip.start()
        .checked_add(clip.duration())
        .ok_or(Invalid("clip timeline end overflow"))
}
fn crossfade(
    project: &Project,
    left: &Clip,
    right: &Clip,
    start: u64,
    end: u64,
) -> Result<[Clip; 2], Invalid> {
    if left.audio_edit().is_some() || right.audio_edit().is_some() {
        return Err(Invalid(
            "crossfades currently require ordinary audio clips; retained or warped joins are unsupported",
        ));
    }
    if left.start() >= right.start()
        || clip_end(left)? < right.start()
        || clip_end(left)? >= clip_end(right)?
        || start <= left.start()
        || end >= clip_end(right)?
    {
        return Err(Invalid(
            "crossfade selection must cross the join between two adjacent audio clips",
        ));
    }
    let frames = |clip: &Clip| {
        project
            .asset(clip.asset())
            .map(|asset| asset.playback_frames())
            .ok_or(Invalid("crossfade source is missing"))
    };
    let outgoing = left.preview_edge_trim_with_tempo(
        TrimEdge::Right,
        end,
        frames(left)?,
        project.tempo_map(),
    )?;
    let incoming = right.preview_edge_trim_with_tempo(
        TrimEdge::Left,
        start,
        frames(right)?,
        project.tempo_map(),
    )?;
    if clip_end(&outgoing)? != end
        || incoming.start() != start
        || clip_end(&incoming)? != clip_end(right)?
    {
        return Err(Invalid(
            "crossfade needs more source material at the selected join",
        ));
    }
    let outgoing = edge(
        &outgoing,
        start - outgoing.start(),
        end - outgoing.start(),
        false,
        project.tempo_map(),
    )?;
    let incoming = edge(&incoming, 0, end - start, true, project.tempo_map())?;
    Ok([outgoing, incoming])
}

pub(crate) fn arrangement(project: &mut Project, range: &TimeRange) -> Result<Created, Invalid> {
    range.validate(project)?;
    let mut candidate = project.clone();
    let mut count = 0;
    for id in range.tracks() {
        let track = project
            .track(*id)
            .ok_or(Invalid("fade track no longer exists"))?;
        for clip in track.midi_clips() {
            let start = project.tempo_map().beat_to_frame(clip.start())?;
            let end = project
                .tempo_map()
                .beat_to_frame(crate::BeatTime::from_raw(
                    clip.start()
                        .raw()
                        .checked_add(clip.length().raw())
                        .ok_or(Invalid("midi end overflow"))?,
                ))?;
            if start < range.end() && range.start() < end {
                return Err(Invalid("fade selection includes midi material"));
            }
        }
        let mut hits = [None, None];
        let mut hit_count = 0;
        for clip in track.clips() {
            if clip.start() < range.end() && range.start() < clip_end(clip)? {
                let slot = hits
                    .get_mut(hit_count)
                    .ok_or(Invalid("fade selection spans more than two audio clips"))?;
                *slot = Some(clip);
                hit_count += 1;
            }
        }
        let [first, second] = hits;
        match (first, second) {
            (Some(clip), None) => {
                let end = clip_end(clip)?;
                let a = range.start().max(clip.start());
                let b = range.end().min(end);
                let incoming = side(clip.start(), end, a, b)?;
                let next = edge(
                    clip,
                    a - clip.start(),
                    b - clip.start(),
                    incoming,
                    project.tempo_map(),
                )?;
                crate::audio_edit::validate_clip(project, &next)?;
                *candidate.clip_mut(clip.id())? = next;
                count += 1;
            }
            (Some(a), Some(b)) => {
                let (left, right) = if a.start() < b.start() {
                    (a, b)
                } else {
                    (b, a)
                };
                for clip in crossfade(project, left, right, range.start(), range.end())? {
                    crate::audio_edit::validate_clip(project, &clip)?;
                    let id = clip.id();
                    *candidate.clip_mut(id)? = clip;
                }
                count += 2;
            }
            _ => (),
        }
    }
    if count == 0 {
        return Err(Invalid("fade selection contains no audio clips"));
    }
    candidate.validate_changed_arrangement_occupancy(project)?;
    *project = candidate;
    Ok(Created::None)
}

pub(crate) fn event(
    project: &mut Project,
    clip: ClipId,
    start: u64,
    end: u64,
) -> Result<Created, Invalid> {
    let source = project
        .clip(clip)
        .ok_or(Invalid("audio clip no longer exists"))?;
    let root = source.audio_event_root();
    if start >= end || end > root.domain.clock.length() {
        return Err(Invalid("event fade selection is outside the retained root"));
    }
    let events = source.editable_audio_events()?;
    let mut hits = events
        .events
        .iter()
        .filter(|e| e.start < end && start < e.end);
    let target = hits
        .next()
        .ok_or(Invalid("fade selection contains no audio event"))?;
    if hits.next().is_some() {
        return Err(Invalid(
            "event crossfades are unsupported; select one event edge or use adjacent arranger clips",
        ));
    }
    let a = start.max(target.start);
    let b = end.min(target.end);
    let incoming = side(target.start, target.end, a, b)?;
    let original = target.fade.unwrap_or(AudioEventFade::new(
        target.source_start,
        ClipFade::neutral(target.length())?,
    )?);
    let first = target
        .source_start
        .checked_add(a - target.start)
        .and_then(|at| at.checked_sub(original.source_start()))
        .ok_or(Invalid("event fade source overflow"))?;
    let last = first
        .checked_add(b - a)
        .ok_or(Invalid("event fade source overflow"))?;
    let fade = AudioEventFade::new(
        original.source_start(),
        fitted(original.shape(), first, last, incoming)?,
    )?;
    crate::audio_events::apply(
        project,
        clip,
        AudioEventEdit::SetFade {
            event: target.id,
            fade: Some(fade),
        },
    )?;
    Ok(Created::None)
}

#[cfg(test)]
#[path = "selection_fades_tests.rs"]
mod tests;
