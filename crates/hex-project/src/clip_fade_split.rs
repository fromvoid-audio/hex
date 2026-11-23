//! Arrangement cuts that keep every clip fade anchored to each part's corners.
//!
//! Splitting any audio clip (one-shot, looped, warped, stretched, musical, or
//! outer-fade) produces left and right parts whose fades are anchored strictly
//! to each part's own boundaries (`offset == 0`, `span == part.duration()`,
//! `in_start == 0`, `out_end_offset == 0`). Each part preserves its uncut outer
//! ramp (clamped to the part's duration if the cut fell inside that ramp), and
//! the newly created cut edge starts neutral with immediately visible,
//! draggable corner handles.
use crate::{Clip, ClipId, Invalid, TempoMap, edit::split_clip_at_tempo};

pub(crate) fn split(
    original: &Clip,
    frame: u64,
    right_id: ClipId,
    tempo: &TempoMap,
) -> Result<(Clip, Clip), Invalid> {
    split_clip_at_tempo(original, frame, right_id, tempo)
}

#[cfg(test)]
#[path = "clip_fade_split_tests.rs"]
mod tests;
