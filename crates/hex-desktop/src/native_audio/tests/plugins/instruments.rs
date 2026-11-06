//! Native instrument insertion through the real Session and desktop worker.
use super::*;
use hex_project::BeatTime;

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
            child.env("HEX_ORIGINAL_INSTANCE_VARIANT", "115");
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
    let note = harness.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_ratio(1, 2).unwrap(),
        pitch: 60,
        channel: if installed.is_some() { 0 } else { 2 },
        velocity: 112,
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
        let previous_completion = harness.plugin_completed.load(Acquire);
        let frames = harness.render_plugin_block();
        if block < 2 {
            assert!(frames.iter().all(|frame| *frame == [0.0; 2]));
        }
        for sample in frames.iter().flatten() {
            peak = peak.max(sample.abs());
            if block > 3 {
                peak_after_edit = peak_after_edit.max(sample.abs());
            }
        }
        let deadline = Instant::now() + Duration::from_millis(800);
        while harness.plugin_completed.load(Acquire) <= previous_completion {
            let status = harness.audio.as_ref().unwrap().status().unwrap();
            assert!(
                Instant::now() < deadline,
                "worker did not complete block {block}; completed={} playing={} source={:?} fault={:?}",
                harness.plugin_completed.load(Acquire),
                status.playing.load(Acquire),
                status.native_effect_source(),
                status.render_fault_snapshot(),
            );
            std::thread::yield_now();
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
}
