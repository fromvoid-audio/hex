//! Live original file-backed audio through the actual VST3 bridge.
use super::*;

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn verified_stream_through_native_main_effect_keeps_compensated_pcm() {
    verify_streamed_effect(EffectRoute::Main, AfterPlayback::NotRequested);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn verified_stream_through_native_track_effect_keeps_compensated_pcm() {
    verify_streamed_effect(EffectRoute::Track, AfterPlayback::NotRequested);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn verified_stream_through_native_return_effect_keeps_compensated_pcm() {
    verify_streamed_effect(EffectRoute::Return, AfterPlayback::NotRequested);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn verified_stream_native_main_effect_scope_resize_overlaps_pause() {
    verify_streamed_effect(EffectRoute::Main, AfterPlayback::ScopeResize);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn verified_stream_native_main_effect_renews_after_stop() {
    verify_streamed_effect(EffectRoute::Main, AfterPlayback::FixedLatency);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn verified_stream_native_track_effect_renews_after_stop() {
    verify_streamed_effect(EffectRoute::Track, AfterPlayback::FixedLatency);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn verified_stream_native_return_effect_renews_after_stop() {
    verify_streamed_effect(EffectRoute::Return, AfterPlayback::FixedLatency);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn verified_stream_native_main_effect_refuses_unannounced_latency_on_renewal() {
    const CHILD: &str = "HEX_DESKTOP_STREAM_LATENCY_REFUSAL_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let files = Files::new();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "68")
            .env("HEX_ORIGINAL_INSTANCE_TRACE", files.0.join("latency-trace"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    verify_streamed_effect(EffectRoute::Main, AfterPlayback::UnannouncedLatency);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AfterPlayback {
    NotRequested,
    FixedLatency,
    UnannouncedLatency,
    ScopeResize,
}

enum EffectRoute {
    Track,
    Return,
    Main,
}

fn source_sample(frame: usize) -> f32 {
    (4096 + 4 * (frame % 2048)) as f32 / 32768.0
}

fn verify_streamed_effect(location: EffectRoute, after: AfterPlayback) {
    let renew = matches!(
        after,
        AfterPlayback::FixedLatency | AfterPlayback::UnannouncedLatency
    );
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(48_000, 24_000);
    let mut original = fs::read(&path).unwrap();
    if renew {
        for (index, sample) in original[44..].chunks_exact_mut(2).enumerate() {
            let value = (4096 + 4 * (index % 2048)) as i16;
            sample.copy_from_slice(&value.to_le_bytes());
        }
        fs::write(&path, &original).unwrap();
    }
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
                EffectRoute::Return if renew => source_sample(index - 512) * (1.0 + 0.5 * scaled),
                EffectRoute::Return => 0.25 + 0.125 * scaled,
                EffectRoute::Track | EffectRoute::Main if renew => {
                    source_sample(index - 512) * scaled
                }
                EffectRoute::Track | EffectRoute::Main => 0.25 * scaled,
            }
        };
        assert_eq!(*frame, [expected; 2], "streamed frame {index}");
    }
    if after == AfterPlayback::ScopeResize {
        verify_scope_resize_overlaps_pause(&mut h, applied);
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
        assert_eq!(restored.state.project, accepted.state.project);
        assert_eq!(restored.state.revision, accepted.state.revision);
        if after == AfterPlayback::UnannouncedLatency {
            let error = restored
                .error
                .as_ref()
                .expect("changed latency must refuse renewal");
            assert!(error.contains("presentation latency refused"), "{error}");
            let status = h.audio.as_ref().unwrap().status().unwrap();
            assert_eq!(status.applied_revision.load(Acquire), applied);
            assert!(!status.playing.load(Acquire));
            for _ in 0..4 {
                h.pump();
                assert_eq!(h.render_plugin_block(), [[0.0; 2]; 256]);
            }
            let status = h.audio.as_ref().unwrap().status().unwrap();
            assert_eq!(status.applied_revision.load(Acquire), applied);
            // The original fixture records SDK control phases. Refusal must
            // follow actual quiescence without another construction or start.
            let observed =
                fs::read(std::env::var_os("HEX_ORIGINAL_INSTANCE_TRACE").unwrap()).unwrap();
            let observed: Vec<i32> = observed
                .chunks_exact(4)
                .map(|bytes| i32::from_ne_bytes(bytes.try_into().unwrap()))
                .collect();
            assert_eq!(observed.len(), 14);
            assert_eq!(observed[1], 0, "no SDK thread/lifecycle violation");
            assert_eq!(observed[2], 1, "same original processor");
            assert_eq!(
                (observed[8], observed[9]),
                (1, 1),
                "one start and completed SDK Stop"
            );
            assert!(observed[10] >= 10, "initial processing actually ran");
        } else {
            assert!(
                restored.error.is_none(),
                "streamed renewal: {:?}",
                restored.error
            );
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
                    .is_some_and(|next| {
                        next.instance == binding.instance && next.epoch > binding.epoch
                    })
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
            let effect_gain = |sample: f32, source: f32| match location {
                EffectRoute::Return => (sample / source - 1.0) * 2.0,
                EffectRoute::Track | EffectRoute::Main => sample / source,
            };
            let gain = effect_gain(resumed[512][0], source_sample(0));
            assert!(
                gain > effect_gain(pcm[2304][0], source_sample(2304 - 512)),
                "the original processor sequence must continue"
            );
            for (index, frame) in resumed.iter().enumerate() {
                let expected = if index < 512 {
                    0.0
                } else {
                    let source = index - 512;
                    let scaled = gain + (source / 256) as f32 * 0.125;
                    match location {
                        EffectRoute::Return => source_sample(source) * (1.0 + 0.5 * scaled),
                        EffectRoute::Track | EffectRoute::Main => source_sample(source) * scaled,
                    }
                };
                assert_eq!(*frame, [expected; 2], "renewed streamed frame {index}");
            }
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

fn verify_scope_resize_overlaps_pause(h: &mut ResidentHarness, applied: u64) {
    let status = |h: &ResidentHarness| {
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert_eq!(status.rejected_revision(), None);
        assert_eq!(status.rejected_action(), None);
        (
            status.applied_revision.load(Acquire),
            status.playing.load(Acquire),
            status.position.load(Acquire),
        )
    };
    let command = |h: &mut ResidentHarness, command| {
        h.audio.as_mut().unwrap().control.command(command).unwrap();
    };
    command(h, PlaybackCommand::Pause);
    instruments::render_completed_block(h);
    assert!(!status(h).1);
    // The callback consumed the incident's Play From before attempting the
    // graph handoff. Enter the range before queueing the successor here too.
    command(h, PlaybackCommand::PlayFrom(4992));
    let deadline = Instant::now() + Duration::from_secs(1);
    while !status(h).1 {
        h.pump();
        instruments::render_completed_block(h);
        assert!(
            Instant::now() < deadline,
            "play from inside the new scope remained pending"
        );
    }

    let first = h.run(Request::Edit(Command::SetLoopRegion {
        start: 4096,
        end: 7168,
        enabled: true,
    }));
    assert!(first.error.is_none(), "{:?}", first.error);
    assert_eq!(status(h).0, applied);
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .publication_in_flight()
    );
    instruments::render_completed_block(h);
    command(h, PlaybackCommand::Pause);
    let deadline = Instant::now() + Duration::from_secs(1);
    let (first_revision, paused_position) = loop {
        let (revision, playing, position) = status(h);
        if revision > applied && !playing {
            break (revision, position);
        }
        h.pump();
        instruments::render_completed_block(h);
        assert!(Instant::now() < deadline, "scope or pause remained pending");
    };
    assert!(paused_position >= 4992, "pause must preserve the playhead");

    let next = h.run(Request::Edit(Command::SetLoopRegion {
        start: 4096,
        end: 8192,
        enabled: true,
    }));
    assert!(next.error.is_none(), "{:?}", next.error);
    assert_eq!(next.state.revision, first.state.revision + 1);
    assert_eq!(
        next.state.project.loop_region(),
        Some(hex_project::LoopRegion::new(4096, 8192, true).unwrap())
    );
    h.applied(first_revision);
    command(h, PlaybackCommand::Play);
    let deadline = Instant::now() + Duration::from_secs(1);
    while !instruments::render_completed_block(h)
        .iter()
        .any(|frame| frame[0] != 0.0)
    {
        assert!(
            Instant::now() < deadline,
            "streamed playback did not resume"
        );
    }
    let (_, playing, position) = status(h);
    assert!(playing);
    assert!(position > paused_position);
}
