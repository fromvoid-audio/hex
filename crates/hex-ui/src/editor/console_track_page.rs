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

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::TrackPage;
    use hex_project::{Command, Created, Editor, Project};

    #[test]
    fn page_captures_folded_columns_and_next_anchor_across_offsets() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut ids = Vec::new();
        for n in 0..5 {
            let Created::Track(id) = editor
                .execute(Command::AddTrack {
                    name: format!("track {n}"),
                })
                .unwrap()
            else {
                panic!("expected track")
            };
            ids.push(id);
        }
        let project = editor.project();
        let folded = [0, 2, 4];
        let page = TrackPage::capture(project, &folded, 1, 1);
        assert_eq!(page.range, 1..2);
        assert_eq!(page.columns.len(), 1);
        assert_eq!((page.columns[0].index, page.columns[0].id), (2, ids[2]));
        assert_eq!(page.next, Some(ids[4]));

        let last = TrackPage::capture(project, &folded, usize::MAX, 2);
        assert_eq!(last.range, 1..3);
        assert_eq!(
            last.columns
                .iter()
                .map(|column| column.id)
                .collect::<Vec<_>>(),
            [ids[2], ids[4]]
        );
        assert_eq!(last.next, None);
        let all = TrackPage::capture(project, &folded, 0, 10);
        assert_eq!(all.range, 0..3);
        assert_eq!(
            all.columns
                .iter()
                .map(|column| column.id)
                .collect::<Vec<_>>(),
            [ids[0], ids[2], ids[4]]
        );
        let empty = TrackPage::capture(project, &[], 0, 4);
        assert!(empty.columns.is_empty());
        assert_eq!(empty.next, None);
        let zero_width = TrackPage::capture(project, &folded, 0, 0);
        assert_eq!(zero_width.range, 0..1);
        assert_eq!(zero_width.next, Some(ids[2]));

        let old = page.columns[0];
        editor.execute(Command::RemoveTrack(ids[0])).unwrap();
        assert_eq!((old.index, old.id), (2, ids[2]));
        assert_ne!(editor.project().tracks()[old.index].id(), old.id);
    }
}
