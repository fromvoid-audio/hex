use crate::{Clip, Invalid};

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
