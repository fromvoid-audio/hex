//! Revision-keyed view projection for group-header indentation.
use hex_project::{ElementColor, Project, TrackId};
use std::collections::BTreeMap;

/// Width of each visible ancestor-colour rail in a grouped track header.
pub(super) const RAIL_WIDTH: f32 = 4.0;

/// Cache canonical hierarchy depths by project identity and revision. The
/// project inventory is validated preorder, so the bounded ancestor stack
/// needs no whole-project scan for each visible header.
#[derive(Default)]
pub(super) struct HierarchyIndent {
    stamp: Option<(u64, u64)>,
    ancestors: BTreeMap<TrackId, Vec<(TrackId, ElementColor)>>,
}

impl HierarchyIndent {
    pub(super) fn reconcile(&mut self, generation: u64, revision: u64, project: &Project) {
        if self.stamp == Some((generation, revision)) {
            return;
        }
        self.ancestors.clear();
        let mut ancestors = Vec::with_capacity(hex_project::MAX_GROUP_DEPTH);
        for track in project.tracks() {
            while ancestors.last().map(|(id, _)| *id) != track.parent_group()
                && !ancestors.is_empty()
            {
                ancestors.pop();
            }
            self.ancestors.insert(track.id(), ancestors.clone());
            if track.is_group() && ancestors.len() < hex_project::MAX_GROUP_DEPTH {
                ancestors.push((track.id(), track.color()));
            }
        }
        self.stamp = Some((generation, revision));
    }
    pub(super) fn pixels(&self, track: TrackId) -> f32 {
        self.ancestors(track).len() as f32 * RAIL_WIDTH
    }
    pub(super) fn ancestors(&self, track: TrackId) -> &[(TrackId, ElementColor)] {
        self.ancestors.get(&track).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, TrackSelectionIds};

    #[test]
    fn nested_ancestors_follow_canonical_order_and_refresh_colours() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let mut ids = Vec::new();
        for name in ["a", "b", "c"] {
            let Created::Track(id) = editor
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track")
            };
            ids.push(id);
        }
        let Created::Track(inner) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&ids[..2]).unwrap(),
                name: "inner".into(),
            })
            .unwrap()
        else {
            panic!("inner group")
        };
        let Created::Track(outer) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[inner, ids[2]]).unwrap(),
                name: "outer".into(),
            })
            .unwrap()
        else {
            panic!("outer group")
        };
        editor
            .execute(Command::SetTrackColor {
                track: outer,
                color: ElementColor::Red,
            })
            .unwrap();
        editor
            .execute(Command::SetTrackColor {
                track: inner,
                color: ElementColor::Blue,
            })
            .unwrap();
        let mut projection = HierarchyIndent::default();
        projection.reconcile(1, 1, editor.project());
        assert!(projection.ancestors(outer).is_empty());
        assert_eq!(projection.ancestors(inner), &[(outer, ElementColor::Red)]);
        assert_eq!(
            projection.ancestors(ids[0]),
            &[(outer, ElementColor::Red), (inner, ElementColor::Blue)]
        );
        assert_eq!(projection.pixels(ids[0]), 8.0);
        editor
            .execute(Command::SetTrackColor {
                track: outer,
                color: ElementColor::Green,
            })
            .unwrap();
        projection.reconcile(1, 2, editor.project());
        assert_eq!(
            projection.ancestors(ids[0])[0],
            (outer, ElementColor::Green)
        );
    }
}
