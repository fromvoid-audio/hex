use crate::{Clip, Invalid};

/// Retain only a positive placed prefix, keeping the original fade/processing
/// domain and media available. Unlike an edge extension or a two-sided split,
/// this also permits ending in a one-shot's existing silent tail; no right-hand
/// fragment or new identity is created. Work remains on the control side.
pub(crate) fn preview_prefix_crop_with_tempo(
    original: &Clip,
    end: u64,
    tempo: &crate::TempoMap,
) -> Result<Clip, Invalid> {
    let original_end = original
        .start()
        .checked_add(original.duration())
        .ok_or(Invalid("audio prefix end overflow"))?;
    if end <= original.start() || end > original_end {
        return Err(Invalid(
            "audio prefix must retain a positive contained interval",
        ));
    }
    if end == original_end {
        return Ok(original.clone());
    }
    if original.audio_edit().is_some() {
        return crate::audio_edit::trim_view(original, crate::TrimEdge::Right, end, tempo);
    }
    let mut prefix = original.clone();
    prefix.duration = end - original.start();
    if !prefix.looping {
        prefix.source_length = prefix.source_length.min(prefix.duration);
    }
    // Retain the fade's original domain/offset, as for the left side of a split:
    // cropping must not rescale the audible prefix or move its fade-out earlier.
    Ok(prefix)
}

/// Preview an audio fragment moved to another timeline position without
/// changing its source mapping or fade domain. The fragment must be wholly
/// contained by the original clip and the destination end must be representable.
pub fn preview_clip_fragment(
    original: &Clip,
    start: u64,
    end: u64,
    destination_start: u64,
) -> Result<Clip, Invalid> {
    let original_end = original
        .start()
        .checked_add(original.duration())
        .ok_or(Invalid("timeline end overflow"))?;
    if start >= end || start < original.start() || end > original_end {
        return Err(Invalid("audio fragment must be inside the original clip"));
    }
    destination_start
        .checked_add(end - start)
        .ok_or(Invalid("fragment destination end overflow"))?;

    let mut fragment = original.clone();
    if start > fragment.start() {
        fragment = crate::edit::split_clip_at(&fragment, start, fragment.id())?.1;
    }
    if end < original_end {
        fragment = crate::edit::split_clip_at(&fragment, end, fragment.id())?.0;
    }
    fragment.start = destination_start;
    Ok(fragment)
}

/// Tempo-aware preview retaining full transformed analysis and fade context.
pub fn preview_clip_fragment_with_tempo(
    original: &Clip,
    start: u64,
    end: u64,
    destination_start: u64,
    tempo: &crate::TempoMap,
) -> Result<Clip, Invalid> {
    if original.audio_edit().is_none() {
        return preview_clip_fragment(original, start, end, destination_start);
    }
    let original_end = original
        .start()
        .checked_add(original.duration())
        .ok_or(Invalid("fragment end overflow"))?;
    if start >= end || start < original.start() || end > original_end {
        return Err(Invalid("audio fragment must be inside the original clip"));
    }
    let mut fragment = original.clone();
    if start > fragment.start() {
        fragment = crate::edit::split_clip_at_tempo(&fragment, start, fragment.id(), tempo)?.1;
    }
    if end < original_end {
        fragment = crate::edit::split_clip_at_tempo(&fragment, end, fragment.id(), tempo)?.0;
    }
    crate::audio_edit::move_clip(&mut fragment, destination_start, tempo)?;
    Ok(fragment)
}
