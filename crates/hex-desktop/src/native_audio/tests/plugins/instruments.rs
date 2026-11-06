//! Native instrument insertion through the real Session and desktop worker.
use super::*;
use hex_engine::{LaunchQuantization::Immediate, SessionPlaybackCommand};
use hex_project::BeatTime;

fn render_completed_block(harness: &ResidentHarness) -> [hex_engine::Frame; 256] {
    let previous = harness.plugin_completed.load(Acquire);
    let frames = harness.render_plugin_block();
    let deadline = Instant::now() + Duration::from_millis(800);
    while harness.plugin_completed.load(Acquire) <= previous {
        let status = harness.audio.as_ref().unwrap().status().unwrap();
        assert!(
            Instant::now() < deadline,
            "worker did not complete next block after {previous}; playing={} source={:?} fault={:?}",
            status.playing.load(Acquire),
            status.native_effect_source(),
            status.render_fault_snapshot(),
        );
        std::thread::yield_now();
    }
    frames
}

#[test]
#[ignore = "requires explicit source-bound SDK receipts and optional installed catalog"]
fn instrument_insertion_plays_authored_midi_in_the_desktop_audio_graph() {
    const NAME: &str = "native_audio::tests::plugins::instruments::instrument_insertion_plays_authored_midi_in_the_desktop_audio_graph";
    const CHILD: &str = "HEX_DESKTOP_TEST_INSTRUMENT_CHILD";
    let installed = std::env::var("HEX_TEST_VST3_INSTRUMENT_BUNDLE").ok();
    if std::env::var_os(CHILD).is_none() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", NAME, "--ignored", "--nocapture"])
            .env(CHILD, "1");
        if installed.is_none() {
            child.env("HEX_ORIGINAL_INSTANCE_VARIANT", "127");
        }
        let result = child.output().unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(
            result.status.success() && output.contains("1 passed"),
            "{output}\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        print!("{output}");
        return;
    }
    let _native = native_fixture();
    let controlled_fixture = installed.is_none();
    // Installed drum instruments may map their default sounds below middle C.
    let installed_pitch = installed.as_ref().and_then(|_| {
        std::env::var("HEX_TEST_VST3_INSTRUMENT_PITCH")
            .ok()
            .map(|pitch| pitch.parse::<u8>().expect("installed instrument pitch"))
    });
    let pitch = installed_pitch.unwrap_or(60);
    let rate = if installed.is_some() { 44_100 } else { 48_000 };
    let mut harness = ResidentHarness::with_plugins_at_rate(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        rate,
    );
    assert!(harness.next().error.is_none());
    let (mut applied, _) = harness.applied(0);
    let add = harness.run(Request::Edit(Command::AddMidiTrack {
        name: "synth".into(),
    }));
    let Some(Created::Track(track)) = add.created else {
        panic!("midi track");
    };
    (applied, _) = harness.applied(applied);
    let clip = harness.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_ratio(1, 1).unwrap(),
    }));
    let Some(Created::Clip(clip)) = clip.created else {
        panic!("midi clip");
    };
    (applied, _) = harness.applied(applied);
    harness.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_ratio(1, 2).unwrap(),
        pitch,
        channel: 0,
        velocity: 112,
    }));
    (applied, _) = harness.applied(applied);
    let scene = harness.run(Request::Edit(Command::AddScene {
        name: "console".into(),
    }));
    let Some(Created::Scene(scene)) = scene.created else {
        panic!("scene")
    };
    (applied, _) = harness.applied(applied);
    let console = harness.run(Request::Edit(Command::CreateSessionMidiClip {
        track,
        scene,
        name: "console notes".into(),
        source_length: BeatTime::from_quarters(1).unwrap(),
    }));
    let Some(Created::SessionClip(console)) = console.created else {
        panic!("console clip")
    };
    (applied, _) = harness.applied(applied);
    let note = harness.run(Request::Edit(Command::AddMidiNote {
        clip: console,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(1).unwrap(),
        pitch: installed_pitch.unwrap_or(if controlled_fixture { 60 } else { 64 }),
        channel: 0,
        velocity: 88,
    }));
    (applied, _) = harness.applied(applied);
    let mut insertion = request(&note.state, PluginEffectDestination::Instrument(track));
    if let Some(bundle) = installed {
        insertion.selection = installed_selection(
            &PathBuf::from(std::env::var_os("HEX_TEST_VST3_INSTRUMENT_CATALOG").unwrap()),
            &bundle,
            hex_plugin_catalog::PluginRole::Instrument,
        );
    }
    harness
        .service
        .submit(Request::InsertPluginEffect(insertion))
        .unwrap();
    let accepted = terminal(&mut harness);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    let project_track = accepted.state.project.track(track).unwrap();
    assert!(project_track.devices().is_empty());
    assert!(project_track.instrument().is_some());
    assert_eq!(accepted.state.project.sample_rate(), rate);
    (applied, _) = harness.applied(applied);
    // Exercise the resident keyboard request on a newly inserted worker
    // before the authored transport has ever started.
    let keys = Arc::new(
        harness
            .audio
            .as_ref()
            .unwrap()
            .control
            .new_computer_keyboard_input_on_control()
            .unwrap()
            .with_live_notes()
            .unwrap(),
    );
    let context = harness.submitted.unwrap().1.context;
    harness.run(Request::PrepareComputerKeyboard {
        project_generation: context.project_generation,
        revision: context.editor_revision,
        request_id: 1,
        track,
        input: Arc::clone(&keys),
    });
    let (keyboard_revision, keyboard) = harness.applied(applied);
    applied = keyboard_revision;
    assert_eq!(keyboard.computer_keyboard_request, Some(1));
    let pcm_peak = |frames: [hex_engine::Frame; 256]| {
        frames
            .into_iter()
            .flatten()
            .map(f32::abs)
            .fold(0.0, f32::max)
    };
    keys.set_key(0, Some((pitch, 100, 1))).unwrap();
    let keyboard_peak = (0..6)
        .map(|_| pcm_peak(render_completed_block(&harness)))
        .fold(0.0, f32::max);
    assert!(keyboard_peak > 0.001, "expected stopped keyboard pcm");
    assert!(
        !harness
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    keys.set_key(0, None).unwrap();
    for _ in 0..4 {
        render_completed_block(&harness);
    }
    if controlled_fixture {
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
    } else {
        let mut release_peak = f32::INFINITY;
        let mut quiet_blocks = 0;
        assert!(
            (0..1000).any(|_| {
                release_peak = pcm_peak(render_completed_block(&harness));
                quiet_blocks = if release_peak < 0.001 {
                    quiet_blocks + 1
                } else {
                    0
                };
                quiet_blocks == 4
            }),
            "keyboard release must settle, held peak={keyboard_peak}, release peak={release_peak}"
        );
    }
    println!("instrument keyboard rate={rate} peak={keyboard_peak}");
    if controlled_fixture {
        use hex_engine::computer_keyboard::LiveNoteEvent;
        let activation = keys.begin_live_notes_activation_on_control().unwrap();
        let queue = keys.live_notes().unwrap();
        queue
            .publish(
                activation,
                LiveNoteEvent::NoteOn {
                    channel: 0,
                    pitch,
                    velocity: 100,
                },
            )
            .unwrap();
        queue
            .publish(
                activation,
                LiveNoteEvent::NoteOff {
                    channel: 0,
                    pitch,
                    velocity: 41,
                },
            )
            .unwrap();
        let audible_frames = (0..6)
            .flat_map(|_| render_completed_block(&harness))
            .filter(|frame| frame.iter().any(|sample| sample.abs() > 0.001))
            .count();
        assert_eq!(
            audible_frames, 1,
            "ordered hardware tap must last one frame"
        );
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        println!("instrument hardware tap rate={rate} frames={audible_frames}");
    }
    // Stop advancing at frame 64 of the copied quantum. Play must carry its
    // new context inside that same worker block.
    let worker_phase = || {
        harness
            .callback
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .native_worker_phase_for_test()
    };
    let wait_for_short_callback = |harness: &ResidentHarness| {
        harness.render();
        wait(|| {
            harness
                .callback
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .native_workers_idle_on_control()
                == Some(true)
        });
    };
    for _ in 0..4 {
        if worker_phase() == Some(64) {
            break;
        }
        wait_for_short_callback(&harness);
    }
    assert_eq!(worker_phase(), Some(64));
    harness
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut peak = 0.0_f32;
    let mut peak_after_edit = 0.0_f32;
    for block in 0..20 {
        let frames = render_completed_block(&harness);
        if block < 2 {
            let early_peak = pcm_peak(frames);
            assert!(early_peak < 0.001, "early block {block}: {early_peak}");
        }
        for sample in frames.iter().flatten() {
            peak = peak.max(sample.abs());
            if block > 3 {
                peak_after_edit = peak_after_edit.max(sample.abs());
            }
        }
        if block == 3 {
            let before = harness
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .native_effect_source()
                .unwrap();
            harness.run(Request::Edit(Command::Balance {
                track,
                balance: -0.25,
            }));
            let (revision, _) = harness.applied(applied);
            let status = harness.audio.as_ref().unwrap().status().unwrap();
            let after = status.native_effect_source().unwrap();
            assert_eq!(after.audio_revision, revision);
            assert_eq!(after.output_identity, before.output_identity);
            assert!(status.playing.load(Acquire));
        }
    }
    assert!(peak > 0.001, "expected instrument pcm, peak={peak}");
    assert!(
        peak_after_edit > 0.001,
        "held instrument note must still sound, peak={peak_after_edit}"
    );
    println!("instrument arrangement rate={rate} peak={peak}");

    let send = |harness: &mut ResidentHarness, command| {
        harness
            .audio
            .as_mut()
            .unwrap()
            .control
            .session_command_with_ticket(command)
            .unwrap();
    };
    send(
        &mut harness,
        SessionPlaybackCommand::LaunchClip {
            clip: console,
            quantization: Immediate,
        },
    );
    for _ in 0..3 {
        render_completed_block(&harness);
    }
    let console_peak = render_completed_block(&harness)
        .iter()
        .flatten()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    assert!(console_peak > 0.001, "expected console instrument pcm");
    send(
        &mut harness,
        SessionPlaybackCommand::StopTrack {
            track,
            quantization: Immediate,
        },
    );
    for _ in 0..3 {
        render_completed_block(&harness);
    }
    let after_stop = render_completed_block(&harness);
    if controlled_fixture {
        assert!(
            after_stop.iter().all(|frame| *frame == [0.0; 2]),
            "stopped console must release the last gate"
        );
    }
    println!("instrument console rate={rate} peak={console_peak}");

    if controlled_fixture {
        // Hold both sources in the retained SDK component through the
        // desktop's priority Stop and stopped-restore handshake.
        send(
            &mut harness,
            SessionPlaybackCommand::LaunchClip {
                clip: console,
                quantization: Immediate,
            },
        );
        for _ in 0..3 {
            render_completed_block(&harness);
        }
        let authored_peak = pcm_peak(render_completed_block(&harness));
        assert!(authored_peak > 0.001);
        keys.set_key(0, Some((60, 100, 2))).unwrap();
        let combined_peak = (0..5)
            .map(|_| pcm_peak(render_completed_block(&harness)))
            .fold(0.0, f32::max);
        assert!(
            combined_peak > 1.5 * authored_peak,
            "expected both held gates"
        );
        let before_stop = harness
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire);
        harness.audio.as_ref().unwrap().control.stop();
        assert_eq!(harness.render_plugin_block(), [[0.0; 2]; 256]);
        harness
            .service
            .submit(Request::RestoreStoppedPlayback)
            .unwrap();
        let pending = harness.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(!pending.stopped_playback_restore_complete);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            harness.pump();
            harness.render_plugin_block();
            if let Ok(event) = harness.service.try_event() {
                assert!(event.error.is_none(), "{:?}", event.error);
                if event.stopped_playback_restore_complete {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "stopped restore terminal timed out"
            );
            std::thread::yield_now();
        }
        let (_, restored) = harness.applied(before_stop);
        assert!(restored.stopped_restore);
        wait(|| harness.plugin_completed.load(Acquire) == 0);
        for _ in 0..4 {
            assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        }
        keys.ensure_output_activation_on_control().unwrap();
        keys.set_key(0, Some((60, 100, 3))).unwrap();
        let restored_peak = (0..6)
            .map(|_| pcm_peak(render_completed_block(&harness)))
            .fold(0.0, f32::max);
        assert!(restored_peak > 0.001, "expected fresh stopped keyboard pcm");
        assert!(
            !harness
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
        keys.set_key(0, None).unwrap();
        for _ in 0..4 {
            render_completed_block(&harness);
        }
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        println!("instrument keyboard after priority stop rate={rate} peak={restored_peak}");
        // The restored Arrangement contains a note at zero. Route to silence
        // before starting the callback to isolate any retained old SDK gate.
        wait(|| {
            harness.pump();
            harness
                .audio
                .as_mut()
                .unwrap()
                .control
                .session_command_with_ticket(SessionPlaybackCommand::StopTrack {
                    track,
                    quantization: Immediate,
                })
                .is_ok()
        });
        send(&mut harness, SessionPlaybackCommand::Play);
        for _ in 0..4 {
            assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
        }
        send(
            &mut harness,
            SessionPlaybackCommand::LaunchClip {
                clip: console,
                quantization: Immediate,
            },
        );
        for _ in 0..3 {
            render_completed_block(&harness);
        }
        assert!(
            render_completed_block(&harness)
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 0.001)
        );
        send(
            &mut harness,
            SessionPlaybackCommand::StopTrack {
                track,
                quantization: Immediate,
            },
        );
        for _ in 0..3 {
            render_completed_block(&harness);
        }
        assert_eq!(render_completed_block(&harness), [[0.0; 2]; 256]);
    }
}
