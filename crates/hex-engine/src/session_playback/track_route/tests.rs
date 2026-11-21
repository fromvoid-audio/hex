use super::*;
use crate::{Audio, PreparedRevision};
use hex_project::{Command, Created, Editor, Project, TrackDestination, TrackSelectionIds};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn one_resident_leaf_reroutes_between_main_and_group_and_other_edits_refuse() {
    let (mut editor, media, leaf, group) = fixture();
    let Created::Track(unrelated) = editor
        .execute(Command::AddTrack {
            name: "unrelated".into(),
        })
        .unwrap()
    else {
        panic!("track");
    };
    let prepare = |project: &Project| {
        Arc::new(
            PreparedRevision::prepare(project, &media, Arc::new(crate::SineTable::new()), None)
                .unwrap(),
        )
    };
    let before = prepare(editor.project());
    let mut next_editor = Editor::new(editor.project().clone(), 16).unwrap();
    next_editor
        .execute(Command::ReparentTracks {
            tracks: TrackSelectionIds::new(&[leaf]).unwrap(),
            destination: TrackDestination {
                parent_group: Some(group),
                before: None,
            },
        })
        .unwrap();
    let after = prepare(next_editor.project());
    let old = SessionPlayback::new(Arc::clone(&before)).unwrap();
    let next = SessionPlayback::new(Arc::clone(&after)).unwrap();
    assert_eq!(next.group_reroute_from(&old), Some(2));
    assert_eq!(
        old.group_reroute_from(&next),
        Some(2),
        "the exact inverse keeps the Group and its other child"
    );
    let mut changed = next;
    changed.routes[0].owner = RouteOwner::Silence;
    assert_eq!(changed.group_reroute_from(&old), None);
    let device = next_editor.project().track(leaf).unwrap().devices()[0].id();
    for command in [
        Command::SetDeviceBypass {
            device,
            bypassed: true,
        },
        Command::ReplaceDeviceAutomation {
            device,
            parameter: hex_devices::delay::DRY_WET,
            points: vec![hex_project::AutomationPoint {
                beat: hex_project::BeatTime::ZERO,
                value: hex_project::AutomationValue::new(0.3).unwrap(),
                outgoing: hex_project::AutomationInterpolation::Step,
            }],
        },
        Command::Gain {
            track: group,
            gain: 0.5,
        },
        Command::MoveTracksBefore {
            tracks: TrackSelectionIds::new(&[leaf]).unwrap(),
            before: Some(group),
        },
        Command::MoveTracksBefore {
            tracks: TrackSelectionIds::new(&[unrelated]).unwrap(),
            before: Some(group),
        },
        Command::InsertDevice {
            track: group,
            before: None,
            state: hex_engine_delay(),
        },
    ] {
        let mut changed = Editor::new(next_editor.project().clone(), 8).unwrap();
        changed.execute(command).unwrap();
        assert_eq!(
            SessionPlayback::new(prepare(changed.project()))
                .unwrap()
                .group_reroute_from(&old),
            None
        );
    }
    // A changed source with the same canonical asset cannot join the old history.
    let mut changed_media = media.clone();
    let asset = editor.project().track(leaf).unwrap().clips()[0].asset();
    changed_media.insert(asset, Audio::new(48_000, vec![[0.5; 2]; 2048]).unwrap());
    let changed = Arc::new(
        PreparedRevision::prepare(
            next_editor.project(),
            &changed_media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    assert_eq!(
        SessionPlayback::new(changed)
            .unwrap()
            .group_reroute_from(&old),
        None
    );

    let streamed_before = streamed(editor.project(), &media, [19; 32]);
    let streamed_after = streamed(next_editor.project(), &media, [19; 32]);
    let changed_stream = streamed(next_editor.project(), &media, [31; 32]);
    let mut old = SessionPlayback::new(Arc::clone(&streamed_before)).unwrap();
    old.playing = true;
    let mut next = SessionPlayback::new(Arc::clone(&streamed_after)).unwrap();
    next.require_continuous_insert = true;
    assert_eq!(next.group_reroute_from(&old), Some(2));
    assert!(next.plan_native_removal_from(&old).unwrap());
    assert_eq!(old.group_reroute_from(&next), Some(2));
    assert!(old.track_faders.iter().all(|fader| !fader.active()));

    for (old, next) in [
        (&streamed_before, &changed_stream),
        (&before, &streamed_after),
        (&streamed_before, &after),
    ] {
        let mut old = SessionPlayback::new(Arc::clone(old)).unwrap();
        old.playing = true;
        let mut next = SessionPlayback::new(Arc::clone(next)).unwrap();
        next.require_continuous_insert = true;
        assert_eq!(next.group_reroute_from(&old), None);
        assert_eq!(old.group_reroute_from(&next), None);
        assert!(!next.plan_native_removal_from(&old).unwrap());
        assert!(old.track_faders.iter().all(|fader| !fader.active()));
    }
}
fn streamed(
    project: &Project,
    media: &BTreeMap<hex_project::AssetId, Audio>,
    fingerprint: [u8; 32],
) -> Arc<PreparedRevision> {
    let sources = media
        .keys()
        .map(|&asset| {
            (
                asset,
                crate::compile::PreparedAudioSource::Streamed(Arc::new(
                    crate::StreamedSourceMetadata::new(
                        crate::StreamSourceKey::new(9, asset),
                        48_000,
                        48_000,
                        2048,
                        2048,
                        hex_project::IDENTITY_CONVERSION,
                        fingerprint,
                    )
                    .unwrap(),
                )),
            )
        })
        .collect();
    Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            project,
            &sources,
            9,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    )
}

#[test]
fn verified_first_child_requires_exact_rows_and_source_across_the_flat_boundary() {
    let (mut editor, media, leaf, group) = fixture();
    let sibling = editor
        .project()
        .tracks()
        .iter()
        .find(|track| track.parent_group() == Some(group))
        .unwrap()
        .id();
    editor
        .execute(Command::ReparentTracks {
            tracks: TrackSelectionIds::new(&[sibling]).unwrap(),
            destination: TrackDestination {
                parent_group: None,
                before: Some(group),
            },
        })
        .unwrap();
    let flat = SessionPlayback::new(streamed(editor.project(), &media, [19; 32])).unwrap();
    assert!(flat.compensation.is_none());
    let selection = TrackSelectionIds::new(&[leaf]).unwrap();
    let mut shifted = Editor::new(editor.project().clone(), 8).unwrap();
    shifted
        .execute(Command::MoveTracksBefore {
            tracks: selection.clone(),
            before: Some(group),
        })
        .unwrap();
    let shifted = SessionPlayback::new(streamed(shifted.project(), &media, [19; 32])).unwrap();
    editor
        .execute(Command::ReparentTracks {
            tracks: selection,
            destination: TrackDestination {
                parent_group: Some(group),
                before: None,
            },
        })
        .unwrap();
    let grouped = SessionPlayback::new(streamed(editor.project(), &media, [19; 32])).unwrap();
    assert!(grouped.compensation.is_some());
    assert_eq!(grouped.group_reroute_from(&flat), Some(2));
    assert_eq!(grouped.verified_flat_group_faders(&flat, 2), Some((2, 1)));
    assert_eq!(grouped.group_reroute_from(&shifted), None);
    assert_eq!(flat.group_reroute_from(&grouped), Some(2));
    assert_eq!(flat.verified_flat_group_faders(&grouped, 2), Some((1, 2)));
    assert_eq!(shifted.group_reroute_from(&grouped), None);
    let different_source =
        SessionPlayback::new(streamed(editor.project(), &media, [31; 32])).unwrap();
    assert_eq!(different_source.group_reroute_from(&flat), None);
    assert_eq!(flat.group_reroute_from(&different_source), None);

    editor
        .execute(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&[sibling]).unwrap(),
            name: "unrelated group".into(),
        })
        .unwrap();
    let two_groups = SessionPlayback::new(streamed(editor.project(), &media, [19; 32])).unwrap();
    editor
        .execute(Command::ReparentTracks {
            tracks: TrackSelectionIds::new(&[leaf]).unwrap(),
            destination: TrackDestination {
                parent_group: None,
                before: None,
            },
        })
        .unwrap();
    let other_group = SessionPlayback::new(streamed(editor.project(), &media, [19; 32])).unwrap();
    assert!(other_group.compensation.is_some());
    assert_eq!(other_group.group_reroute_from(&two_groups), None);
    assert_eq!(two_groups.group_reroute_from(&other_group), None);
}

