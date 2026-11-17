//! Announced latency changes rebuild the original source and parallel PDC.
use super::*;

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn native_latency_restart_rebuilds_compensation_on_the_original_source() {
    run(false, Route::Track);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn streamed_latency_restart_rebuilds_compensation_on_the_original_source() {
    run(true, Route::Track);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn streamed_return_latency_restart_preserves_parallel_impulse_alignment() {
    run(true, Route::Return);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn streamed_main_latency_restart_preserves_summed_impulse_origin() {
    run(true, Route::Main);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn streamed_serial_latency_restart_keeps_both_workers_and_parallel_alignment() {
    run(true, Route::SerialTrack);
}

#[derive(Clone, Copy, Debug)]
enum Route {
    Track,
    SerialTrack,
    Return,
    Main,
}

fn run(streamed: bool, route: Route) {
    const CHILD: &str = "HEX_DESKTOP_ANNOUNCED_LATENCY_CHILD";
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
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "105")
            .env("HEX_ORIGINAL_INSTANCE_RELEASE", files.0.join("release"))
            .env("HEX_ORIGINAL_INSTANCE_TRACE", files.0.join("trace"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("1 passed"));
        for line in stdout
            .lines()
            .filter(|line| line.starts_with("latency_restart,"))
        {
            println!("{line}");
        }
        return;
    }
    let _native = native_fixture();
    assert_eq!(
        std::env::var("HEX_ORIGINAL_INSTANCE_VARIANT").as_deref(),
        Ok("105")
    );
    let release = PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap());
    assert!(!release.exists());
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let files = Files::new();
    let path = files.wav(48_000, if streamed { 24_000 } else { 4096 });
    let mut wav = fs::read(&path).unwrap();
    wav[44..].fill(0);
    wav[44 + 256 * 2..44 + 257 * 2].copy_from_slice(&8192i16.to_le_bytes());
    fs::write(&path, &wav).unwrap();
    h.run(Request::Import(path.clone()));
    let (applied, _) = h.applied(applied);
    let mut state = h.run(Request::Import(path.clone())).state;
    let (mut applied, _) = h.applied(applied);
    assert_eq!(state.project.tracks().len(), 2);
    let sources = state.canonical_audio_source.as_ref().unwrap();
    assert!(sources.required_sources().next().is_some());
    assert!(sources.required_sources().all(|(_, source)| {
        matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)) == streamed
    }));
    if streamed {
        assert_eq!(
            sources
                .candidate_reservation_quote()
                .unwrap()
                .prepared_pcm_frames,
            0
        );
    }
    let track = state.project.tracks()[0].id();
    let destination = match route {
        Route::Track | Route::SerialTrack => PluginEffectDestination::Track(track),
        Route::Main => PluginEffectDestination::Main,
        Route::Return => {
            let added = h.run(Request::Edit(Command::AddReturnTrack {
                name: "latency".into(),
            }));
            (applied, _) = h.applied(applied);
            let Some(Created::ReturnTrack(return_track)) = added.created else {
                panic!("return track")
            };
            state = h
                .run(Request::Edit(Command::SetReturnSend {
                    track,
                    return_track,
                    gain: 0.5,
                }))
                .state;
            (applied, _) = h.applied(applied);
            PluginEffectDestination::Return(return_track)
        }
    };
    h.service
        .submit(Request::InsertPluginEffect(request(&state, destination)))
        .unwrap();
    assert!(h.next().error.is_none());
    let mut accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    (applied, _) = h.applied(applied);
    if matches!(route, Route::SerialTrack) {
        h.service
            .submit(Request::InsertPluginEffect(request(
                &accepted.state,
                PluginEffectDestination::Track(track),
            )))
            .unwrap();
        assert!(h.next().error.is_none());
        accepted = terminal(&mut h);
        assert!(accepted.error.is_none(), "{:?}", accepted.error);
        assert!(accepted.plugin_effect_complete[0].accepted);
        (applied, _) = h.applied(applied);
    }
    let source_count = if matches!(route, Route::SerialTrack) {
        2
    } else {
        1
    };
    wait(|| {
        h.plugin_progress
            .lock()
            .unwrap()
            .source_bindings
            .iter()
            .flatten()
            .count()
            == source_count
    });
    let bindings: Vec<_> = h
        .plugin_progress
        .lock()
        .unwrap()
        .source_bindings
        .into_iter()
        .flatten()
        .map(|(device, binding, _)| (device, binding))
        .collect();
    if source_count == 2 {
        assert_ne!(bindings[0].1.instance, bindings[1].1.instance);
        assert_eq!(
            bindings
                .iter()
                .map(|(device, _)| *device)
                .collect::<Vec<_>>(),
            accepted.state.project.tracks()[0]
                .devices()
                .iter()
                .map(|device| device.id().value())
                .collect::<Vec<_>>()
        );
    }
    let audio = h.audio.as_mut().unwrap();
    let before = audio.status().unwrap().native_effect_source().unwrap();
    audio.control.command(PlaybackCommand::Play).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while h.plugin_completed.load(Acquire) == 0 {
        // Real file pages can still be arriving. A waiting callback may not
        // move the musical clock or stand in for a completed foreign block.
        assert_eq!(instruments::render_completed_block(&h), [[0.0; 2]; 256]);
        assert!(Instant::now() < deadline, "initial stream did not advance");
    }
    assert_eq!(h.plugin_completed.load(Acquire), 1);
    wait(|| h.plugin_progress.lock().unwrap().observed_latency_receipt);
    let binding = h.plugin_progress.lock().unwrap().binding.unwrap();

    fs::write(release, b"request latency restart").unwrap();
    wait(|| {
        h.pump();
        h.latency_restart == Some(before)
    });
    let audio = h.audio.as_mut().unwrap();
    assert!(audio.status().unwrap().playing.load(Acquire));
    audio.control.command(PlaybackCommand::Stop).unwrap();
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
    wait(|| {
        h.pump();
        let progress = h.plugin_progress.lock().unwrap();
        bindings.iter().all(|(device, binding)| {
            progress
                .source_bindings
                .iter()
                .flatten()
                .any(|(next_device, next, count)| {
                    next_device == device
                        && next == binding
                        && count.is_some_and(|count| count >= 1)
                })
        })
    });
    let completed_before_stop: Vec<_> = {
        let progress = h.plugin_progress.lock().unwrap();
        bindings
            .iter()
            .map(|(device, binding)| {
                progress
                    .source_bindings
                    .iter()
                    .flatten()
                    .find(|(next_device, next, _)| next_device == device && next == binding)
                    .unwrap()
                    .2
                    .unwrap() as usize
            })
            .collect()
    };
    h.service.submit(Request::RestoreStoppedPlayback).unwrap();
    let restored = restored_terminal(&mut h);
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!(restored.state.revision, accepted.state.revision);
    assert_eq!(restored.state.project, accepted.state.project);
    let (fresh, metadata) = h.applied(applied);
    assert!(metadata.stopped_restore);
    let audio = h.audio.as_mut().unwrap();
    let after = audio.status().unwrap().native_effect_source().unwrap();
    assert_eq!(after.audio_revision, fresh);
    assert_eq!(after.context, before.context);
    assert_eq!(after.output_identity, before.output_identity);
    wait(|| {
        h.plugin_progress
            .lock()
            .unwrap()
            .binding
            .is_some_and(|updated| {
                updated.instance == binding.instance && updated.epoch > binding.epoch
            })
            && h.plugin_completed.load(Acquire) == 0
    });
    wait(|| {
        let progress = h.plugin_progress.lock().unwrap();
        bindings.iter().all(|(device, old)| {
            progress
                .source_bindings
                .iter()
                .flatten()
                .any(|(next_device, next, _)| {
                    next_device == device
                        && next.root == old.root
                        && next.instance == old.instance
                        && next.graph == old.graph
                        && next.epoch > old.epoch
                })
        })
    });
    for (_, _, count) in h
        .plugin_progress
        .lock()
        .unwrap()
        .source_bindings
        .into_iter()
        .flatten()
    {
        assert_eq!(count, Some(0));
    }
    // Actual callback installation can precede the worker's fence release.
    wait(|| {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::PlayFrom(0))
            .is_ok()
    });
    // Control's exact per-binding prefixes and the independent SDK block law
    // determine each serial gain; no gain is inferred from rendered samples.
    // The SDK snapshot corroborates one helper's actual quiescent phase.
    let observed = fs::read(std::env::var_os("HEX_ORIGINAL_INSTANCE_TRACE").unwrap()).unwrap();
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
        "renewed facts precede the second processing start"
    );
    assert!(observed[10] >= 1);
    assert!(
        completed_before_stop.contains(&(observed[10] as usize)),
        "SDK snapshot agrees with an original worker prefix"
    );
    let mut pcm = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    let output_frames = if source_count == 2 { 1536 } else { 1024 };
    while pcm.len() < output_frames {
        let block = instruments::render_completed_block(&h);
        let status = h.audio.as_ref().unwrap().status().unwrap();
        if status.stream_buffering.load(Acquire) {
            assert_eq!(block, [[0.0; 2]; 256]);
            assert_eq!(status.performance_frame.load(Acquire), pcm.len() as u64);
            assert!(Instant::now() < deadline, "renewed stream did not advance");
            std::thread::yield_now();
        } else {
            pcm.extend(block);
        }
    }
    let nonzero: Vec<_> = pcm
        .iter()
        .enumerate()
        .filter(|(_, frame)| **frame != [0.0; 2])
        .collect();
    // Both tracks have the same input impulse. Track/Return compensation
    // aligns dry paths with the wet path; Main delays their complete sum.
    // The bridge and seven algorithmic frames must each be counted once.
    assert_eq!(nonzero.len(), 1, "{nonzero:?}");
    let mut wet_frame = 256;
    let mut wet_gain = 1.0;
    for completed in &completed_before_stop {
        wet_gain *= 0.5 + (completed + wet_frame / 256) as f32 * 0.125;
        wet_frame += 512 + 7;
    }
    assert_eq!(nonzero[0].0, wet_frame);
    let expected = match route {
        Route::Track | Route::SerialTrack => 0.25 + 0.25 * wet_gain,
        Route::Return => 0.5 + 0.125 * wet_gain,
        Route::Main => 0.5 * wet_gain,
    };
    assert_eq!(
        *nonzero[0].1, [expected; 2],
        "parallel dry and original wet amplitudes"
    );
    assert_eq!(fs::read(&path).unwrap(), wav);
    assert_eq!(
        restored.state.project.tracks()[0].clips(),
        state.project.tracks()[0].clips()
    );
    assert_eq!(
        restored.state.project.assets().collect::<Vec<_>>(),
        state.project.assets().collect::<Vec<_>>()
    );
    println!(
        "latency_restart,streamed={streamed},route={route:?},sdk_before_resume={observed:?},worker_prefixes={completed_before_stop:?},peak_frame={},peak={expected}",
        nonzero[0].0
    );
}
