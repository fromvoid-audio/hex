//! Canonical descendant interval projection and revision-keyed group cache.
use super::super::arrangement_map::{Interval, append_track_intervals, merge};
use hex_project::{Project, TrackId};
use std::{collections::BTreeMap, rc::Rc};

#[derive(Default)]
pub(super) struct Group {
    pub(super) rows: Vec<Rc<[Interval]>>,
    pub(super) blocks: Vec<Interval>,
}
#[derive(Default)]
pub(in crate::editor) struct Cache {
    stamp: Option<(u64, u64)>,
    groups: Rc<BTreeMap<TrackId, Group>>,
}
impl Cache {
    pub(super) fn read(
        &mut self,
        generation: u64,
        revision: u64,
        project: &Project,
    ) -> Rc<BTreeMap<TrackId, Group>> {
        if self.stamp != Some((generation, revision)) {
            self.groups = Rc::new(project_groups(project));
            self.stamp = Some((generation, revision));
        }
        Rc::clone(&self.groups)
    }
}
fn project_groups(project: &Project) -> BTreeMap<TrackId, Group> {
    let mut groups = BTreeMap::<TrackId, Group>::new();
    let mut ancestors = Vec::with_capacity(hex_project::MAX_GROUP_DEPTH);
    for track in project.tracks() {
        while ancestors.last().copied() != track.parent_group() && !ancestors.is_empty() {
            ancestors.pop();
        }
        if track.is_group() {
            groups.entry(track.id()).or_default();
            ancestors.push(track.id());
        } else if !ancestors.is_empty() {
            let mut intervals = Vec::new();
            append_track_intervals(project, track, &mut intervals);
            merge(&mut intervals);
            if intervals.is_empty() {
                continue;
            }
            let row: Rc<[Interval]> = intervals.into();
            for ancestor in &ancestors {
                let group = groups.entry(*ancestor).or_default();
                group.blocks.extend_from_slice(&row);
                group.rows.push(Rc::clone(&row));
            }
        }
    }
    for group in groups.values_mut() {
        merge(&mut group.blocks);
    }
    groups
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_project::{
        BeatTime, Command, Created, Editor, IDENTITY_CONVERSION, Tempo, TrackSelectionIds,
    };
    fn add(model: &mut Editor, name: &str) -> TrackId {
        let Created::Track(id) = model
            .execute(Command::AddTrack { name: name.into() })
            .unwrap()
        else {
            panic!("track")
        };
        id
    }
    fn group(model: &mut Editor, tracks: &[TrackId], name: &str) -> TrackId {
        let Created::Track(id) = model
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(tracks).unwrap(),
                name: name.into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        id
    }
    fn midi(model: &mut Editor, track: TrackId, start: u64, length: u64) {
        model
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(start).unwrap(),
                length: BeatTime::from_quarters(length).unwrap(),
            })
            .unwrap();
    }
    fn mixed() -> (Editor, TrackId, TrackId, TrackId) {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let a = add(&mut model, "a");
        let b = add(&mut model, "b");
        let c = add(&mut model, "c");
        let other = add(&mut model, "unrelated");
        model
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(4).unwrap(),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        model
            .execute(Command::ImportPreparedAudioClip {
                track: a,
                start: 0,
                path: "/tmp/group-overview.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 96_000,
                playback_frames: 96_000,
                conversion_version: IDENTITY_CONVERSION,
            })
            .unwrap();
        midi(&mut model, b, 2, 4);
        midi(&mut model, c, 8, 2);
        midi(&mut model, other, 0, 64);
        let inner = group(&mut model, &[a, b], "inner");
        let outer = group(&mut model, &[inner, c], "outer");
        (model, inner, outer, a)
    }
    #[test]
    fn nested_groups_share_rows_use_mixed_time_and_exclude_unrelated_content() {
        let (model, inner, outer, _) = mixed();
        let before = model.project().snapshot();
        let data = project_groups(model.project());
        assert_eq!(
            data[&inner].blocks,
            vec![Interval {
                start: 0,
                end: 192_000
            }]
        );
        assert_eq!(
            data[&outer].blocks,
            vec![
                Interval {
                    start: 0,
                    end: 192_000
                },
                Interval {
                    start: 288_000,
                    end: 384_000
                }
            ]
        );
        assert_eq!(data[&inner].rows.len(), 2);
        assert_eq!(data[&outer].rows.len(), 3);
        assert!(Rc::ptr_eq(&data[&inner].rows[0], &data[&outer].rows[0]));
        assert!(Rc::ptr_eq(&data[&inner].rows[1], &data[&outer].rows[1]));
        assert_eq!(model.project().snapshot(), before);
    }
    #[test]
    fn adjacent_boundaries_and_empty_groups_are_not_invented_or_joined() {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let a = add(&mut model, "a");
        let empty = add(&mut model, "empty");
        midi(&mut model, a, 0, 2);
        midi(&mut model, a, 2, 2);
        midi(&mut model, a, 6, 2);
        let full = group(&mut model, &[a], "full");
        let empty = group(&mut model, &[empty], "empty");
        let data = project_groups(model.project());
        assert_eq!(
            data[&full].blocks,
            vec![
                Interval {
                    start: 0,
                    end: 48_000
                },
                Interval {
                    start: 48_000,
                    end: 96_000
                },
                Interval {
                    start: 144_000,
                    end: 192_000
                }
            ]
        );
        assert!(data[&empty].rows.is_empty() && data[&empty].blocks.is_empty());
    }
    #[test]
    fn cache_reuses_projection_and_retires_changed_or_replaced_context() {
        let (mut model, _, outer, a) = mixed();
        let mut cache = Cache::default();
        let original = cache.read(1, 1, model.project());
        assert!(Rc::ptr_eq(&original, &cache.read(1, 1, model.project())));
        let clip = model.project().track(a).unwrap().clips()[0].id();
        model.execute(Command::RemoveClip(clip)).unwrap();
        let changed = cache.read(1, 2, model.project());
        assert!(!Rc::ptr_eq(&original, &changed));
        assert_eq!(changed[&outer].blocks[0].start, 48_000);
        assert!(model.undo());
        let restored = cache.read(1, 3, model.project());
        assert_eq!(restored[&outer].blocks, original[&outer].blocks);
        assert!(cache.read(2, 3, &Project::new(48_000).unwrap()).is_empty());
    }
    #[test]
    fn hundred_track_projection_is_bounded_and_reused() {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        let mut tracks = Vec::new();
        for i in 0..100 {
            let track = add(&mut model, &format!("track {i}"));
            midi(&mut model, track, 0, 4);
            tracks.push(track);
        }
        let group = group(&mut model, &tracks, "all");
        let mut cache = Cache::default();
        let first = cache.read(1, 1, model.project());
        assert_eq!(first[&group].rows.len(), 100);
        assert_eq!(
            first[&group].blocks,
            vec![Interval {
                start: 0,
                end: 96_000
            }]
        );
        assert!(Rc::ptr_eq(&first, &cache.read(1, 1, model.project())));
    }
}
