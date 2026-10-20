//! Ordinary source trim; the owning command and scoped preview share this law.
use super::*;
pub(super) fn plain_trim(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    asset_playback_frames: u64,
) -> Result<AudioClipPreview, Invalid> {
    if clip.edit.is_some() {
        return Err(Invalid("musical audio trim requires the project tempo map"));
    }
    if clip.duration == 0 || clip.source_length == 0 {
        return Err(Invalid("clip and source lengths must be positive"));
    }
    let timeline_end = clip
        .start
        .checked_add(clip.duration)
        .ok_or(Invalid("timeline end overflow"))?;
    let source_end = clip
        .source_offset
        .checked_add(clip.source_length)
        .ok_or(Invalid("source end overflow"))?;
    if source_end > asset_playback_frames {
        return Err(Invalid("clip source interval exceeds media"));
    }
    if (clip.looping && clip.source_phase >= clip.source_length)
        || (!clip.looping && clip.source_phase != 0)
    {
        return Err(Invalid("clip source phase is outside its source interval"));
    }

    let end = timeline_end;
    let (start, duration, source_offset, source_length, source_phase) = match (clip.looping, edge) {
        (true, TrimEdge::Left) => {
            let new_start = frame.clamp(0, end - 1);
            let phase = if new_start >= clip.start {
                (u128::from(clip.source_phase)
                    + u128::from(new_start - clip.start) % u128::from(clip.source_length))
                    % u128::from(clip.source_length)
            } else {
                let delta = u128::from(clip.start - new_start) % u128::from(clip.source_length);
                (u128::from(clip.source_phase) + u128::from(clip.source_length) - delta)
                    % u128::from(clip.source_length)
            } as u64;
            (
                new_start,
                end - new_start,
                clip.source_offset,
                clip.source_length,
                phase,
            )
        }
        (true, TrimEdge::Right) => {
            let new_end = frame.clamp(clip.start + 1, u64::MAX);
            (
                clip.start,
                new_end - clip.start,
                clip.source_offset,
                clip.source_length,
                clip.source_phase,
            )
        }
        (false, TrimEdge::Left) => {
            let min_start = clip.start.saturating_sub(clip.source_offset);
            let max_advance = clip
                .duration
                .saturating_sub(1)
                .min(clip.source_length.saturating_sub(1));
            let max_start = clip
                .start
                .checked_add(max_advance)
                .ok_or(Invalid("timeline end overflow"))?;
            let new_start = frame.clamp(min_start, max_start);
            if new_start >= clip.start {
                let advance = new_start - clip.start;
                (
                    new_start,
                    clip.duration - advance,
                    clip.source_offset + advance,
                    clip.source_length - advance,
                    0,
                )
            } else {
                let extension = clip.start - new_start;
                (
                    new_start,
                    clip.duration
                        .checked_add(extension)
                        .ok_or(Invalid("timeline end overflow"))?,
                    clip.source_offset - extension,
                    clip.source_length
                        .checked_add(extension)
                        .ok_or(Invalid("source length overflow"))?,
                    0,
                )
            }
        }
        (false, TrimEdge::Right) => {
            let source_end = clip
                .source_offset
                .checked_add(clip.source_length)
                .ok_or(Invalid("source end overflow"))?;
            let media_remaining = asset_playback_frames - source_end;
            let timeline_remaining = u64::MAX - end;
            let extension = media_remaining.min(timeline_remaining);
            let minimum_reduction = clip
                .duration
                .saturating_sub(1)
                .min(clip.source_length.saturating_sub(1));
            let minimum_end = end - minimum_reduction;
            let maximum_end = end + extension;
            let new_end = frame.clamp(minimum_end, maximum_end);
            if new_end >= end {
                let extension = new_end - end;
                (
                    clip.start,
                    clip.duration + extension,
                    clip.source_offset,
                    clip.source_length + extension,
                    0,
                )
            } else {
                let reduction = end - new_end;
                (
                    clip.start,
                    clip.duration - reduction,
                    clip.source_offset,
                    clip.source_length - reduction,
                    0,
                )
            }
        }
    };

    let changed = start != clip.start
        || duration != clip.duration
        || source_offset != clip.source_offset
        || source_length != clip.source_length
        || source_phase != clip.source_phase;
    let fade = if changed {
        clip.fade.reanchor(duration)?
    } else {
        clip.fade
    };
    Ok(AudioClipPreview {
        start,
        duration,
        source_offset,
        source_length,
        source_phase,
        looping: clip.looping,
        gain: clip.gain,
        fade,
        retained_fade: fade,
        unchanged: !changed,
    })
}
