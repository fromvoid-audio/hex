//! Immutable visible Console track page and its following insertion anchor.
//!
//! The canonical indices and IDs are captured together from one project and
//! folded projection. A proposed track occupies a display column without
//! a canonical ID. Callers fence the page with their source revision and
//! layout frame; this projection does not own or mutate project state.
use crate::track_proposals::{Anchor, ProposalRows, TrackProposal};
use hex_project::{Project, TrackId};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    pub index: usize,
    pub id: TrackId,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TrackPage {
    pub range: Range<usize>,
    pub columns: Vec<Column>,
    pub next: Option<TrackId>,
    proposals: ProposalRows,
}

pub fn visible_track_range(track_count: usize, start: usize, visible_count: usize) -> Range<usize> {
    let count = visible_count.max(1);
    let start = start.min(track_count.saturating_sub(count));
    start..start.saturating_add(count).min(track_count)
}

impl TrackPage {
    pub fn capture(project: &Project, visible: &[usize], start: usize, count: usize) -> Self {
        Self::capture_with_proposals(project, visible, start, count, &[])
    }

    pub fn capture_with_proposals(
        project: &Project,
        visible: &[usize],
        start: usize,
        count: usize,
        anchors: &[Anchor],
    ) -> Self {
        let base = visible_track_range(visible.len(), start, count);
        let rows = ProposalRows::project(project.tracks(), visible, anchors, |ordinal| {
            (base.start..=base.end).contains(&ordinal)
        });
        let Some(latest) = rows.latest_row() else {
            return Self::from_range(project, visible, base, rows);
        };
        let count = count.max(1);
        // Browse in canonical ordinals and spend display columns on proposals
        // that land on this page. If they fill the viewport, reveal the newest.
        let start = base.start.max((latest + 1).saturating_sub(count));
        let end = (start + count).min(rows.track_row(visible.len()));
        let canonical = rows.tracks_before(start)..rows.tracks_before(end);
        Self::from_range(project, visible, canonical, rows.within(start..end))
    }

    fn from_range(
        project: &Project,
        visible: &[usize],
        range: Range<usize>,
        proposals: ProposalRows,
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
            proposals,
        }
    }

    pub fn proposal_index(&self, proposal: TrackProposal) -> Option<usize> {
        self.proposals.row_of(proposal)
    }

    pub fn proposal_at(&self, column: usize) -> Option<TrackProposal> {
        self.proposals.at(column)
    }

    pub fn displayed_columns(&self) -> impl Iterator<Item = Option<&Column>> {
        (0..self.display_index(self.columns.len())).map(|index| {
            self.track_index(index)
                .and_then(|index| self.columns.get(index))
        })
    }

    pub fn display_index(&self, track_index: usize) -> usize {
        self.proposals.track_row(track_index)
    }

    pub fn track_index(&self, display_index: usize) -> Option<usize> {
        self.proposals.track_ordinal(display_index)
    }
}

#[cfg(test)]
mod tests {
    use super::{TrackPage, TrackProposal, visible_track_range};
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
        let proposal = TrackProposal::Empty(0);
        let show = |start, count, before| {
            TrackPage::capture_with_proposals(
                project,
                &visible,
                start,
                count,
                &[(before, proposal)],
            )
        };
        let id = |index: usize| project.tracks()[index].id();
        let folded = show(0, 2, Some(id(1)));
        assert_eq!(folded.range, 0..1);
        assert_eq!(folded.proposal_index(proposal), Some(1));
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

        assert_eq!(show(0, 1, None).proposal_index(proposal), None);
        assert_eq!(show(2, 1, None).proposal_index(proposal), Some(0));
        assert!(show(2, 1, None).columns.is_empty());
    }

    #[test]
    fn shared_anchor_preserves_admission_order_and_narrow_page_reveals_the_latest_birth() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        for n in 0..3 {
            editor
                .execute(Command::AddTrack {
                    name: format!("track {n}"),
                })
                .unwrap();
        }
        let project = editor.project();
        let before = Some(project.tracks()[1].id());
        let one = TrackProposal::Empty(1);
        let device = TrackProposal::Device { before_empty: 2 };
        let two = TrackProposal::Empty(2);
        let anchors = [(before, device), (before, one), (before, two)];
        let show =
            |width| TrackPage::capture_with_proposals(project, &[0, 1, 2], 0, width, &anchors);

        let wide = show(6);
        assert_eq!(wide.proposal_at(1), Some(one));
        assert_eq!(wide.proposal_at(2), Some(device));
        assert_eq!(wide.proposal_at(3), Some(two));
        assert_eq!(wide.track_index(4), Some(1));
        assert_eq!(wide.display_index(1), 4);

        let narrow = show(2);
        assert_eq!(narrow.proposal_at(0), Some(device));
        assert_eq!(narrow.proposal_at(1), Some(two));
        assert_eq!(narrow.range, 1..1);
        assert_eq!(narrow.next, before);
    }
}
