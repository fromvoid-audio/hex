//! Pure section-focus track visibility and ancestor closure.
use hex_project::{Project, Track, TrackId};
use std::collections::BTreeSet;

pub(super) fn interval_visible(start: u64, end: u64, ranges: &[(u64, u64)]) -> bool {
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

/// Returns matching track IDs and their group ancestors for nonempty ranges.
pub(super) fn visible_tracks(
    project: &Project,
    ranges: &[(u64, u64)],
    new_track: Option<TrackId>,
) -> BTreeSet<TrackId> {
    let mut visible = BTreeSet::new();
    if !ranges.is_empty() {
        for track in project.tracks() {
            if new_track != Some(track.id()) && !track_intersects_sections(project, track, ranges) {
                continue;
            }
            visible.insert(track.id());
            let mut parent = track.parent_group();
            for _ in 0..hex_project::MAX_TRACKS {
                let Some(id) = parent else { break };
                if !visible.insert(id) {
                    break;
                }
                parent = project.track(id).and_then(|track| track.parent_group());
            }
        }
    }
    visible
}
