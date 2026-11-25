//! View-only rows for admitted track births. Real track ordinals and IDs stay
//! canonical, while Arrangement rows and Session columns use the same mapping.
use hex_project::{Track, TrackId};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackProposal {
    Device { before_empty: u64 },
    Empty(u64),
}

impl TrackProposal {
    fn order(self) -> (u64, bool) {
        match self {
            Self::Device { before_empty } => (before_empty, false),
            Self::Empty(id) => (id, true),
        }
    }
}

pub type Anchor = (Option<TrackId>, TrackProposal);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProposalRows(Vec<(usize, TrackProposal)>);

pub fn anchor_ordinal(
    tracks: &[Track],
    visible: &[usize],
    before: Option<TrackId>,
) -> Option<usize> {
    match before {
        Some(id) => {
            let index = tracks.iter().position(|track| track.id() == id)?;
            Some(visible.partition_point(|&track_index| track_index < index))
        }
        None => Some(visible.len()),
    }
}

impl ProposalRows {
    pub fn project(
        tracks: &[Track],
        visible: &[usize],
        anchors: &[Anchor],
        in_range: impl Fn(usize) -> bool,
    ) -> Self {
        let mut rows: Vec<_> = anchors
            .iter()
            .filter_map(|&(anchor, proposal)| {
                let ordinal = anchor_ordinal(tracks, visible, anchor)?;
                in_range(ordinal).then_some((ordinal, proposal))
            })
            .collect();
        rows.sort_by_key(|&(ordinal, proposal)| (ordinal, proposal.order()));
        for (offset, (row, _)) in rows.iter_mut().enumerate() {
            *row += offset;
        }
        Self(rows)
    }

    pub fn at(&self, row: usize) -> Option<TrackProposal> {
        let index = self.0.binary_search_by_key(&row, |&(row, _)| row).ok()?;
        Some(self.0[index].1)
    }

    pub fn row_of(&self, proposal: TrackProposal) -> Option<usize> {
        self.0
            .iter()
            .find_map(|&(row, item)| (item == proposal).then_some(row))
    }

    pub fn latest_row(&self) -> Option<usize> {
        self.0
            .iter()
            .max_by_key(|&(_, item)| item.order())
            .map(|&(row, _)| row)
    }

    pub fn indices(&self) -> impl DoubleEndedIterator<Item = usize> + ExactSizeIterator + '_ {
        self.0.iter().map(|&(row, _)| row)
    }

    pub fn track_row(&self, ordinal: usize) -> usize {
        let mut row = ordinal;
        for position in self.indices() {
            if position > row {
                break;
            }
            row += 1;
        }
        row
    }

    pub fn tracks_before(&self, row: usize) -> usize {
        row - self.0.partition_point(|&(position, _)| position < row)
    }

    pub fn track_ordinal(&self, row: usize) -> Option<usize> {
        self.at(row).is_none().then(|| self.tracks_before(row))
    }

    pub fn within(self, range: Range<usize>) -> Self {
        Self(
            self.0
                .into_iter()
                .filter(|(row, _)| range.contains(row))
                .map(|(row, item)| (row - range.start, item))
                .collect(),
        )
    }
}
