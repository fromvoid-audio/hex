//! Prepared inline identity table for bounded live project observation.
use crate::Track;

/// Immutable, inline owner index prepared before callback publication. The
/// callback merges a sorted selection with this table in at most256 comparisons.
/// No callback sorting, project scan, allocation or independent heap ownership.
#[derive(Clone, Debug)]
pub(crate) struct ProjectTrackIds {
    ids: [u64; hex_project::MAX_TRACKS],
    count: usize,
    valid: bool,
}
impl ProjectTrackIds {
    pub(crate) fn prepare(tracks: &[Track]) -> Self {
        Self::from_ids(
            tracks
                .iter()
                .filter_map(|t| t.canonical_track_id().map(|id| id.value())),
        )
    }
    fn from_ids(ids: impl Iterator<Item = u64>) -> Self {
        let mut table = Self {
            ids: [0; hex_project::MAX_TRACKS],
            count: 0,
            valid: true,
        };
        for id in ids {
            if table.count == table.ids.len() || id == 0 {
                table.valid = false;
                return table;
            }
            table.ids[table.count] = id;
            table.count += 1;
        }
        table.ids[..table.count].sort_unstable();
        table.valid = table.ids[..table.count].windows(2).all(|p| p[0] < p[1]);
        table
    }
    pub(crate) fn contains(&self, tracks: &[u64], mut compared: impl FnMut()) -> bool {
        if !self.valid || tracks.len() > self.count {
            return false;
        }
        let mut selected = 0;
        for owner in &self.ids[..self.count] {
            let Some(id) = tracks.get(selected) else {
                return true;
            };
            compared();
            if owner == id {
                if selected > 0 && tracks[selected - 1] >= *id {
                    return false;
                }
                selected += 1;
            } else if owner > id {
                return false;
            }
        }
        selected == tracks.len()
    }
}
#[cfg(test)]
mod project_owner_index_tests {
    use super::*;
    #[test]
    fn project_owner_validation_is_linear_at_maximum_and_refuses_stale_or_unsorted_ids() {
        let index = ProjectTrackIds::from_ids((1..=256).rev());
        for ids in [vec![], vec![1], vec![256], (1..=256).collect()] {
            let mut comparisons = 0;
            assert!(index.contains(&ids, || comparisons += 1));
            assert!(comparisons <= 256);
            if ids.is_empty() {
                assert_eq!(comparisons, 0);
            }
            if ids.len() == 256 {
                assert_eq!(comparisons, 256, "linear, never256 squared");
            }
        }
        for ids in [&[0][..], &[257], &[1, 1], &[2, 1], &[1, 257]] {
            assert!(!index.contains(ids, || {}));
        }
        let replaced = ProjectTrackIds::from_ids((2..=257).rev());
        assert!(!replaced.contains(&[1], || {}));
        assert!(replaced.contains(&[257], || {}));
        assert!(!ProjectTrackIds::from_ids(1..=257).contains(&[], || {}));
    }
}