#[test]
fn route_only_device_proof_refuses_foreign_metadata_without_a_binding() {
    let (editor, media, _, _) = fixture();
    let revision = PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::new(crate::SineTable::new()),
        None,
    )
    .unwrap();
    let native = revision.arrangement().tracks()[2].device_chain[0].clone();
    assert!(same_devices(
        std::slice::from_ref(&native),
        std::slice::from_ref(&native)
    ));
    let mut foreign = native.clone();
    foreign.definition = Some(hex_lattice::PreparedProcessorFactory::Foreign(
        hex_lattice::PreparedForeignProcessor::new(8, 0, 16).unwrap(),
    ));
    assert!(foreign.foreign_processor().unwrap().binding().is_none());
    for (old, next) in [
        (&native, &foreign),
        (&foreign, &native),
        (&foreign, &foreign),
    ] {
        assert!(!same_devices(
            std::slice::from_ref(old),
            std::slice::from_ref(next)
        ));
    }
}

#[test]
fn group_reroute_refuses_a_delayed_leaf_to_group_edge_in_either_direction() {
    use hex_lattice_contract::NativeDeviceMetadata;
    let (mut editor, media, leaf, group) = fixture();
    let child = editor
        .project()
        .tracks()
        .iter()
        .find(|track| track.parent_group() == Some(group))
        .unwrap()
        .id();
    editor
        .execute(Command::InsertDevice {
            track: child,
            before: None,
            state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
        })
        .unwrap();
    let prepare = |project: &Project| {
        Arc::new(
            PreparedRevision::prepare(project, &media, Arc::new(crate::SineTable::new()), None)
                .unwrap(),
        )
    };
    let before = prepare(editor.project());
    editor
        .execute(Command::ReparentTracks {
            tracks: TrackSelectionIds::new(&[leaf]).unwrap(),
            destination: TrackDestination {
                parent_group: Some(group),
                before: None,
            },
        })
        .unwrap();
    let after = prepare(editor.project());
    let delay = before.arrangement().main_latency_frames();
    assert!(delay > 0);
    assert_eq!(after.arrangement().main_latency_frames(), delay);
    let old = SessionPlayback::new(before).unwrap();
    let next = SessionPlayback::new(after).unwrap();
    assert_eq!(next.group_reroute_from(&old), None);
    assert_eq!(old.group_reroute_from(&next), None);
}

