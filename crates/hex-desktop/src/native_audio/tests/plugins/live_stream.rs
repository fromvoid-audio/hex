//! Live original file-backed audio through the actual VST3 bridge.
use super::*;

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn verified_stream_through_native_main_effect_keeps_compensated_pcm() {
    verify_streamed_effect(EffectRoute::Main, false);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn verified_stream_through_native_track_effect_keeps_compensated_pcm() {
    verify_streamed_effect(EffectRoute::Track, false);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn verified_stream_through_native_return_effect_keeps_compensated_pcm() {
    verify_streamed_effect(EffectRoute::Return, false);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn verified_stream_native_main_effect_renews_after_stop() {
    verify_streamed_effect(EffectRoute::Main, true);
}

enum EffectRoute {
    Track,
    Return,
    Main,
}

fn verify_streamed_effect(location: EffectRoute, renew: bool) {
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(48_000, 24_000);
    let original = fs::read(&path).unwrap();
    // The source exceeds this harness's 8,192-frame resident limit.
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let imported = h.run(Request::Import(path.clone()));
    assert!(imported.error.is_none(), "{:?}", imported.error);
    let source = imported.state.canonical_audio_source.as_ref().unwrap();
    assert!(
        source
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)))
    );
    let (mut applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let (state, destination) = match location {
        EffectRoute::Track => (
            imported.state.clone(),
            PluginEffectDestination::Track(track),
        ),
        EffectRoute::Return => {
            let added = h.run(Request::Edit(Command::AddReturnTrack {
                name: "effect".into(),
            }));
            (applied, _) = h.applied(applied);
            let Some(Created::ReturnTrack(bus)) = added.created else {
                panic!("return")
            };
            let state = h
                .run(Request::Edit(Command::SetReturnSend {
                    track,
                    return_track: bus,
                    gain: 0.5,
                }))
                .state;
            (applied, _) = h.applied(applied);
            (state, PluginEffectDestination::Return(bus))
        }
        EffectRoute::Main => (imported.state.clone(), PluginEffectDestination::Main),
    };
    h.service
        .submit(Request::InsertPluginEffect(request(&state, destination)))
        .unwrap();
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    let (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();

    let mut pcm = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while pcm.len() < 2_560 {
        let block = instruments::render_completed_block(&h);
        let status = h.audio.as_ref().unwrap().status().unwrap();
        if status.stream_buffering.load(Acquire) {
            assert_eq!(block, [[0.0; 2]; 256]);
            assert_eq!(status.performance_frame.load(Acquire), pcm.len() as u64);
            assert!(
                Instant::now() < deadline,
                "stream pages did not become ready"
            );
            std::thread::yield_now();
        } else {
            pcm.extend(block);
        }
    }
    // The SDK fixture scales each processed block by 0.5 + block/8, while
    // the copied bridge contributes two 256-frame quanta of initial silence.
    for (index, frame) in pcm.iter().enumerate() {
        let expected = if index < 512 {
            0.0
        } else {
            let scaled = 0.5 + ((index - 512) / 256) as f32 * 0.125;
            match location {
                EffectRoute::Return => 0.25 + 0.125 * scaled,
                EffectRoute::Track | EffectRoute::Main => 0.25 * scaled,
            }
        };
        assert_eq!(*frame, [expected; 2], "streamed frame {index}");
    }
    if renew {
        let old = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .native_effect_source()
            .unwrap();
        let binding = h.plugin_progress.lock().unwrap().binding.unwrap();
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Stop)
            .unwrap();
        h.render_plugin_block();
        assert!(
            !h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
        h.service.submit(Request::RestoreStoppedPlayback).unwrap();
        let restored = restored_terminal(&mut h);
        assert!(
            restored.error.is_none(),
            "streamed renewal: {:?}",
            restored.error
        );
        assert_eq!(restored.state.project, accepted.state.project);
        let (fresh, metadata) = h.applied(applied);
        assert!(metadata.stopped_restore);
        let new = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .native_effect_source()
            .unwrap();
        assert_eq!(
            (new.output_identity, new.context, new.audio_revision),
            (old.output_identity, old.context, fresh)
        );
        wait(|| {
            h.plugin_progress
                .lock()
                .unwrap()
                .binding
                .is_some_and(|next| next.instance == binding.instance && next.epoch > binding.epoch)
        });
        wait(|| {
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::PlayFrom(0))
                .is_ok()
        });
        let mut resumed = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        while resumed.len() < 1536 {
            let block = instruments::render_completed_block(&h);
            let status = h.audio.as_ref().unwrap().status().unwrap();
            if status.stream_buffering.load(Acquire) {
                assert_eq!(block, [[0.0; 2]; 256]);
                assert_eq!(status.performance_frame.load(Acquire), resumed.len() as u64);
                assert!(
                    Instant::now() < deadline,
                    "stream pages did not become ready after renewal"
                );
                std::thread::yield_now();
            } else {
                resumed.extend(block);
            }
        }
        let first = resumed[512][0];
        assert!(
            first > pcm[2304][0],
            "the original processor sequence must continue"
        );
        for (index, frame) in resumed.iter().enumerate() {
            let expected = if index < 512 {
                0.0
            } else {
                first + ((index - 512) / 256) as f32 * 0.03125
            };
            assert_eq!(*frame, [expected; 2], "renewed streamed frame {index}");
        }
    }
    assert_eq!(fs::read(path).unwrap(), original);
    assert_eq!(
        accepted.state.project.tracks()[0].clips(),
        imported.state.project.tracks()[0].clips()
    );
    assert_eq!(
        accepted.state.project.assets().collect::<Vec<_>>(),
        imported.state.project.assets().collect::<Vec<_>>()
    );
}
