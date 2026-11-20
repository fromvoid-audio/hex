//! Physical SDK gate and exact predecessor through a Resident removal offer.
use super::*;
use hex_session::service::{DeviceInsertOutcome, GraphEditOperation};

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn resident_vst3_removal_and_stopped_history_wait_for_callback_receipts() {
    const CHILD: &str = "HEX_TEST_HOSTED_INSTRUMENT_REMOVAL_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let trace = Files::new();
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_NOTE_TRACE_DIR", &trace.0)
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
    let catalog = Files::new();
    let directory = catalog.0.canonicalize().unwrap();
    let mut selected = selection();
    let module = directory.join("original.vst3");
    super::super::ui::copy_original_module(&selected.module, &module);
    selected.module = module;
    hex_ui::write_plugin_effect_test_catalog(&directory, &selected, "Instrument|Synth", 0).unwrap();
    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        directory.join("plugin-catalog.json"),
    );
    h.next();
    let (mut applied, _) = h.applied(0);
    // The harness limits resident PCM to 8,192 frames.
    h.run(Request::Import(files.wav(48_000, 8_192)));
    (applied, _) = h.applied(applied);
    let Created::Track(track) = h
        .run(Request::Edit(Command::AddMidiTrack {
            name: "held instrument".into(),
        }))
        .created
        .unwrap()
    else {
        panic!("instrument track");
    };
    (applied, _) = h.applied(applied);
    let length = BeatTime::from_quarters(8).unwrap();
    let Created::Clip(clip) = h
        .run(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length,
        }))
        .created
        .unwrap()
    else {
        panic!("instrument clip");
    };
    (applied, _) = h.applied(applied);
    let authored = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: length,
        pitch: 60,
        channel: 0,
        velocity: 63,
    }));
    (applied, _) = h.applied(applied);
    let mut insertion = request(&authored.state, PluginEffectDestination::Instrument(track));
    insertion.selection = selected;
    h.run(Request::InsertPluginEffect(insertion));
    let before = terminal(&mut h);
    assert!(before.plugin_effect_complete[0].accepted);
    let original = before
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    h.applied(applied);
    h.enable_confirmed_device_inserts();
    let control = &mut h.audio.as_mut().unwrap().control;
    control.set_audio_diagnostics(hex_audio::audio_diagnostics::AudioDiagnosticsFlags::ALL);
    control.command(PlaybackCommand::Play).unwrap();
    for _ in 0..7 {
        render_completed_block(&h);
    }
    let incumbent = 0.25 + 63.0 / 127.0;
    let expect_incumbent = |h: &ResidentHarness| {
        assert!(
            render_completed_block(h)
                .iter()
                .flatten()
                .all(|value| (*value - incumbent).abs() < 1e-5)
        );
    };
    expect_incumbent(&h);
    let source = |h: &ResidentHarness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source()
            .unwrap()
    };
    let predecessor = source(&h);
    let worker = |h: &ResidentHarness| {
        h.plugin_progress
            .lock()
            .unwrap()
            .source_bindings
            .iter()
            .flatten()
            .find_map(|&(device, binding, _)| (device == original.value()).then_some(binding))
    };
    let binding = worker(&h).unwrap();
    let trace_dir = PathBuf::from(std::env::var_os("HEX_ORIGINAL_NOTE_TRACE_DIR").unwrap());
    let [trace]: [PathBuf; 1] = fs::read_dir(trace_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "bin"))
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    let event = |h: &mut ResidentHarness| {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            h.pump();
            if let Ok(event) = h.service.try_event() {
                return event;
            }
            render_completed_block(h);
            assert!(Instant::now() < deadline, "missing hosted removal event");
        }
    };
    let start = |h: &mut ResidentHarness| {
        h.service
            .submit(Request::Edit(Command::RemoveInstrument { track }))
            .unwrap();
        loop {
            let event = event(h);
            assert_eq!(event.state.project, before.state.project);
            assert_eq!(event.state.revision, before.state.revision);
            assert_eq!(event.state.can_undo, before.state.can_undo);
            assert_eq!(event.state.can_redo, before.state.can_redo);
            assert_eq!(source(h), predecessor);
            assert_eq!(worker(h), Some(binding));
            if let Some(error) = event.error {
                while let Some(row) = h.audio.as_mut().unwrap().control.take_audio_diagnostic() {
                    eprintln!("hosted removal diagnostic {row:?}");
                }
                panic!("hosted removal refused before consent: {error}");
            }
            assert!(!event.request_complete, "removal completed before its cut");
            if let Some(offer) = event.device_insert_confirmation {
                assert_eq!(offer.operation(), GraphEditOperation::RemoveDevice);
                assert_eq!(
                    offer.submission().output_identity,
                    predecessor.output_identity
                );
                break offer;
            }
        }
    };
    assert!(start(&mut h).cancel());
    let cancelled = loop {
        let event = event(&mut h);
        if event.request_complete {
            break event;
        }
    };
    assert_eq!(
        cancelled.device_insert_complete.unwrap().outcome,
        DeviceInsertOutcome::Cancelled
    );
    assert_eq!(cancelled.state.project, before.state.project);
    assert_eq!(cancelled.state.revision, before.state.revision);
    assert_eq!(cancelled.state.can_undo, before.state.can_undo);
    assert_eq!(cancelled.state.can_redo, before.state.can_redo);
    assert_eq!(source(&h), predecessor);
    assert_eq!(worker(&h), Some(binding));
    expect_incumbent(&h);

    let approval = start(&mut h);
    let (submitted, resume) = h.probe_next_submitted_device_insert();
    assert!(approval.proceed());
    let queued = submitted.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(source(&h), predecessor);
    assert_eq!(worker(&h), Some(binding));
    resume.send(()).unwrap();
    let installed = loop {
        let event = event(&mut h);
        assert!(event.error.is_none(), "{:?}", event.error);
        if event.request_complete {
            break event;
        }
        assert_eq!(event.state.project, before.state.project);
    };
    let receipt = installed.device_insert_complete.unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(receipt.submission, queued);
    assert_eq!(installed.state.revision, before.state.revision + 1);
    assert!(
        installed
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .is_none()
    );
    assert_eq!(source(&h).audio_revision, queued.audio_revision);
    assert_eq!(source(&h).context, queued.context);
    assert_eq!(source(&h).output_identity, predecessor.output_identity);
    assert!(installed.state.project.device(original).is_some());
    for _ in 0..2 {
        render_completed_block(&h);
    }
    let dry = render_completed_block(&h);
    assert!(
        dry.iter()
            .flatten()
            .all(|sample| (*sample - 0.25).abs() < 1e-5)
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        h.pump();
        render_completed_block(&h);
        let words = fs::read(&trace)
            .ok()
            .and_then(|bytes| <[u8; 20]>::try_from(bytes).ok())
            .map(|data| {
                std::array::from_fn::<_, 5, _>(|index| {
                    i32::from_ne_bytes(data[index * 4..index * 4 + 4].try_into().unwrap())
                })
            });
        if words.is_some_and(|[_, on, off, on_id, off_id]| {
            on == 1 && off == 1 && on_id > 0 && on_id == off_id
        }) && h.plugin_records.load(Acquire) == 0
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "original gate did not retire: {words:?}, {:?}, {:?}",
            h.plugin_progress.lock().unwrap(),
            h.retirement_issues
        );
    }

    h.audio.as_ref().unwrap().control.stop();
    h.render_plugin_block();
    let mut stopped = h.run(Request::RestoreStoppedPlayback);
    if !stopped.stopped_playback_restore_complete {
        stopped = restored_terminal(&mut h);
    }
    assert!(stopped.error.is_none(), "{:?}", stopped.error);
    assert_eq!(stopped.state.project, installed.state.project);
    let (_, stopped_metadata) = h.applied(queued.audio_revision);
    assert!(stopped_metadata.stopped_restore);
    let stopped_source = source(&h);
    assert!(
        !h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    let (private, resume, _) = h.probe_next_native_publication();
    h.service.submit(Request::Undo).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while private.try_recv().is_err() {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(
                !event.request_complete,
                "Undo ended before native publication"
            );
        }
        h.render_plugin_block();
        assert!(
            Instant::now() < deadline,
            "fresh native Undo did not prepare"
        );
        std::thread::yield_now();
    }
    assert_eq!(source(&h), stopped_source);
    assert_eq!(h.plugin_records.load(Acquire), 1);
    let fresh = h.plugin_progress.lock().unwrap().newest_source.unwrap().1;
    assert_ne!(fresh, binding);
    resume.send(()).unwrap();

    // Control may now submit the prepared runtime, but no callback has run.
    // The editor still owns the removed snapshot and its original history.
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            assert_eq!(source(&h), stopped_source);
            assert!(
                !event.request_complete,
                "Undo completed before callback admission"
            );
            assert_eq!(event.state.project, stopped.state.project);
            assert_eq!(event.state.revision, stopped.state.revision);
            assert_eq!(event.state.can_undo, stopped.state.can_undo);
            assert_eq!(event.state.can_redo, stopped.state.can_redo);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Undo did not acknowledge the private graph"
        );
        std::thread::yield_now();
    }
    let undone = loop {
        let event = event(&mut h);
        assert!(event.error.is_none(), "{:?}", event.error);
        if event.request_complete {
            break event;
        }
    };
    let receipt = undone.device_insert_complete.unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(source(&h).audio_revision, receipt.submission.audio_revision);
    assert_eq!(source(&h).context, receipt.submission.context);
    assert_eq!(source(&h).output_identity, stopped_source.output_identity);
    assert_eq!(undone.state.revision, stopped.state.revision + 1);
    assert_eq!(undone.state.project, before.state.project);
    wait(|| {
        h.pump();
        worker(&h) == Some(fresh)
    });
    assert_eq!(h.render_plugin_block(), [[0.0; 2]; 256]);

    let restored_source = source(&h);
    h.refuse.store(true, Release);
    h.service.submit(Request::Redo).unwrap();
    let refused = h.next();
    assert!(refused.request_complete && refused.error.is_some());
    assert_eq!(refused.state.project, undone.state.project);
    assert_eq!(refused.state.revision, undone.state.revision);
    assert_eq!(refused.state.can_undo, undone.state.can_undo);
    assert_eq!(refused.state.can_redo, undone.state.can_redo);
    assert_eq!(source(&h), restored_source);
    h.refuse.store(false, Release);

    let (submitted, resume) = h.probe_next_submitted_device_insert();
    h.service.submit(Request::Redo).unwrap();
    let queued = submitted.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(source(&h), restored_source);
    resume.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        h.pump();
        if let Ok(pending) = h.service.try_event() {
            assert!(pending.error.is_none(), "{:?}", pending.error);
            assert!(
                !pending.request_complete,
                "Redo completed before callback admission"
            );
            assert_eq!(pending.state.project, undone.state.project);
            assert_eq!(pending.state.revision, undone.state.revision);
            assert_eq!(pending.state.can_undo, undone.state.can_undo);
            assert_eq!(pending.state.can_redo, undone.state.can_redo);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Redo did not acknowledge the private graph"
        );
        std::thread::yield_now();
    }
    assert_eq!(source(&h), restored_source);
    let redone = loop {
        let event = event(&mut h);
        assert!(event.error.is_none(), "{:?}", event.error);
        if event.request_complete {
            break event;
        }
    };
    let receipt = redone.device_insert_complete.unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(receipt.submission, queued);
    assert_eq!(source(&h).audio_revision, queued.audio_revision);
    assert_eq!(source(&h).context, queued.context);
    assert_eq!(source(&h).output_identity, restored_source.output_identity);
    assert_eq!(redone.state.revision, undone.state.revision + 1);
    assert_eq!(redone.state.project, installed.state.project);
    wait(|| {
        h.pump();
        h.render_plugin_block();
        h.plugin_records.load(Acquire) == 0
    });
}
