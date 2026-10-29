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
    fixture_with_feeds(false)
}
fn fixture_with_feeds(feeds: bool) -> Fixture {
    fixture_with_options(feeds, false)
}
fn fixture_with_options(feeds: bool, automation: bool) -> Fixture {
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
    if feeds {
        let mut returns = Vec::new();
        for _ in 0..2 {
            let Created::ReturnTrack(id) = editor
                .execute(Command::AddReturnTrack {
                    name: "feed".into(),
                })
                .unwrap()
            else {
                panic!("return")
            };
            returns.push(id);
            let state = hex_engine::patch_native_device_parameter(
                &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                    .unwrap(),
                hex_lattice_contract::ParameterId::new(11).unwrap(),
                hex_lattice_contract::ParameterValue::Bool(true),
            )
            .unwrap();
            editor
                .execute(Command::InsertReturnDevice {
                    return_track: id,
                    before: None,
                    state,
                })
                .unwrap();
        }
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: returns[1],
                gain: 0.5,
            })
            .unwrap();
        editor
            .execute(Command::SetReturnFeed {
                source: returns[1],
                destination: returns[0],
                gain: 0.5,
            })
            .unwrap();
    }
    if automation {
        editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
        editor
            .execute(Command::ReplaceTrackGainEnvelope {
                track,
                points: vec![
                    hex_project::GainPoint {
                        incoming_gain: None,
                        beat: hex_project::BeatTime::ZERO,
                        gain: 0.25,
                        outgoing: hex_project::GainInterpolation::Linear,
                    },
                    hex_project::GainPoint {
                        incoming_gain: None,
                        beat: hex_project::BeatTime::ONE,
                        gain: 1.5,
                        outgoing: hex_project::GainInterpolation::Step,
                    },
                ],
            })
            .unwrap();
    }
    let mut snapshot = editor.project().snapshot();
    let id = snapshot.next_id;
    snapshot.next_id += 1;
    snapshot.tracks[0].devices.push(DeviceSnapshot {
        automation: Vec::new(),
        comparison: None,
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
    assert_mixed_pcm_and_observation(fixture());
}
#[test]
fn authored_feeds_with_borrowed_pages_match_resident_pcm_nodes_and_owner_counts() {
    assert_mixed_pcm_and_observation(fixture_with_feeds(true));
}
fn assert_mixed_pcm_and_observation(mut f: Fixture) {
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

#[test]
fn automated_gain_with_return_feeds_and_borrowed_pages_matches_resident_pcm_nodes_and_owners() {
    assert_mixed_pcm_and_observation(fixture_with_options(true, true));
}
#[test]
fn main_candidate_observation_streamed_pages_keeps_pcm_and_refusal_semantics() {
    let mut f = fixture_with_options(true, true);
    let owners = Arc::strong_count(&f.streamed);
    let mut observed = [[0.0; 2]; 512];
    let mut output = observed;
    let mut expected = output;
    let mut calls = 0;
    let mut actual = SessionPlayback::new(Arc::clone(&f.streamed)).unwrap();
    let mut plain = SessionPlayback::new(Arc::clone(&f.resident)).unwrap();
    actual.apply(SessionPlaybackCommand::Play).unwrap();
    plain.apply(SessionPlaybackCommand::Play).unwrap();
    let before = actual.performance_frame();
    assert!(
        actual
            .render_with_post_node_and_main_observers(
                &mut output,
                None,
                Some(&mut |_: &[Frame]| calls += 1)
            )
            .is_err()
    );
    assert_eq!(calls, 0);
    assert_eq!(actual.performance_frame(), before);
    actual
        .render_with_pages_post_node_and_main_observers(
            &mut output,
            &mut f.pages,
            None,
            Some(&mut |frames: &[Frame]| {
                calls += 1;
                observed.copy_from_slice(frames);
            }),
        )
        .unwrap();
    plain.render(&mut expected).unwrap();
    assert_eq!(calls, 1);
    assert_eq!(observed, output);
    assert_eq!(output, expected);
    assert_eq!(actual.performance_frame(), 512);
    drop(actual);
    assert_eq!(Arc::strong_count(&f.streamed), owners);
    let mut actual = hex_engine::Renderer::new(f.streamed.arrangement());
    let mut plain = hex_engine::Renderer::new(f.resident.arrangement());
    actual.play();
    plain.play();
    actual
        .render_with_pages_and_main_observer(
            &mut output,
            &mut f.pages,
            Some(&mut |frames: &[Frame]| observed.copy_from_slice(frames)),
        )
        .unwrap();
    plain.render(&mut expected).unwrap();
    assert_eq!(observed, output);
    assert_eq!(output, expected);
    assert_eq!(actual.position(), 512);
}

#[test]
fn ordinary_exact_demand_covers_return_feed_render_and_keeps_resident_pcm_and_clocks() {
    let mut f = fixture_with_options(true, true);
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 9,
    };
    let mut actual = Playback::from_prepared(Arc::clone(&f.streamed));
    let mut oracle = Playback::from_prepared(Arc::clone(&f.resident));
    for runtime in [&mut actual, &mut oracle] {
        runtime
            .try_apply_action(hex_engine::PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
    }
    struct Protected<'a> {
        pages: &'a mut Pages,
        extents: &'a [hex_engine::StreamResourceExtent],
    }
    impl MediaPages for Protected<'_> {
        fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
            assert!(
                self.extents
                    .iter()
                    .any(|extent| extent.source == key.into() && extent.page_index == index),
                "render read outside actual engine demand"
            );
            self.pages.page(key, index)
        }
        fn request_page(&mut self, _: StreamSourceKey, _: u64) {
            panic!("all demanded pages are installed")
        }
    }
    for command in [
        None,
        Some(SessionPlaybackCommand::LaunchClip {
            clip: f.clip,
            quantization: LaunchQuantization::Immediate,
        }),
        Some(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        }),
    ] {
        if let Some(command) = command {
            for runtime in [&mut actual, &mut oracle] {
                runtime
                    .try_apply_action(hex_engine::PlaybackAction::Session(command))
                    .unwrap();
            }
        }
        for count in [1, 3, 17, 257] {
            let mut expected = vec![[0.0; 2]; count];
            oracle.render(&mut expected).unwrap();
            let mut observed = vec![[0.0; 2]; count];
            let mut offset = 0;
            while offset < count {
                let clock = (actual.position(), actual.performance_frame());
                let mut extents = Vec::new();
                let window = actual
                    .stream_playback(context)
                    .visit_next_stream_chunk(count - offset, None, &mut |extent| {
                        extents.push(extent);
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(
                    window.frames,
                    (count - offset).min(256),
                    "this zero-latency return-feed fixture keeps the normal source quantum"
                );
                assert_eq!(
                    (actual.position(), actual.performance_frame()),
                    clock,
                    "demand is read-only"
                );
                let mut pages = Protected {
                    pages: &mut f.pages,
                    extents: &extents,
                };
                actual
                    .stream_playback(context)
                    .render_with_pages_and_signal_observers(
                        &mut observed[offset..offset + window.frames],
                        &mut pages,
                        None,
                        None,
                    )
                    .unwrap();
                offset += window.frames;
            }
            assert_eq!(observed, expected);
            assert_eq!(
                (actual.position(), actual.performance_frame()),
                (oracle.position(), oracle.performance_frame())
            );
        }
    }
    assert_eq!(f.pages.requests, 0);
}
