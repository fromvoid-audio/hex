//! Immutable visible Console track page and its following insertion anchor.
//!
//! The canonical indices and IDs are captured together from one project and
//! folded projection. A proposed device track occupies a display column without
//! a canonical ID. Callers fence the page with their source revision and
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
    proposal: Option<usize>,
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
        Self::from_range(project, visible, range, None)
    }

    pub(super) fn with_device_proposal(
        self,
        project: &Project,
        visible: &[usize],
        count: usize,
        before: Option<TrackId>,
    ) -> Self {
        let index = match before {
            Some(id) => {
                let Some(index) = project.tracks().iter().position(|track| track.id() == id) else {
                    return self;
                };
                visible.partition_point(|&track_index| track_index < index)
            }
            None => visible.len(),
        };
        if index < self.range.start || index > self.range.end {
            return self;
        }
        let count = count.max(1);
        // A drop on a full page's tail keeps its column on screen. Once the
        // track commits, the same start paints the canonical replacement.
        let start = self.range.start + usize::from(index == self.range.start + count);
        let end = (start + count - 1).min(visible.len());
        Self::from_range(project, visible, start..end, Some(index - start))
    }

    fn from_range(
        project: &Project,
        visible: &[usize],
        range: Range<usize>,
        proposal: Option<usize>,
    ) -> Self {
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
            proposal,
        }
    }

    pub(super) fn proposal_index(&self) -> Option<usize> {
        self.proposal
    }

    pub(super) fn displayed_columns(&self) -> impl Iterator<Item = Option<&Column>> {
        (0..self.display_index(self.columns.len())).map(|index| {
            self.track_index(index)
                .and_then(|index| self.columns.get(index))
        })
    }

    pub(super) fn display_index(&self, track_index: usize) -> usize {
        track_index + usize::from(self.proposal.is_some_and(|index| index <= track_index))
    }

    pub(super) fn track_index(&self, display_index: usize) -> Option<usize> {
        match self.proposal {
            Some(index) if display_index == index => None,
            Some(index) if display_index > index => Some(display_index - 1),
            _ => Some(display_index),
        }
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{TrackPage, visible_track_range};
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

    #[test]
    fn track_paging_keeps_a_bounded_last_page_reachable() {
        assert_eq!(visible_track_range(256, 0, 8), 0..8);
        assert_eq!(visible_track_range(256, 248, 8), 248..256);
        assert_eq!(visible_track_range(256, usize::MAX, 8), 248..256);
        assert_eq!(visible_track_range(10, 8, 8), 2..10);
        assert_eq!(visible_track_range(0, 0, 8), 0..0);
    }

    #[test]
    fn full_console_page_keeps_a_proposal_at_the_tail_and_at_folded_boundaries() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        for n in 0..4 {
            editor
                .execute(Command::AddTrack {
                    name: format!("track {n}"),
                })
                .unwrap();
        }
        let project = editor.project();
        let visible = [0, 2, 3];
        let show = |start, count, before| {
            TrackPage::capture(project, &visible, start, count)
                .with_device_proposal(project, &visible, count, before)
        };
        let id = |index: usize| project.tracks()[index].id();
        let folded = show(0, 2, Some(id(1)));
        assert_eq!(folded.range, 0..1);
        assert_eq!(folded.proposal_index(), Some(1));
        assert_eq!(folded.next, Some(id(2)));

        let tail = show(1, 2, None);
        assert_eq!(tail.range, 2..3);
        assert_eq!(
            tail.displayed_columns()
                .map(|column| column.map(|column| column.id))
                .collect::<Vec<_>>(),
            [Some(id(3)), None]
        );
        assert_eq!(tail.track_index(0), Some(0));
        assert_eq!(tail.track_index(1), None);
        assert_eq!(tail.track_index(2), Some(1));
        assert_eq!(tail.display_index(1), 2);

        assert_eq!(show(0, 1, None).proposal_index(), None);
        assert_eq!(show(2, 1, None).proposal_index(), Some(0));
        assert!(show(2, 1, None).columns.is_empty());
    }
}
