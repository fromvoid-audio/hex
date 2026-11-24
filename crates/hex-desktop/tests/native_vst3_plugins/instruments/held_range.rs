//! A refused resident clip-range edit leaves the sounding instrument and later mixer edits usable.
use super::*;

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn resident_held_range_refusal_keeps_canonical_clip_and_later_mixer_edits() {
    const CHILD: &str = "HEX_TEST_HELD_RANGE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let traces = Files::new();
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_NOTE_TRACE_DIR", &traces.0)
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let _native = native_fixture();
    let files = Files::new();
    let dir = files.0.canonicalize().unwrap();
    let mut selected = selection();
    let module = dir.join("original.vst3");
    super::super::ui::copy_original_module(&selected.module, &module);
    selected.module = module;
    hex_ui::write_plugin_effect_test_catalog(&dir, &selected, "Instrument|Synth", 0).unwrap();
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        44_100,
        dir.join("plugin-catalog.json"),
        8_000_000,
    );
    h.next();
    let (mut revision, _) = h.applied(0);
    let imported = h.run(Request::Import(files.wav(44_100, 131_072)));
    let audio_track = imported.state.project.tracks()[0].id();
    (revision, _) = h.applied(revision);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "held note".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("midi track")
    };
    (revision, _) = h.applied(revision);
    let length = BeatTime::from_quarters(32).unwrap();
    let Some(Created::Clip(clip)) = h
        .run(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length,
        }))
        .created
    else {
        panic!("midi clip")
    };
    (revision, _) = h.applied(revision);
    let authored = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: length,
        pitch: 60,
        channel: 0,
        velocity: 63,
    }));
    (revision, _) = h.applied(revision);
    let mut insertion = request(&authored.state, PluginEffectDestination::Instrument(track));
    insertion.selection = selected;
    h.run(Request::InsertPluginEffect(insertion));
    let installed = terminal(&mut h);
    assert!(
        installed.plugin_effect_complete[0].accepted,
        "{:?}",
        installed.error
    );
    h.applied(revision);
    h.enable_confirmed_device_inserts();
    let source = |h: &ResidentHarness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source()
            .unwrap()
    };
    let before = source(&h);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let settled = |h: &ResidentHarness| {
        for _ in 0..7 {
            render_completed_block(h);
        }
        render_completed_block(h)[255]
    };
    let sounding = settled(&h);
    let held = 63.0 / 127.0;
    assert!(
        sounding.iter().all(|sample| *sample > held + 0.1),
        "held instrument and imported track: {sounding:?}"
    );
    let traces = PathBuf::from(std::env::var_os("HEX_ORIGINAL_NOTE_TRACE_DIR").unwrap());
    let paths = fs::read_dir(&traces)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "bin"))
        .collect::<Vec<_>>();
    let [original_trace]: [PathBuf; 1] = paths.try_into().expect("one original instrument helper");
    let until = installed
        .state
        .project
        .tempo_map()
        .beat_to_frame(length)
        .unwrap();
    let range = hex_project::TimeRange::new(vec![track], 0, until).unwrap();
    let pending = h.run(Request::Edit(Command::SetTimeRangeActive {
        range: range.clone(),
        active: false,
    }));
    assert!(
        !pending.request_complete,
        "a held range requires the actual audio decision"
    );
    assert_eq!(pending.state.revision, installed.state.revision);
    let refused = edit_terminal(&mut h);
    assert_eq!(refused.state.project, installed.state.project);
    assert_eq!(refused.state.revision, installed.state.revision);
    let refusal = refused.error;
    let (refused_revision, reason) = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .rejected_revision()
        .unwrap();
    assert_eq!(
        reason,
        hex_engine::PlaybackHandoffRejection::HeldArrangementGateChanged
    );
    assert!(
        refusal
            .as_deref()
            .is_some_and(|message| message.contains(reason.message())),
        "{refusal:?}"
    );
    assert_eq!(source(&h), before);
    assert!(refused_revision > before.audio_revision);
    let mut last_audio = refused_revision;
    for muted in [true, false] {
        let event = h.run(Request::Edit(Command::ToggleMute(audio_track)));
        assert_eq!(
            event.state.project.track(audio_track).unwrap().muted(),
            muted
        );
        let (next, receipt) = h.applied(last_audio);
        assert!(next > last_audio);
        assert_eq!(receipt.context.editor_revision, event.state.revision);
        assert_eq!(source(&h).context, receipt.context);
        last_audio = next;
        let pcm = settled(&h);
        let expected = if muted { [held; 2] } else { sounding };
        for (actual, expected) in pcm.into_iter().zip(expected) {
            assert!(
                (actual - expected).abs() < 1e-5,
                "mute={muted}: {actual} != {expected}"
            );
        }
    }
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render_plugin_block(), [[0.0; 2]; 256]);
    // Retire the original helper so its control lane writes the final note ledger.
    let mut restored = h.run(Request::RestoreStoppedPlayback);
    if !restored.stopped_playback_restore_complete {
        restored = restored_terminal(&mut h);
    }
    assert!(restored.error.is_none(), "{:?}", restored.error);
    wait(|| {
        h.pump();
        let Ok(bytes) = fs::read(&original_trace) else {
            return false;
        };
        let Ok(data) = <[u8; 20]>::try_from(bytes) else {
            return false;
        };
        let [_, ons, offs, on_id, off_id] =
            std::array::from_fn(|i| i32::from_ne_bytes(data[i * 4..i * 4 + 4].try_into().unwrap()));
        ons == 1 && offs == 1 && on_id > 0 && on_id == off_id
    });

    let pending = h.run(Request::Edit(Command::SetTimeRangeActive {
        range,
        active: false,
    }));
    assert!(!pending.request_complete);
    let applied = edit_terminal(&mut h);
    assert!(applied.error.is_none(), "{:?}", applied.error);
    assert_eq!(applied.state.revision, pending.state.revision + 1);
    assert_eq!(source(&h).context.editor_revision, applied.state.revision);
    assert!(!applied.state.project.midi_clip(clip).unwrap().active());
    for (request, active) in [(Request::Undo, true), (Request::Redo, false)] {
        h.service.submit(request).unwrap();
        let history = edit_terminal(&mut h);
        assert!(
            history.error.is_none(),
            "history active={active}: {:?}",
            history.error
        );
        assert_eq!(
            history.state.project.midi_clip(clip).unwrap().active(),
            active
        );
    }
}

fn edit_terminal(h: &mut ResidentHarness) -> Event {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        h.pump();
        if h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
        {
            render_completed_block(h);
        } else {
            h.render_plugin_block();
        }
        if let Ok(event) = h.service.try_event()
            && event.request_complete
        {
            return event;
        }
        assert!(
            Instant::now() < deadline,
            "resident edit did not reach a terminal"
        );
    }
}