#[test]
fn aligned_group_entry_and_exit_require_reciprocal_row_shifts() {
    use hex_lattice_contract::NativeDeviceMetadata;
    let (mut editor, media, leaf, group) = fixture();
    let Created::Track(unrelated) = editor
        .execute(Command::AddTrack {
            name: "unrelated".into(),
        })
        .unwrap()
    else {
        panic!("unrelated track");
    };
    editor
        .execute(Command::InsertDevice {
            track: unrelated,
            before: None,
            state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::MoveTracksBefore {
            tracks: TrackSelectionIds::new(&[leaf]).unwrap(),
            before: Some(group),
        })
        .unwrap();
    let prepare = |editor: &Editor| {
        Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &media,
                Arc::new(crate::SineTable::new()),
                None,
            )
            .unwrap(),
        )
    };
    let before = prepare(&editor);
    editor
        .execute(Command::ReparentTracks {
            tracks: TrackSelectionIds::new(&[leaf]).unwrap(),
            destination: TrackDestination {
                parent_group: Some(group),
                before: None,
            },
        })
        .unwrap();
    let after = prepare(&editor);
    let latency = before.arrangement().main_latency_frames();
    assert!(latency > 0);
    assert_eq!(after.arrangement().main_latency_frames(), latency);
    let old = SessionPlayback::new(before).unwrap();
    let next = SessionPlayback::new(after).unwrap();
    assert_eq!(next.group_reroute_from(&old), Some(0));
    assert_eq!(old.group_reroute_from(&next), Some(2));

    let mut below = Editor::new(editor.project().clone(), 4).unwrap();
    below
        .execute(Command::ReparentTracks {
            tracks: TrackSelectionIds::new(&[leaf]).unwrap(),
            destination: TrackDestination {
                parent_group: None,
                before: None,
            },
        })
        .unwrap();
    let below = SessionPlayback::new(prepare(&below)).unwrap();
    assert_eq!(below.group_reroute_from(&next), None);
    assert_eq!(next.group_reroute_from(&below), None);

    editor
        .execute(Command::MoveTracksBefore {
            tracks: TrackSelectionIds::new(&[unrelated]).unwrap(),
            before: Some(group),
        })
        .unwrap();
    let compound = SessionPlayback::new(prepare(&editor)).unwrap();
    assert_eq!(compound.group_reroute_from(&old), None);
}

