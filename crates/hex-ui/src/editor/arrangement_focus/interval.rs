//! Canonical track intersection with section frame ranges.
use hex_project::{Project, Track};

fn interval_visible(start: u64, end: u64, ranges: &[(u64, u64)]) -> bool {
    ranges.is_empty() || (start < end && ranges.iter().any(|r| start < r.1 && end > r.0))
}
/// Section intervals choose whole tracks; clip visibility never crops those tracks.
/// Empty intervals retain the ordinary unfiltered view.
pub(crate) fn track_intersects_sections(
    project: &Project,
    track: &Track,
    ranges: &[(u64, u64)],
) -> bool {
    ranges.is_empty()
        || track.clips().iter().any(|clip| {
            clip.start()
                .checked_add(clip.duration())
                .is_some_and(|end| interval_visible(clip.start(), end, ranges))
        })
        || track.midi_clips().iter().any(|clip| {
            let Some(end) = clip.start().raw().checked_add(clip.length().raw()) else {
                return false;
            };
            let map = project.tempo_map();
            match (
                map.beat_to_frame(clip.start()),
                map.beat_to_frame(hex_project::BeatTime::from_raw(end)),
            ) {
                (Ok(start), Ok(end)) => interval_visible(start, end, ranges),
                _ => false,
            }
        })
}
