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
    fn compiled_project_moves_original_context_without_discarded_default_birth() {
        use crate::{
            Audio, Clip, Project, owned_shared::OwnedShared,
            shared_analytic::SharedAnalyticContextTable,
        };
        let contexts = OwnedShared::new_owned(SharedAnalyticContextTable::default());
        let pointer = std::sync::Arc::as_ptr(contexts.arc());
        let mut tracks = Vec::with_capacity(5);
        tracks.push(
            Track::new(
                vec![
                    Clip::new(
                        Audio::new(48_000, vec![[0.25, -0.5]; 4]).unwrap(),
                        0,
                        4,
                        false,
                    )
                    .unwrap(),
                ],
                0.5,
                false,
            )
            .unwrap(),
        );
        let track_pointer = tracks.as_ptr();
        let track_capacity = tracks.capacity();
        let returns = Vec::with_capacity(7);
        let return_pointer = returns.as_ptr();
        let return_capacity = returns.capacity();
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        let project =
            Project::with_compiled_tracks(48_000, tracks, returns, None, 0.5, contexts).unwrap();
        assert_eq!(
            crate::owned_shared::birth_count(),
            0,
            "field assembly must not construct then discard a default owner"
        );
        assert_eq!(crate::owned_shared::clone_count(), 0);
        assert_eq!(
            std::sync::Arc::as_ptr(project.shared_contexts.arc()),
            pointer
        );
        assert_eq!(
            std::sync::Arc::strong_count(project.shared_contexts.arc()),
            1
        );
        assert_eq!(project.tracks.as_ptr(), track_pointer);
        assert_eq!(project.tracks.capacity(), track_capacity);
        assert_eq!(project.return_buses.as_ptr(), return_pointer);
        assert_eq!(project.return_buses.capacity(), return_capacity);
        let mut renderer = crate::Renderer::new(&project);
        renderer.play();
        let mut pcm = [[0.0; 2]; 6];
        renderer.render(&mut pcm).unwrap();
        assert_eq!(
            pcm,
            [
                [0.0625, -0.125],
                [0.0625, -0.125],
                [0.0625, -0.125],
                [0.0625, -0.125],
                [0.0; 2],
                [0.0; 2]
            ]
        );
    }
    #[test]
    fn compiled_project_assembly_preserves_validation_and_unknown_context_origin() {
        use crate::{
            Audio, Clip, Project, owned_shared::OwnedShared,
            shared_analytic::SharedAnalyticContextTable,
        };
        crate::owned_shared::reset_birth_count();
        assert!(Project::with_master_gain(0, vec![], None, 1.0).is_err());
        assert!(Project::with_master_gain(48_000, vec![], None, f32::NAN).is_err());
        let track = Track::new(
            vec![Clip::new(Audio::new(44_100, vec![[0.25; 2]; 4]).unwrap(), 0, 4, false).unwrap()],
            1.0,
            false,
        )
        .unwrap();
        assert!(Project::with_master_gain(48_000, vec![track], None, 1.0).is_err());
        assert_eq!(crate::owned_shared::birth_count(), 0);
        let unknown =
            OwnedShared::untracked(std::sync::Arc::new(SharedAnalyticContextTable::default()));
        let pointer = std::sync::Arc::as_ptr(unknown.arc());
        let project =
            Project::with_compiled_tracks(48_000, vec![], vec![], None, 1.0, unknown).unwrap();
        assert_eq!(
            std::sync::Arc::as_ptr(project.shared_contexts.arc()),
            pointer
        );
        assert!(project.shared_contexts.parts().is_none());
        assert_eq!(crate::owned_shared::birth_count(), 0);
        let legacy = Project::with_master_gain(48_000, vec![], None, 1.0).unwrap();
        assert_eq!(crate::owned_shared::birth_count(), 1);
        assert_eq!(
            std::sync::Arc::strong_count(legacy.shared_contexts.arc()),
            1
        );
    }
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
