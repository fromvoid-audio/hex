//! Combined borrowed pages/observation contracts; no native publication.
use hex_engine::{
    Audio, BorrowedMediaPage, DeviceTypeId, Frame, LaunchQuantization, MediaPages,
    NativeDeviceClass, PostNodeIdentity, PreparedRevision, SessionPlayback, SessionPlaybackCommand,
    SineTable, StreamSourceKey, StreamedSourceMetadata,
    compile::PreparedAudioSource,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{Command, Created, DeviceSnapshot, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

struct Pages {
    key: StreamSourceKey,
    frames: Vec<Frame>,
    missing: bool,
    requests: usize,
}
impl MediaPages for Pages {
    fn page(&mut self, key: StreamSourceKey, page_index: u64) -> Option<BorrowedMediaPage<'_>> {
        (!self.missing && key == self.key && page_index == 0).then_some(BorrowedMediaPage {
            page_index,
            frames: &self.frames,
        })
    }
    fn request_page(&mut self, _: StreamSourceKey, _: u64) {
        self.requests += 1;
    }
}
struct Fixture {
    resident: Arc<PreparedRevision>,
    streamed: Arc<PreparedRevision>,
    metadata: Arc<StreamedSourceMetadata>,
    pages: Pages,
    clip: hex_project::ClipId,
}
fn fixture() -> Fixture {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "stream".into(),
            path: "stream.wav".into(),
            source_sample_rate: 48000,
            source_frames: 1024,
            playback_frames: 1024,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Imported { asset: other, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "resident".into(),
            path: "resident.wav".into(),
            source_sample_rate: 48000,
            source_frames: 1024,
            playback_frames: 1024,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "session".into(),
            asset,
            source_start: 64,
            source_end: 1024,
        })
        .unwrap()
    else {
        panic!("session")
    };
    let mut snapshot = editor.project().snapshot();
    let id = snapshot.next_id;
    snapshot.next_id += 1;
    snapshot.tracks[0].devices.push(DeviceSnapshot {
        id,
        bypassed: false,
        state: hex_engine::default_native_device_state_for_class(
            NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.eq8"),
        )
        .unwrap(),
    });
    let project = Project::from_snapshot(snapshot).unwrap();
    let pcm: Vec<_> = (0..1024)
        .map(|i| {
            if i % 2 == 0 {
                [0.25, -0.5]
            } else {
                [-0.125, 0.375]
            }
        })
        .collect();
    let metadata = Arc::new(
        StreamedSourceMetadata::new(
            StreamSourceKey::new(7, asset),
            48000,
            48000,
            1024,
            1024,
            hex_project::IDENTITY_CONVERSION,
            [17; 32],
        )
        .unwrap(),
    );
    let mut bank = BTreeMap::from([
        (
            asset,
            PreparedAudioSource::Resident(Audio::new(48000, pcm.clone()).unwrap()),
        ),
        (
            other,
            PreparedAudioSource::Resident(Audio::new(48000, vec![[0.125; 2]; 1024]).unwrap()),
        ),
    ]);
    let table = Arc::new(SineTable::new());
    let resident = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &project,
            &bank,
            7,
            Arc::clone(&table),
            None,
        )
        .unwrap(),
    );
    bank.insert(asset, PreparedAudioSource::Streamed(Arc::clone(&metadata)));
    let streamed = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(&project, &bank, 7, table, None)
            .unwrap(),
    );
    Fixture {
        resident,
        streamed,
        pages: Pages {
            key: metadata.key(),
            frames: pcm,
            missing: false,
            requests: 0,
        },
        metadata,
        clip,
    }
}

#[test]
fn mixed_arrangement_session_pcm_and_post_node_samples_match_resident_oracle() {
    let mut f = fixture();
    let mut resident = Playback::try_from_prepared_with_session(Arc::clone(&f.resident)).unwrap();
    let mut streamed = Playback::try_from_prepared_with_session(Arc::clone(&f.streamed)).unwrap();
    resident.apply(PlaybackCommand::Play);
    streamed.apply(PlaybackCommand::Play);
    let owners = Arc::strong_count(&f.streamed);
    let metadata_owners = Arc::strong_count(&f.metadata);
    for action in [
        None,
        Some(SessionPlaybackCommand::LaunchClip {
            clip: f.clip,
            quantization: LaunchQuantization::Immediate,
        }),
        Some(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        }),
    ] {
        if let Some(action) = action {
            resident
                .try_apply_action(hex_engine::PlaybackAction::Session(action))
                .unwrap();
            streamed
                .try_apply_action(hex_engine::PlaybackAction::Session(action))
                .unwrap();
        }
        let mut expected = [[0.0; 2]; 257];
        let mut actual = [[0.0; 2]; 257];
        let mut expected_nodes: Vec<(PostNodeIdentity, usize, Vec<Frame>)> = Vec::new();
        let mut actual_nodes = Vec::new();
        resident
            .render_with_post_node_observer(
                &mut expected,
                Some(&mut |id, offset, samples: &[Frame]| {
                    expected_nodes.push((id, offset, samples.to_vec()))
                }),
            )
            .unwrap();
        // Explicit trait-object use proves the new ?Sized provider boundary.
        let pages: &mut dyn MediaPages = &mut f.pages;
        streamed
            .render_with_pages_and_post_node_observer(
                &mut actual,
                pages,
                Some(&mut |id, offset, samples: &[Frame]| {
                    actual_nodes.push((id, offset, samples.to_vec()))
                }),
            )
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual_nodes, expected_nodes);
        assert!(!actual_nodes.is_empty());
        assert!(
            actual_nodes
                .iter()
                .any(|(id, _, _)| id.device_type == DeviceTypeId::from_static("hex.eq8"))
        );
        assert_eq!(streamed.position(), resident.position());
        assert_eq!(streamed.performance_frame(), resident.performance_frame());
        assert_eq!(Arc::strong_count(&f.streamed), owners);
        assert_eq!(Arc::strong_count(&f.metadata), metadata_owners);
    }
    assert_eq!(f.pages.requests, 0);
}

