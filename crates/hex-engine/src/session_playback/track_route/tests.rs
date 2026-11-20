use super::*;
use crate::{Audio, PreparedRevision};
use hex_project::{Command, Created, Editor, Project, TrackDestination, TrackSelectionIds};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn one_resident_leaf_reroutes_between_main_and_group_and_other_edits_refuse() {
    let (editor, media, leaf, group) = fixture();
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
    assert_eq!(next.resident_group_reroute_from(&old), Some(2));
    assert_eq!(
        old.resident_group_reroute_from(&next),
        Some(2),
        "the exact inverse keeps the Group and its other child"
    );
    assert_eq!(old.exact_resident_group_entry_from(&next), None);
    let mut changed = next;
    changed.routes[0].owner = RouteOwner::Silence;
    assert_eq!(changed.resident_group_reroute_from(&old), None);
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
                .resident_group_reroute_from(&old),
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
            .resident_group_reroute_from(&old),
        None
    );

    let sources = media
        .iter()
        .map(|(&asset, _)| {
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
                        [19; 32],
                    )
                    .unwrap(),
                )),
            )
        })
        .collect();
    let streamed = |project: &Project| {
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
    };
    let streamed_before = streamed(editor.project());
    let streamed_after = streamed(next_editor.project());
    for (old, next) in [
        (&streamed_before, &streamed_after),
        (&before, &streamed_after),
        (&streamed_before, &after),
    ] {
        let old = SessionPlayback::new(Arc::clone(old)).unwrap();
        let mut next = SessionPlayback::new(Arc::clone(next)).unwrap();
        next.require_continuous_insert = true;
        assert_eq!(next.resident_group_reroute_from(&old), None);
        assert_eq!(old.resident_group_reroute_from(&next), None);
        assert!(!next.plan_native_removal_from(&old).unwrap());
        assert!(old.track_faders.iter().all(|fader| !fader.active()));
    }
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
