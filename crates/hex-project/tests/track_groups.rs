use hex_project::{
    Command, Created, Editor, Project, RoutingOwner, TrackDestination, TrackId, TrackKind,
    TrackSelectionIds,
};

fn fixture() -> (Editor, [TrackId; 4]) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
    let ids = ["a", "b", "c", "d"].map(|name| {
        let Created::Track(id) = e.execute(Command::AddTrack { name: name.into() }).unwrap() else {
            panic!("track")
        };
        id
    });
    (e, ids)
}
fn selection(ids: &[TrackId]) -> TrackSelectionIds {
    TrackSelectionIds::new(ids).unwrap()
}
fn group(e: &mut Editor, ids: &[TrackId]) -> TrackId {
    let Created::Track(id) = e
        .execute(Command::GroupTracks {
            tracks: selection(ids),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    id
}
fn ids(e: &Editor) -> Vec<TrackId> {
    e.project().tracks().iter().map(|t| t.id()).collect()
}

#[test]
fn nonadjacent_siblings_group_once_with_complete_undo_redo() {
    let (mut e, [a, b, c, d]) = fixture();
    let mut before = e.project().snapshot();
    let g = group(&mut e, &[c, a]);
    assert_eq!(ids(&e), [g, a, c, b, d]);
    let after = e.project().snapshot();
    for child in [a, c] {
        assert_eq!(e.project().track(child).unwrap().parent_group(), Some(g));
    }
    assert_eq!(e.project().track(g).unwrap().kind(), TrackKind::Group);
    assert_eq!(e.project().track(g).unwrap().gain(), 1.0);
    assert_eq!(
        e.project()
            .track_hierarchy()
            .unwrap()
            .row(g)
            .unwrap()
            .subtree_range(),
        0..3
    );
    // History preserves the identity high-water mark to prevent ID reuse.
    before.next_id = after.next_id;
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}

#[test]
fn nested_selection_moves_one_subtree_and_refuses_self_parent_atomically() {
    let (mut e, [a, b, c, d]) = fixture();
    let inner = group(&mut e, &[a, b]);
    let outer = group(&mut e, &[inner, c]);
    e.execute(Command::ReparentTracks {
        tracks: selection(&[inner, a]),
        destination: TrackDestination {
            parent_group: None,
            before: None,
        },
    })
    .unwrap();
    assert_eq!(ids(&e), [outer, c, d, inner, a, b]);
    assert_eq!(e.project().track(a).unwrap().parent_group(), Some(inner));
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::ReparentTracks {
            tracks: selection(&[inner]),
            destination: TrackDestination {
                parent_group: Some(inner),
                before: None
            }
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    assert!(e.undo());
    assert_eq!(ids(&e), [outer, inner, a, b, c, d]);
    // A no-op may not erase the undone move.
    e.execute(Command::ReparentTracks {
        tracks: selection(&[inner]),
        destination: TrackDestination {
            parent_group: Some(outer),
            before: Some(inner),
        },
    })
    .unwrap();
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn duplicate_last_child_and_whole_group_remap_parents_without_escaping() {
    let (mut e, [a, b, c, d]) = fixture();
    let g = group(&mut e, &[a, b]);
    e.execute(Command::DuplicateTracks {
        tracks: selection(&[b]),
        without_clips: false,
    })
    .unwrap();
    let copy = e.project().tracks()[3].id();
    assert_ne!(copy, b);
    assert_eq!(ids(&e), [g, a, b, copy, c, d]);
    assert_eq!(e.project().track(copy).unwrap().parent_group(), Some(g));
    e.execute(Command::DuplicateTracks {
        tracks: selection(&[g, a]),
        without_clips: true,
    })
    .unwrap();
    let tracks = e.project().tracks();
    let copy_group = tracks[4].id();
    assert!(tracks[4].is_group());
    assert_eq!(tracks[4].parent_group(), None);
    for child in &tracks[5..8] {
        assert_eq!(child.parent_group(), Some(copy_group));
    }
    assert_eq!(tracks.len(), 10);
    assert!(e.undo());
    assert_eq!(ids(&e), [g, a, b, copy, c, d]);
}

#[test]
fn ungroup_retains_children_delete_removes_subtree_and_undo_restores() {
    let (mut e, [a, b, c, d]) = fixture();
    let inner = group(&mut e, &[a, b]);
    let outer = group(&mut e, &[inner, c]);
    e.execute(Command::UngroupTrack(inner)).unwrap();
    assert_eq!(ids(&e), [outer, a, b, c, d]);
    assert_eq!(e.project().track(a).unwrap().parent_group(), Some(outer));
    let before = e.project().snapshot();
    e.execute(Command::RemoveTrack(outer)).unwrap();
    assert_eq!(ids(&e), [d]);
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn groups_reject_console_cells_and_implicit_output_feedback_without_history_changes() {
    let (mut e, [a, b, _, _]) = fixture();
    let g = group(&mut e, &[a, b]);
    let Created::Scene(scene) = e
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::SetSessionStopCell { track: g, scene })
            .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    assert!(
        e.execute(Command::SetAudioFeed {
            source: RoutingOwner::Track(g),
            destination: a,
            gain: 1.0
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    e.execute(Command::SetAudioFeed {
        source: RoutingOwner::Track(a),
        destination: b,
        gain: 0.5,
    })
    .unwrap();
}

#[test]
fn depth_limit_and_discontiguous_or_missing_parents_refuse_without_mutation() {
    let (mut e, [a, b, _, _]) = fixture();
    let mut root = a;
    for _ in 0..hex_project::MAX_GROUP_DEPTH {
        root = group(&mut e, &[root]);
    }
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::GroupTracks {
            tracks: selection(&[root]),
            name: "too deep".into()
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    let mut malformed = before.clone();
    malformed.tracks.last_mut().unwrap().parent_group = Some(root.value());
    assert!(Project::from_snapshot(malformed).is_err());
    let mut malformed = before.clone();
    malformed
        .tracks
        .iter_mut()
        .find(|t| t.id == b.value())
        .unwrap()
        .parent_group = Some(a.value());
    assert!(Project::from_snapshot(malformed).is_err());
}

#[test]
fn group_content_and_track_limit_refusals_preserve_ids_and_redo() {
    let (mut e, [a, b, _, _]) = fixture();
    let g = group(&mut e, &[a, b]);
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::PlaceMidiClip {
            track: g,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::ONE
        })
        .is_err()
    );
    assert!(
        e.execute(Command::InsertInstrument {
            track: g,
            state: hex_project::default_instrument_state().unwrap()
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    // All hierarchy rows, including empty groups, share the existing capacity.
    while e.project().tracks().len() < hex_project::MAX_TRACKS {
        e.execute(Command::AddTrack {
            name: "leaf".into(),
        })
        .unwrap();
    }
    let full = e.project().snapshot();
    assert!(
        e.execute(Command::GroupTracks {
            tracks: selection(&[g]),
            name: "overflow".into()
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), full);
    assert!(e.undo());
    let below = e.project().snapshot();
    assert!(
        e.execute(Command::ReparentTracks {
            tracks: selection(&[g]),
            destination: TrackDestination {
                parent_group: Some(a),
                before: None
            }
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), below);
    assert!(e.can_redo());
}

#[test]
fn batch_validates_group_content_before_preparing_and_preserves_history_on_refusal() {
    let (mut e, [a, b, _, _]) = fixture();
    let g = group(&mut e, &[a, b]);
    let before = e.project().snapshot();
    let mut prepared = false;
    let result: Result<_, hex_project::Invalid> = e.execute_many_prepared(
        vec![Command::InsertInstrument {
            track: g,
            state: hex_project::default_instrument_state().unwrap(),
        }],
        |_, _| {
            prepared = true;
            Ok(())
        },
    );
    assert!(result.is_err());
    assert!(!prepared);
    assert_eq!(e.project().snapshot(), before);
    assert!(e.undo());
    assert!(e.can_redo());
}

#[test]
fn reparent_preserves_an_authored_additive_feed_parallel_to_default_output() {
    let (mut e, [a, b, c, _]) = fixture();
    let g = group(&mut e, &[b]);
    e.execute(Command::SetAudioFeed {
        source: RoutingOwner::Track(a),
        destination: g,
        gain: 0.5,
    })
    .unwrap();
    let routes = e.project().audio_feeds().to_vec();
    e.execute(Command::ReparentTracks {
        tracks: selection(&[a]),
        destination: TrackDestination {
            parent_group: Some(g),
            before: None,
        },
    })
    .unwrap();
    assert_eq!(e.project().audio_feeds(), routes);
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::SetAudioFeed {
            source: RoutingOwner::Track(g),
            destination: a,
            gain: 0.5
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    e.execute(Command::SetAudioFeed {
        source: RoutingOwner::Track(g),
        destination: c,
        gain: 0.5,
    })
    .unwrap();
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::ReparentTracks {
            tracks: selection(&[c]),
            destination: TrackDestination {
                parent_group: Some(g),
                before: None
            }
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
}
