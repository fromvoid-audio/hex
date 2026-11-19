//! Start resolution for directed frame reads over an explicit repeating cycle.
//! The finite window and processing root stay retained; only existing base/current
//! phase metadata changes. Musical, Once and unmatched-cycle reads remain refused.
use super::*;

pub(super) fn preview(
    clip: &Clip,
    position: u64,
    mut content: AudioContentView,
) -> Result<AudioViewResize, Invalid> {
    let edit = clip
        .audio_edit()
        .ok_or(Invalid("audio content domain missing"))?;
    let mut placement = edit
        .placement
        .ok_or(Invalid("this audio view's play start is read-only"))?;
    let mut base = content
        .base_placement
        .ok_or(Invalid("this audio view's play start is read-only"))?;
    if !matches!(edit.domain.clock, AudioDomainClock::Frames(_))
        || !matches!(edit.timing, AudioClipTiming::Frames)
        || placement.mode != AudioViewMode::Repeat
        || base.mode != AudioViewMode::Repeat
        || (edit.view_start, edit.view_end) != (content.base_view_start, content.base_view_end)
    {
        return Err(Invalid("this audio view's play start is read-only"));
    }
    if position < content.base_view_start || position >= content.base_view_end {
        return Err(Invalid("play start must be inside the retained cycle"));
    }
    let cycle = content
        .base_view_end
        .checked_sub(content.base_view_start)
        .filter(|length| *length > 0)
        .ok_or(Invalid("audio directed cycle must be positive"))?;
    let read = content.window.read_position(0)? % cycle;
    let target = position
        .checked_sub(content.base_view_start)
        .ok_or(Invalid("audio directed start precedes cycle"))?;
    let phase = (u128::from(target) + u128::from(cycle) - u128::from(read)) % u128::from(cycle);
    let phase = u64::try_from(phase).map_err(|_| Invalid("audio directed start phase overflow"))?;
    base.cycle_phase = phase;
    placement.cycle_phase = phase;
    content.base_placement = Some(base);
    let frame_end = clip
        .start()
        .checked_add(clip.duration())
        .ok_or(Invalid("audio directed end overflow"))?;
    Ok(AudioViewResize {
        repeat_period: None,
        view_start: edit.view_start,
        view_end: edit.view_end,
        reference_start_raw: edit.reference_start_raw(),
        placement,
        frame_start: clip.start(),
        frame_end,
        play_start_position: Some(position),
        content_override: Some(content),
        outer_start: None,
    })
}