#[test]
fn pause_empty_misses_and_unavailable_data_preserve_render_contract() {
    let mut f = fixture();
    let mut session = SessionPlayback::new(Arc::clone(&f.streamed)).unwrap();
    let mut calls = 0;
    let mut output = [[9.0; 2]; 512];
    session
        .render_with_pages_and_post_node_observer(
            &mut output,
            &mut f.pages,
            Some(&mut |_, _, _: &[Frame]| calls += 1),
        )
        .unwrap();
    assert_eq!(output, [[0.0; 2]; 512]);
    assert_eq!(calls, 0);
    assert_eq!(session.performance_frame(), 0);
    session.apply(SessionPlaybackCommand::Play).unwrap();
    session
        .render_with_pages_and_post_node_observer(
            &mut [],
            &mut f.pages,
            Some(&mut |_, _, _: &[Frame]| calls += 1),
        )
        .unwrap();
    assert_eq!(calls, 0);
    assert_eq!(session.performance_frame(), 0);
    f.pages.missing = true;
    let mut oracle = SessionPlayback::new(Arc::clone(&f.streamed)).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut missing = Pages {
        key: f.pages.key,
        frames: f.pages.frames.clone(),
        missing: true,
        requests: 0,
    };
    let mut expected = [[0.0; 2]; 512];
    oracle
        .render_with_pages(&mut expected, &mut missing)
        .unwrap();
    session
        .render_with_pages_and_post_node_observer(
            &mut output,
            &mut f.pages,
            Some(&mut |_, _, _: &[Frame]| calls += 1),
        )
        .unwrap();
    assert_eq!(output, expected);
    assert!(output.iter().any(|frame| *frame != [0.0; 2]));
    assert!(f.pages.requests > 0);
    assert_eq!(session.performance_frame(), 512);
    // Malformed page samples follow the existing unavailable-data policy;
    // they are not a typed render error. Preserve that behavior as well.
    session.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    f.pages.missing = false;
    f.pages.frames[300] = [f32::NAN; 2];
    session
        .render_with_pages_and_post_node_observer(&mut output, &mut f.pages, None)
        .unwrap();
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
}

#[test]
fn new_observed_legacy_refusal_is_before_all_mutation_and_old_paths_remain_valid() {
    let mut f = fixture();
    let mut legacy = Playback::from_shared(Arc::clone(f.streamed.arrangement()));
    legacy.apply(PlaybackCommand::Play);
    let before = (
        legacy.position(),
        legacy.performance_frame(),
        legacy.playing(),
    );
    let mut output = [[9.0; 2]; 32];
    let mut calls = 0;
    assert!(
        legacy
            .render_with_pages_and_post_node_observer(
                &mut output,
                &mut f.pages,
                Some(&mut |_, _, _: &[Frame]| calls += 1)
            )
            .is_err()
    );
    assert_eq!(output, [[9.0; 2]; 32]);
    assert_eq!(calls, 0);
    assert_eq!(f.pages.requests, 0);
    assert_eq!(
        (
            legacy.position(),
            legacy.performance_frame(),
            legacy.playing()
        ),
        before
    );
    legacy
        .render_with_pages_and_post_node_observer(&mut output, &mut f.pages, None)
        .unwrap();
    let mut old = Playback::from_shared(Arc::clone(f.streamed.arrangement()));
    old.apply(PlaybackCommand::Play);
    let mut expected = [[0.0; 2]; 32];
    old.render_with_pages(&mut expected, &mut f.pages).unwrap();
    assert_eq!(output, expected);
    for playing in [false, true] {
        for empty in [false, true] {
            let mut legacy = Playback::from_shared(Arc::clone(f.streamed.arrangement()));
            if playing {
                legacy.apply(PlaybackCommand::Play);
            }
            let before = (
                legacy.position(),
                legacy.performance_frame(),
                legacy.playing(),
                f.pages.requests,
            );
            let mut untouched = [[9.0; 2]; 32];
            let target = if empty {
                &mut untouched[..0]
            } else {
                &mut untouched[..]
            };
            assert!(
                legacy
                    .render_with_pages_and_post_node_observer(
                        target,
                        &mut f.pages,
                        Some(&mut |_, _, _: &[Frame]| calls += 1)
                    )
                    .is_err()
            );
            assert_eq!(untouched, [[9.0; 2]; 32]);
            assert_eq!(
                (
                    legacy.position(),
                    legacy.performance_frame(),
                    legacy.playing(),
                    f.pages.requests
                ),
                before
            );
        }
    }
    let mut resident = Playback::from_shared(Arc::clone(f.resident.arrangement()));
    resident.apply(PlaybackCommand::Play);
    resident
        .render_with_post_node_observer(&mut expected, Some(&mut |_, _, _: &[Frame]| calls += 1))
        .unwrap();
    assert_eq!(
        calls, 0,
        "legacy observed path still has its existing behavior"
    );
    let mut session = SessionPlayback::new(Arc::clone(&f.streamed)).unwrap();
    session.apply(SessionPlaybackCommand::Play).unwrap();
    assert!(
        session
            .render_with_post_node_observer(&mut expected, None)
            .is_err(),
        "provider-less stream refusal preserved"
    );
}
