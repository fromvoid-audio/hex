//! Check the actual Session tempo edit against the hosted worker's PCM.
use super::{
    instruments::{render_completed_block, render_replay_block},
    *,
};
use hex_engine::playback::PlaybackCommand;
use hex_project::Tempo;

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn running_tempo_edit_preserves_hosted_pcm_and_beat() {
    let _native = native_fixture();
    for streamed in [false, true] {
        run(streamed);
    }
}

fn start_playback(files: &Files, streamed: bool, looped: bool) -> (ResidentHarness, Event, u64) {
    let (mut h, accepted, revision) = prepare_graph(files, streamed, looped, false);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    (h, accepted, revision)
}

fn prepare_graph(
    files: &Files,
    streamed: bool,
    looped: bool,
    upstream_limiter: bool,
) -> (ResidentHarness, Event, u64) {
    let frames = if streamed { 96_000 } else { 8192 };
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let mut imported = h.run(Request::Import(files.wav(48_000, frames)));
    let (mut imported_revision, _) = h.applied(initial);
    let track = imported.state.project.tracks()[0].id();
    assert!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .required_sources()
            .all(|(_, source)| {
                matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)) == streamed
            })
    );
    if looped {
        imported = h.run(Request::Edit(Command::SetLoopRegion {
            start: 768,
            end: 2304,
            enabled: true,
        }));
        (imported_revision, _) = h.applied(imported_revision);
    }
    if upstream_limiter {
        let limiter = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_engine::DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap();
        imported = h.run(Request::Edit(Command::InsertDevice {
            track,
            before: None,
            state: limiter,
        }));
        (imported_revision, _) = h.applied(imported_revision);
    }
    h.service
        .submit(Request::InsertPluginEffect(request(
            &imported.state,
            PluginEffectDestination::Track(track),
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    let (applied, _) = h.applied(imported_revision);

    (h, accepted, applied)
}

fn run(streamed: bool) {
    let files = Files::new();
    let (mut h, accepted, applied) = start_playback(&files, streamed, false);
    let original_map = accepted.state.project.tempo_map().clone();
    let mut ordinal = 0;
    let ready = Instant::now() + Duration::from_secs(2);
    while ordinal < 6 {
        let block = render_completed_block(&h);
        let status = h.audio.as_ref().unwrap().status().unwrap();
        if status.stream_buffering.load(Acquire) {
            assert_eq!(block, [[0.0; 2]; 256]);
            assert_eq!(status.performance_frame.load(Acquire), ordinal as u64 * 256);
            assert!(
                Instant::now() < ready,
                "initial audio pages did not become ready"
            );
            continue;
        }
        assert_pcm(ordinal, &block);
        ordinal += 1;
    }

    h.service
        .submit(Request::Edit(Command::SetProjectTempo {
            tempo: Tempo::new(90_000_000).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    let mut edited = None;
    let mut cut = None;
    while edited.is_none() || cut.is_none() {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            if event.state.revision > accepted.state.revision {
                edited = Some(event);
            }
        }
        let status = h.audio.as_ref().unwrap().status().unwrap();
        let previous_revision = status.applied_revision.load(Acquire);
        let previous_position = status.position.load(Acquire);
        let pcm = render_replay_block(&h);
        std::thread::sleep(Duration::from_millis(2));
        if previous_revision == applied && status.applied_revision.load(Acquire) > applied {
            cut = Some((previous_position, status.position.load(Acquire)));
        }
        assert!(
            !status.stream_buffering.load(Acquire),
            "tempo edit interrupted the previous playback at frame {previous_position}"
        );
        assert_pcm(ordinal, &pcm);
        ordinal += 1;
        assert!(
            Instant::now() < until,
            "tempo edit did not reach the callback"
        );
    }
    let (before, after) = cut.unwrap();
    let beat = original_map.frame_to_beat(before).unwrap();
    let relocated = edited
        .unwrap()
        .state
        .project
        .tempo_map()
        .beat_to_frame(beat)
        .unwrap();
    assert_eq!(after, relocated + 256);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire) && status.rejected_revision().is_none());
    for _ in 0..3 {
        assert_pcm(ordinal, &render_completed_block(&h));
        ordinal += 1;
    }
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn delayed_effect_automation_follows_the_source_clock_at_a_live_tempo_cut() {
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, DeviceAutomationTarget,
    };
    use hex_session::service::PluginParameterQuery;
    const CHILD: &str = "HEX_DESKTOP_TEST_DELAYED_TEMPO_EFFECT";
    if std::env::var_os(CHILD).is_none() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "144")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(
            result.status.success() && stdout.contains("1 passed"),
            "{stdout}
{}",
            String::from_utf8_lossy(&result.stderr)
        );
        print!("{stdout}");
        return;
    }
    let _native = native_fixture();
    let files = Files::new();
    let (mut h, installed, revision) = prepare_graph(&files, false, false, true);
    let device = installed.state.project.tracks()[0]
        .devices()
        .last()
        .unwrap()
        .id();
    let context = h.submitted.unwrap().1.context;
    let output_identity = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.run(Request::QueryPluginParameters(PluginParameterQuery {
        context,
        device,
        output_identity,
        request_id: 10,
        page_first: None,
    }));
    let rows = h.next().plugin_parameters.unwrap().rows.unwrap();
    let gain = rows
        .iter()
        .find(|row| row.name.eq_ignore_ascii_case("gain"))
        .unwrap();
    let end = BeatTime::from_ratio(1, 4).unwrap();
    let points = [(BeatTime::ZERO, 0.1), (end, 0.8)]
        .map(|(beat, value)| AutomationPoint {
            beat,
            value: AutomationValue::new(value).unwrap(),
            outgoing: AutomationInterpolation::Linear,
        })
        .to_vec();
    let authored = super::automation::edit_with_running_callback(
        &mut h,
        Request::DeviceAutomation {
            project_generation: context.project_generation,
            revision: context.editor_revision,
            device,
            target: DeviceAutomationTarget::Vst3 { parameter: gain.id },
            vst3_class: gain.automation_class,
            points,
        },
    );
    assert!(authored.error.is_none(), "{:?}", authored.error);
    let applied = h.applied(revision).0;
    let stage = h
        .submitted_observation()
        .unwrap()
        .processor_latency(
            device.value(),
            installed
                .state
                .project
                .device(device)
                .unwrap()
                .state()
                .type_id(),
        )
        .unwrap();
    assert!(stage.upstream_frames > 0);
    assert_eq!(stage.stage_frames, 512);
    let latency = u64::from(stage.upstream_frames + stage.stage_frames + stage.downstream_frames);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..5 {
        render_completed_block(&h);
    }
    let binding = |h: &ResidentHarness| {
        h.plugin_progress
            .lock()
            .unwrap()
            .source_bindings
            .iter()
            .flatten()
            .find(|(id, _, _)| *id == device.value())
            .map(|(_, binding, _)| *binding)
            .expect("active effect binding")
    };
    let original = binding(&h);
    let phase = h
        .callback
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .native_worker_phase_for_test()
        .unwrap();
    let effect_cut_phase = (u32::try_from(phase).unwrap() + stage.upstream_frames) % 256;
    assert_ne!(effect_cut_phase, 0);
    let origin = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .performance_frame
        .load(Acquire);
    h.service
        .submit(Request::Edit(Command::SetProjectTempo {
            tempo: Tempo::new(90_000_000).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    let mut cut = None;
    let mut edited = None;
    let mut pcm = Vec::new();
    while edited.is_none() || cut.is_none() {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            if event.state.revision > authored.state.revision {
                edited = Some(event);
            }
        }
        let status = h.audio.as_ref().unwrap().status().unwrap();
        let before = status.applied_revision.load(Acquire);
        let performance = status.performance_frame.load(Acquire);
        let cursor = status.position.load(Acquire);
        // Source preparation can wait for a callback acknowledgement.
        // Pace the callback while the tempo edit is still in flight.
        pcm.extend(render_replay_block(&h));
        assert_eq!(status.performance_frame.load(Acquire), performance + 256);
        if before == applied && status.applied_revision.load(Acquire) > applied {
            cut = Some((performance, cursor, status.position.load(Acquire)));
        }
        assert!(
            Instant::now() < until,
            "tempo edit did not reach the callback"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    for _ in 0..5 {
        pcm.extend(render_completed_block(&h));
    }
    let (performance, old_cursor, new_after) = cut.unwrap();
    let old = authored.state.project.tempo_map();
    let edited = edited.unwrap();
    let new = edited.state.project.tempo_map();
    let new_cursor = new
        .beat_to_frame(old.frame_to_beat(old_cursor).unwrap())
        .unwrap();
    assert_eq!(new_after, new_cursor + 256);
    let old_end = old.beat_to_frame(end).unwrap();
    let new_end = new.beat_to_frame(end).unwrap();
    for (index, frame) in pcm.iter().enumerate() {
        let source = origin + index as u64 - latency;
        let progress = if source < performance {
            source as f64 / old_end as f64
        } else {
            (new_cursor + source - performance) as f64 / new_end as f64
        };
        let expected = (0.1 + 0.7 * progress.min(1.0)) as f32;
        assert!(
            frame.iter().all(|sample| (sample - expected).abs() < 2e-5),
            "source {source} cut {performance} latency {latency}: {frame:?}, expected {expected}"
        );
    }
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        Some(output_identity)
    );
    assert_eq!(binding(&h), original);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.playing.load(Acquire) && status.rejected_revision().is_none());
    assert!(status.render_fault_snapshot().is_none());
    println!(
        "delayed effect tempo cut performance={performance} old={old_cursor} new={new_cursor} latency={latency} effect_phase={effect_cut_phase}"
    );
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn running_loop_tempo_edit_keeps_hosted_pcm_after_a_wrap() {
    let _native = native_fixture();
    for streamed in [false, true] {
        let render_from_start = |h: &ResidentHarness, ordinal: u64| {
            let until = Instant::now() + Duration::from_secs(2);
            loop {
                let pcm = render_completed_block(h);
                let status = h.audio.as_ref().unwrap().status().unwrap();
                if !status.stream_buffering.load(Acquire) {
                    assert_eq!(status.performance_frame.load(Acquire), (ordinal + 1) * 256);
                    return pcm;
                }
                assert_eq!(pcm, [[0.0; 2]; 256]);
                assert_eq!(status.performance_frame.load(Acquire), ordinal * 256);
                assert!(
                    Instant::now() < until,
                    "initial loop pages did not become ready"
                );
            }
        };
        // Capture the uninterrupted oracle first. The native source holds
        // the exclusive output lease for the lifetime of the harness.
        let expected: Vec<_> = {
            let files = Files::new();
            let (uninterrupted, _, _) = start_playback(&files, streamed, true);
            (0..128)
                .map(|ordinal| render_from_start(&uninterrupted, ordinal))
                .collect()
        };
        let files = Files::new();
        let (mut playback, applied, revision) = start_playback(&files, streamed, true);
        for (ordinal, expected) in expected.iter().take(12).enumerate() {
            let pcm = render_from_start(&playback, ordinal as u64);
            if ordinal == 2 {
                assert_pcm(2, &pcm);
            }
            assert_eq!(&pcm, expected, "loop warm-up block {ordinal}");
        }
        assert_eq!(
            playback
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .position
                .load(Acquire),
            1536
        );
        let old_tempo = applied.state.project.tempo_map();
        playback
            .service
            .submit(Request::Edit(Command::SetProjectTempo {
                tempo: Tempo::new(90_000_000).unwrap(),
            }))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        let mut terminal = None;
        let mut cut = None;
        let mut held_blocks = 2;
        while held_blocks > 0 || terminal.is_none() {
            playback.pump();
            if let Ok(event) = playback.service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                if event.state.revision > applied.state.revision {
                    terminal = Some(event);
                }
            }
            let status = playback.audio.as_ref().unwrap().status().unwrap();
            let before_position = status.position.load(Acquire);
            let before_frame = status.performance_frame.load(Acquire);
            let before_revision = status.applied_revision.load(Acquire);
            let pcm = render_replay_block(&playback);
            std::thread::sleep(Duration::from_millis(2));
            assert!(!status.stream_buffering.load(Acquire));
            assert_eq!(status.performance_frame.load(Acquire), before_frame + 256);
            if before_revision == revision && status.applied_revision.load(Acquire) > revision {
                cut = Some((before_position, before_frame));
            }
            if held_blocks > 0 {
                assert_eq!(
                    pcm,
                    *expected
                        .get((before_frame / 256) as usize)
                        .expect("tempo edit exceeded the uninterrupted PCM oracle"),
                    "in-flight PCM at frame {before_frame}, cut {cut:?}"
                );
                if cut.is_some() {
                    held_blocks -= 1;
                }
            }
            assert!(Instant::now() < until, "loop tempo revision did not commit");
        }

        let (old_position, cut_frame) = cut.unwrap();
        let terminal = terminal.unwrap();
        let edited = &terminal.state.project;
        let status = playback.audio.as_ref().unwrap().status().unwrap();
        let region = edited.loop_region().unwrap();
        assert_eq!((region.start(), region.end()), (1024, 3072));
        let new_position = edited
            .tempo_map()
            .beat_to_frame(old_tempo.frame_to_beat(old_position).unwrap())
            .unwrap();
        let actual_cursor = |frames: u64| {
            region.start()
                + (new_position - region.start() + frames) % (region.end() - region.start())
        };
        assert_eq!(
            status.position.load(Acquire),
            actual_cursor(status.performance_frame.load(Acquire) - cut_frame)
        );
        for _ in 0..9 {
            let pcm = render_completed_block(&playback);
            assert!(
                pcm.iter().any(|frame| *frame != [0.0; 2]),
                "a full quantum was silent"
            );
            assert_eq!(
                status.position.load(Acquire),
                actual_cursor(status.performance_frame.load(Acquire) - cut_frame)
            );
        }
        assert!(status.playing.load(Acquire) && status.rejected_revision().is_none());
    }
}

fn assert_pcm(ordinal: usize, block: &[hex_engine::Frame; 256]) {
    // The original effect fixture increases its gain on every successful SDK
    // call. The input is 0.25 and the bridge delays it by exactly two quanta.
    let expected = if ordinal < 2 {
        0.0
    } else {
        0.25 * (0.5 + (ordinal - 2) as f32 * 0.125)
    };
    for (offset, frame) in block.iter().enumerate() {
        assert_eq!(*frame, [expected; 2], "block {ordinal}, offset {offset}");
    }
}
