//! Observe the original worker in a private direct Arrangement move candidate.
use super::*;
use hex_audio::prepared::FreshRuntimeCancellationResult;
use hex_project::DeviceSelectionIds;
use hex_session::service::DeviceInsertOutcome;

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn direct_held_vst3_move_preserves_original_worker_on_cancel_and_refusal() {
    const CHILD: &str = "HEX_TEST_DIRECT_HOSTED_INSTRUMENT_MOVE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
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
    h.run(Request::Import(files.wav(48_000, 8_192)));
    (applied, _) = h.applied(applied);
    let mut add_track = |name: &str| {
        let Created::Track(track) = h
            .run(Request::Edit(Command::AddMidiTrack { name: name.into() }))
            .created
            .unwrap()
        else {
            panic!("MIDI track");
        };
        (applied, _) = h.applied(applied);
        track
    };
    let donor = add_track("held source");
    let receiver = add_track("destination");
    let length = BeatTime::from_quarters(32).unwrap();
    let Created::Clip(clip) = h
        .run(Request::Edit(Command::PlaceMidiClip {
            track: donor,
            start: BeatTime::ZERO,
            length,
        }))
        .created
        .unwrap()
    else {
        panic!("instrument clip");
    };
    (applied, _) = h.applied(applied);
    h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: length,
        pitch: 60,
        channel: 0,
        velocity: 63,
    }));
    (applied, _) = h.applied(applied);
    let mixed = h.run(Request::Edit(Command::SetTrackGains {
        gains: vec![(donor, 0.25), (receiver, 0.75)],
    }));
    (applied, _) = h.applied(applied);
    let mut insertion = request(&mixed.state, PluginEffectDestination::Instrument(donor));
    insertion.selection = selected;
    h.run(Request::InsertPluginEffect(insertion));
    let before = terminal(&mut h);
    assert!(before.plugin_effect_complete[0].accepted);
    let instrument = before
        .state
        .project
        .track(donor)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    h.applied(applied);

    let recording_source = |h: &ResidentHarness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source()
            .unwrap()
    };
    let binding = |h: &ResidentHarness| {
        h.plugin_progress
            .lock()
            .unwrap()
            .source_bindings
            .iter()
            .flatten()
            .find_map(|&(id, binding, _)| (id == instrument.value()).then_some(binding))
    };
    let original_worker = binding(&h).unwrap();
    h.enable_confirmed_device_inserts();
    let control = &mut h.audio.as_mut().unwrap().control;
    control.set_audio_diagnostics(hex_audio::audio_diagnostics::AudioDiagnosticsFlags::ALL);
    control.command(PlaybackCommand::Play).unwrap();
    for _ in 0..7 {
        render_completed_block(&h);
    }
    let note = 63.0 / 127.0;
    let donor_pcm = 0.25 + 0.25 * note;
    let predecessor_pcm = render_completed_block(&h);
    assert!(
        predecessor_pcm
            .iter()
            .flatten()
            .all(|sample| (*sample - donor_pcm).abs() < 1e-5),
        "held worker output before move: {:?}",
        predecessor_pcm[0]
    );
    let predecessor = recording_source(&h);
    let mut last_move_revision = predecessor.audio_revision;
    for cancel in [true, false] {
        let prepared_binding = h
            .device_insert_probe()
            .observe_next_foreign_binding(receiver, instrument);
        let (submitted, resume) = h.probe_next_submitted_device_insert();
        h.service
            .submit(Request::MoveTrackDevices {
                project_generation: 0,
                revision: before.state.revision,
                source: donor,
                selected: DeviceSelectionIds::new(&[instrument]).unwrap(),
                destination: receiver,
            })
            .unwrap();
        // The candidate borrows compiler facts. Neither fresh worker capture
        // nor a callback should be needed to compile or submit it.
        let exact = submitted.recv_timeout(Duration::from_millis(800)).unwrap();
        let selected = prepared_binding
            .try_recv()
            .expect("prepared graph observation precedes submission");
        assert_eq!(
            selected,
            Some((
                hex_engine::ProcessorChainOwner::Track(donor.value()),
                original_worker
            ))
        );
        assert_eq!(exact.output_identity, predecessor.output_identity);
        assert!(exact.audio_revision > last_move_revision);
        assert_eq!(recording_source(&h), predecessor);
        resume.send(()).unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(!pending.request_complete, "move finished before a callback");
        assert_eq!(pending.state.project, before.state.project);
        if cancel {
            wait(|| {
                match h
                    .audio
                    .as_ref()
                    .unwrap()
                    .control
                    .cancel_fresh_runtime_publication(exact.audio_revision)
                {
                    FreshRuntimeCancellationResult::Busy => false,
                    state => {
                        assert_eq!(state, FreshRuntimeCancellationResult::Cancelled);
                        true
                    }
                }
            });
        }
        let deadline = Instant::now() + Duration::from_millis(800);
        let terminal = loop {
            render_replay_block(&h);
            h.pump();
            if let Ok(event) = h.service.try_event() {
                assert!(event.device_insert_confirmation.is_none());
                assert_eq!(event.state.project, before.state.project);
                if event.request_complete {
                    break event;
                }
            }
            assert!(
                Instant::now() < deadline,
                "move never returned its exact source"
            );
        };
        let complete = terminal.device_insert_complete.unwrap();
        assert_eq!(complete.submission, exact);
        last_move_revision = complete.submission.audio_revision;
        let expected = if cancel {
            DeviceInsertOutcome::Cancelled
        } else {
            DeviceInsertOutcome::Refused(hex_engine::PlaybackHandoffRejection::SessionRuntime)
        };
        assert_eq!(complete.outcome, expected);
        assert_eq!(recording_source(&h), predecessor);
        assert_eq!(binding(&h), Some(original_worker));
        assert!(render_completed_block(&h).iter().flatten().all(|sample| {
            (*sample - donor_pcm).abs() < 1e-5 || (*sample - 0.25 * note).abs() < 1e-5
        }));
    }

    // An unrelated edit recompiles the current canonical donor. It must still
    // select the original worker and preserve its sounding gate.
    let refreshed = h.run(Request::Edit(Command::SetTrackGains {
        gains: vec![(receiver, 0.5)],
    }));
    assert!(refreshed.request_complete);
    assert!(
        refreshed
            .state
            .project
            .track(donor)
            .unwrap()
            .instrument()
            .is_some()
    );
    // The refusal above remains the last rejected revision until a new
    // receipt replaces it. Wait past that exact terminal for the refresh.
    let (refreshed_revision, metadata) = h.applied(last_move_revision);
    let refreshed_source = recording_source(&h);
    assert_eq!(refreshed_source.audio_revision, refreshed_revision);
    assert_eq!(refreshed_source.context, metadata.context);
    assert_eq!(
        refreshed_source.output_identity,
        predecessor.output_identity
    );
    assert_eq!(binding(&h), Some(original_worker));
    assert!(render_completed_block(&h).iter().flatten().all(|sample| {
        (*sample - donor_pcm).abs() < 1e-5 || (*sample - 0.25 * note).abs() < 1e-5
    }));
}