#[test]
fn aligned_nested_group_reroute_requires_immediately_related_groups() {
    use hex_lattice_contract::NativeDeviceMetadata;
    let (mut editor, media, leaf, inner) = fixture();
    let group = |editor: &mut Editor, track, name: &str| {
        let Created::Track(id) = editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&[track]).unwrap(),
                name: name.into(),
            })
            .unwrap()
        else {
            panic!("enclosing group");
        };
        id
    };
    let enclosing = group(&mut editor, inner, "enclosing");
    let outermost = group(&mut editor, enclosing, "outermost");
    let Created::Track(unrelated) = editor
        .execute(Command::AddTrack {
            name: "unrelated".into(),
        })
        .unwrap()
    else {
        panic!("unrelated track");
    };
    editor
        .execute(Command::InsertDevice {
            track: unrelated,
            before: None,
            state: hex_devices::limiter::DEFINITION.default_state().unwrap(),
        })
        .unwrap();
    let mut prepare_with_parent = |parent_group| {
        editor
            .execute(Command::ReparentTracks {
                tracks: TrackSelectionIds::new(&[leaf]).unwrap(),
                destination: TrackDestination {
                    parent_group: Some(parent_group),
                    before: None,
                },
            })
            .unwrap();
        Arc::new(
            PreparedRevision::prepare(
                editor.project(),
                &media,
                Arc::new(crate::SineTable::new()),
                None,
            )
            .unwrap(),
        )
    };
    let inner_route = prepare_with_parent(inner);
    let enclosing_route = prepare_with_parent(enclosing);
    let outermost_route = prepare_with_parent(outermost);
    let latency = inner_route.arrangement().main_latency_frames();
    assert!(latency > 0);
    assert_eq!(enclosing_route.arrangement().main_latency_frames(), latency);
    assert_eq!(outermost_route.arrangement().main_latency_frames(), latency);
    let inner_route = SessionPlayback::new(inner_route).unwrap();
    let enclosing_route = SessionPlayback::new(enclosing_route).unwrap();
    let outermost_route = SessionPlayback::new(outermost_route).unwrap();
    // Each pair has the same track order and delay. Only the adjacent
    // parent/child routes can exchange their leaf.
    for (old, next) in [
        (&inner_route, &enclosing_route),
        (&enclosing_route, &inner_route),
    ] {
        assert!(next.group_reroute_from(old).is_some());
    }
    for (old, next) in [
        (&inner_route, &outermost_route),
        (&outermost_route, &inner_route),
    ] {
        assert_eq!(next.group_reroute_from(old), None);
    }
}

fn hex_engine_delay() -> hex_lattice_contract::NativeDeviceState {
    use hex_lattice_contract::NativeDeviceMetadata;
    hex_devices::delay::DEFINITION.default_state().unwrap()
}
fn fixture() -> (
    Editor,
    BTreeMap<hex_project::AssetId, Audio>,
    hex_project::TrackId,
    hex_project::TrackId,
) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let mut media = BTreeMap::new();
    let mut tracks = Vec::new();
    let mut group = None;
    for name in ["inside", "moving"] {
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: name.into(),
                path: format!("{name}.wav").into(),
                sample_rate: 48_000,
                frames: 2048,
            })
            .unwrap()
        else {
            panic!("audio")
        };
        media.insert(asset, Audio::new(48_000, vec![[0.25; 2]; 2048]).unwrap());
        tracks.push(track);
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: hex_engine_delay(),
            })
            .unwrap();
        if group.is_none() {
            let Created::Track(id) = editor
                .execute(Command::GroupTracks {
                    tracks: TrackSelectionIds::new(&[track]).unwrap(),
                    name: "group".into(),
                })
                .unwrap()
            else {
                panic!("group")
            };
            group = Some(id);
        }
    }
    (editor, media, tracks[1], group.unwrap())
}
