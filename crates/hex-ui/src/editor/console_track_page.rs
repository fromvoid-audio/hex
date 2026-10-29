//! Immutable visible Console track page and its following insertion anchor.
//!
//! The canonical indices and IDs are captured together from one project and
//! folded projection. Callers fence the page with their source revision and
//! layout frame; this projection does not own or mutate project state.
use hex_project::{Project, TrackId};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Column {
    pub index: usize,
    pub id: TrackId,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct TrackPage {
    pub range: Range<usize>,
    pub columns: Vec<Column>,
    pub next: Option<TrackId>,
}

pub(super) fn visible_track_range(
    track_count: usize,
    start: usize,
    visible_count: usize,
) -> Range<usize> {
    let count = visible_count.max(1);
    let start = start.min(track_count.saturating_sub(count));
    start..start.saturating_add(count).min(track_count)
}

impl TrackPage {
    pub fn capture(project: &Project, visible: &[usize], start: usize, count: usize) -> Self {
        let range = visible_track_range(visible.len(), start, count);
        let columns = visible
            .get(range.clone())
            .unwrap_or_default()
            .iter()
            .filter_map(|&index| {
                project.tracks().get(index).map(|track| Column {
                    index,
                    id: track.id(),
                })
            })
            .collect();
        let next = visible
            .get(range.end)
            .and_then(|&index| project.tracks().get(index))
            .map(|track| track.id());
        Self {
            range,
            columns,
            next,
        }
    }
}
