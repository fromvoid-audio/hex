//! Native publication refuses streams until callback page ownership is integrated.
use hex_audio::prepared::{self, PublicationPolicy};
use hex_engine::{
    Audio, Clip, PreparedRevision, Project, StreamSourceKey, StreamedSourceMetadata, Track,
    compile::PreparedAudioSource, playback::Playback,
};
use hex_project::{Command, Created, Editor, Project as CanonicalProject};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering::Acquire},
};

fn streamed_session_only() -> Arc<PreparedRevision> {
    let mut editor = Editor::new(CanonicalProject::new(48_000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "test.wav".into(),
            sample_rate: 48_000,
            frames: 8192,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    editor
        .execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "one shot".into(),
            asset,
            source_start: 4093,
            source_end: 8192,
        })
        .unwrap();
    let metadata = StreamedSourceMetadata::new(
        StreamSourceKey::new(77, asset),
        48_000,
        48_000,
        8192,
        8192,
        0,
        [0x34; 32],
    )
    .unwrap();
    let bank = BTreeMap::from([(asset, PreparedAudioSource::Streamed(Arc::new(metadata)))]);
    Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            editor.project(),
            &bank,
            77,
            hex_engine::SharedSineTable::new_on_control(),
            None,
        )
        .unwrap(),
    )
}

#[test]
fn streamed_candidates_are_returned_before_publication_or_revision_admission() {
    let revision = streamed_session_only();
    assert!(
        !revision.arrangement().contains_streamed_sources(),
        "fixture must test Session-only streams"
    );
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    for policy in [
        PublicationPolicy::InheritRuntime,
        PublicationPolicy::InstallFreshRuntime,
    ] {
        for candidate in [
            Playback::from_prepared(Arc::clone(&revision)),
            Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap(),
        ] {
            assert!(candidate.contains_streamed_sources());
            let rejected = match control.publish_with_policy(candidate, policy) {
                Ok(_) => panic!("native callback admitted unsupported streams"),
                Err(e) => e,
            };
            assert_eq!(
                rejected.reason,
                "streamed playback requires a native media page endpoint"
            );
            assert!(
                rejected.playback.contains_streamed_sources(),
                "caller keeps candidate"
            );
            callback.begin_block();
            assert_eq!(control.status.applied_revision.load(Acquire), 0);
        }
    }
    // Refusal must not consume a publication slot or increment the revision.
    let resident = Playback::new(
        Project::new(
            48_000,
            vec![
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
                    1.0,
                    false,
                )
                .unwrap(),
            ],
        )
        .unwrap(),
    );
    let admitted = match control.publish(resident) {
        Ok(n) => n,
        Err(e) => panic!("{}", e.reason),
    };
    assert_eq!(admitted, 1);
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}
