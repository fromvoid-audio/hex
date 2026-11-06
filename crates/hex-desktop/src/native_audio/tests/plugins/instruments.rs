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
        duration: BeatTime::from_ratio(1, 8).unwrap(),
        pitch: 60,
        channel: if installed.is_some() { 0 } else { 2 },
        velocity: 112,
    }));
    (applied, _) = harness.applied(applied);
    let mut insertion = request(&note.state, PluginEffectDestination::Instrument(track));
    if let Some(bundle) = installed {
        let catalog = hex_plugin_catalog::Catalog::load(&PathBuf::from(
            std::env::var_os("HEX_TEST_VST3_INSTRUMENT_CATALOG").unwrap(),
        ))
        .unwrap();
        let selected = catalog
            .rows("")
            .into_iter()
            .find_map(|row| {
                (row.module.file_name()?.to_str()? == bundle)
                    .then_some(row.selection?)
                    .filter(|selection| {
                        selection.role == hex_plugin_catalog::PluginRole::Instrument
                    })
            })
            .unwrap();
        insertion.selection = PluginEffectSelection {
            module: selected.module.clone(),
            executable: selected.executable_relative_path.clone(),
            cid: selected.cid,
            executable_sha256: selected.executable_sha256.clone(),
            info_sha256: selected.info_sha256.clone(),
        };
    }
    harness
        .service
        .submit(Request::InsertPluginEffect(insertion))
        .unwrap();
    let accepted = terminal(&mut harness);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    let track = accepted.state.project.track(track).unwrap();
    assert!(track.devices().is_empty());
    assert!(track.instrument().is_some());
    assert_eq!(accepted.state.project.sample_rate(), rate);
    harness.applied(applied);
    harness
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let mut peak = 0.0_f32;
    for block in 0..20 {
        let frames = harness.render_plugin_block();
        if block < 2 {
            assert!(frames.iter().all(|frame| *frame == [0.0; 2]));
        }
        for sample in frames.iter().flatten() {
            peak = peak.max(sample.abs());
        }
        let deadline = Instant::now() + Duration::from_millis(800);
        while harness.plugin_completed.load(Acquire) <= block {
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
    }
    assert!(peak > 0.001, "expected instrument pcm, peak={peak}");
    println!("instrument arrangement rate={rate} peak={peak}");
}
