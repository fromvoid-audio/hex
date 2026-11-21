//! A powered-off hosted Return keeps its aligned send in the current pass.
use super::*;
use hex_session::service::{ResidentGraphSubmission, ViewState};

const FADE: usize = 240;

fn assert_aligned_bypass(h: &ResidentHarness, latency: usize) {
    let (start, pcm) = render_rolling(h);
    for (offset, frame) in pcm.into_iter().enumerate() {
        let at = start + offset;
        assert_eq!(
            frame,
            [1.5 * dry_at(at, latency); 2],
            "direct dry and bypassed Return at frame {at}, delay {latency}"
        );
    }
}

fn assert_workerless_partition(h: &ResidentHarness, partition: usize) {
    const FRAMES: usize = 514;
    let start = status(h).position.load(Acquire) as usize;
    let mut pcm = [[0.0; 2]; FRAMES];
    let mut host = [0.0; 2 * FRAMES];
    let mut slot = h.callback.lock().unwrap();
    let callback = slot.as_mut().unwrap();
    let measured = allocation_counter::measure(|| {
        for chunk in pcm.chunks_mut(partition) {
            let output = &mut host[..chunk.len() * 2];
            callback.render_native_direct(chunk, output).unwrap();
            assert!(
                output
                    .chunks_exact(2)
                    .zip(chunk.iter())
                    .all(|(host, engine)| host == engine)
            );
        }
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0),
    );
    assert!(status(h).playing.load(Acquire));
    assert_eq!(status(h).position.load(Acquire), (start + FRAMES) as u64);
    for (offset, frame) in pcm.into_iter().enumerate() {
        assert_eq!(frame, [1.5 * dry_at(start + offset, BRIDGE); 2]);
    }
}

fn assert_aligned_mix(h: &ResidentHarness, wet_scale: f32, toward_bypass: bool) {
    let (start, pcm) = render_rolling(h);
    for (offset, frame) in pcm.into_iter().enumerate() {
        let progress = (offset + 1).min(FADE) as f32 / FADE as f32;
        let bypass = if toward_bypass {
            progress
        } else {
            1.0 - progress
        };
        let at = start + offset;
        let expected = dry_at(at, BRIDGE) * (1.0 + 0.5 * bypass + wet_scale * (1.0 - bypass));
        assert!(
            frame
                .into_iter()
                .all(|sample| (sample - expected).abs() < 2e-6),
            "aligned Return ramp at {at}: {frame:?} vs {expected}"
        );
    }
}

fn assert_original(event: &Event, current: &ViewState) {
    assert_eq!(event.state.project, current.project);
    assert_eq!(event.state.revision, current.revision);
    assert_eq!(event.state.can_undo, current.can_undo);
    assert_eq!(event.state.can_redo, current.can_redo);
}

fn queued_without_callback(
    h: &mut ResidentHarness,
    current: &ViewState,
    predecessor: AppliedAudioSource,
) -> ResidentGraphSubmission {
    let deadline = Instant::now() + Duration::from_millis(400);
    let mut pending = false;
    loop {
        h.pump();
        while let Ok(event) = h.service.try_event() {
            assert!(
                !event.request_complete,
                "Return power completed before the callback: status={:?}, error={:?}",
                event.status, event.error
            );
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(event.device_insert_confirmation.is_none());
            assert_original(&event, current);
            pending = true;
        }
        if let Some((audio_revision, metadata)) = h.submitted.as_ref()
            && pending
            && *audio_revision > predecessor.audio_revision
        {
            assert_eq!(metadata.context.editor_revision, current.revision + 1);
            return ResidentGraphSubmission {
                audio_revision: *audio_revision,
                output_identity: predecessor.output_identity,
                context: metadata.context,
            };
        }
        assert!(
            Instant::now() < deadline,
            "missing Return power preparation"
        );
        std::thread::yield_now();
    }
}

fn terminal_at_cut(
    h: &mut ResidentHarness,
    current: &ViewState,
    render_next: impl Fn(&ResidentHarness),
) -> Event {
    let deadline = Instant::now() + Duration::from_millis(400);
    loop {
        h.pump();
        while let Ok(event) = h.service.try_event() {
            assert!(
                event.device_insert_confirmation.is_none(),
                "this aligned Return should stay in the current pass"
            );
            if event.request_complete {
                return event;
            }
            assert!(event.error.is_none(), "{:?}", event.error);
            assert_original(&event, current);
        }
        assert!(
            Instant::now() < deadline,
            "missing actual Return power terminal"
        );
        render_next(h);
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn running_resident_return_power_off_waits_for_the_exact_cut_and_returns_the_worker() {
    let _native = native_fixture();
    let files = Files::new();
    let RollingHostedReturn {
        harness: mut h,
        inserted,
        track,
        bus,
        device,
        predecessor,
        binding,
    } = rolling_hosted_return(&files);
    h.enable_confirmed_device_inserts();
    let current = &inserted.state;
    let original_device = current.project.device(device).unwrap();
    let device_type = original_device.state().type_id();
    let original_worker = |h: &ResidentHarness| {
        assert_eq!(status(h).recording_source(), Some(predecessor));
        assert_eq!(h.plugin_records.load(Acquire), 1);
        let (live, accepted, _) = h.plugin_progress.lock().unwrap().source_bindings[0].unwrap();
        assert_eq!((live, accepted), (device.value(), binding));
    };
    let mut installed = None;
    for cancel in [true, false] {
        let next_wet = assert_aligned_wet(&h) + 0.0625 - 1.0;
        h.service
            .submit(Request::Edit(Command::SetDeviceBypass {
                device,
                bypassed: true,
            }))
            .unwrap();
        let exact = queued_without_callback(&mut h, current, predecessor);
        original_worker(&h);
        assert_aligned_mix(&h, next_wet, true);
        original_worker(&h);

        if cancel {
            let deadline = Instant::now() + Duration::from_millis(200);
            loop {
                match h
                    .audio
                    .as_ref()
                    .unwrap()
                    .control
                    .cancel_fresh_runtime_publication(exact.audio_revision)
                {
                    FreshRuntimeCancellationResult::Busy => {
                        assert!(Instant::now() < deadline, "Return cancellation stayed busy");
                        std::thread::yield_now();
                    }
                    result => {
                        assert_eq!(result, FreshRuntimeCancellationResult::Cancelled);
                        break;
                    }
                }
            }
            assert_aligned_mix(&h, next_wet + 0.0625, false);
            let event = terminal_at_cut(&mut h, current, |h| {
                assert_aligned_wet(h);
            });
            let receipt = event.device_insert_complete.unwrap();
            assert_eq!(receipt.submission, exact);
            assert_eq!(receipt.outcome, DeviceInsertOutcome::Cancelled);
            assert_original(&event, current);
            original_worker(&h);
            assert_aligned_wet(&h);
        } else {
            let event = terminal_at_cut(&mut h, current, |h| assert_aligned_bypass(h, BRIDGE));
            assert!(event.error.is_none(), "{:?}", event.error);
            let receipt = event.device_insert_complete.unwrap();
            assert_eq!(receipt.submission, exact);
            assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
            installed = Some((event, exact));
        }
    }

    let (powered_off, exact) = installed.unwrap();
    let applied = status(&h).recording_source().unwrap();
    assert_eq!(applied.output_identity, exact.output_identity);
    assert_eq!(applied.context, exact.context);
    assert_eq!(applied.audio_revision, exact.audio_revision);
    assert_ne!(applied, predecessor);
    assert_eq!(powered_off.state.revision, current.revision + 1);
    assert_eq!(powered_off.state.project.tracks(), current.project.tracks());
    let [kept_bus] = powered_off.state.project.return_tracks() else {
        panic!("the original Return is still routed");
    };
    assert_eq!(kept_bus.id(), bus);
    let [off] = kept_bus.devices() else {
        panic!("the original authored device is still in its slot");
    };
    assert_eq!(off.id(), device);
    assert!(off.bypassed());
    assert_eq!(off.state(), original_device.state());
    let observation = h.submitted_observation().unwrap();
    assert_eq!(
        observation.processor_status(device.value(), device_type),
        Some(hex_engine::PreparedDeviceProcessing::PoweredOff)
    );
    assert_eq!(observation.main_latency().unwrap().arrival_frames, 0);
    assert!(status(&h).native_effect_source().is_none());

    let deadline = Instant::now() + Duration::from_millis(400);
    while h.plugin_records.load(Acquire) != 0 {
        assert_aligned_bypass(&h, BRIDGE);
        h.pump();
        assert!(
            Instant::now() < deadline,
            "original Return worker stayed warm"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(h.plugin_progress.lock().unwrap().source_bindings[0].is_none());
    for _ in 0..3 {
        assert_aligned_bypass(&h, BRIDGE);
    }
    for partition in [1, 257] {
        assert_workerless_partition(&h, partition);
    }

    let renamed = h.run(Request::Edit(Command::RenameTrack {
        track,
        name: "dry refresh".into(),
    }));
    assert_eq!(renamed.state.revision, powered_off.state.revision + 1);
    let deadline = Instant::now() + Duration::from_millis(400);
    let refreshed = loop {
        h.pump();
        assert_aligned_bypass(&h, BRIDGE);
        assert_eq!(h.plugin_records.load(Acquire), 0);
        if let Some(source) = status(&h).recording_source()
            && source.context.editor_revision == renamed.state.revision
        {
            break source;
        }
        assert!(
            Instant::now() < deadline,
            "workerless refresh did not apply"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(refreshed.output_identity, applied.output_identity);
    assert!(refreshed.audio_revision > applied.audio_revision);
    h.pump();
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .processor_status(device.value(), device_type),
        Some(hex_engine::PreparedDeviceProcessing::PoweredOff)
    );
    assert!(status(&h).native_effect_source().is_none());

    h.audio.as_ref().unwrap().control.stop();
    render(&h);
    assert!(!status(&h).playing.load(Acquire));
    assert_eq!(status(&h).position.load(Acquire), 0);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_aligned_bypass(&h, 0);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn saved_off_resident_return_reopens_without_a_worker_and_preserves_authored_state() {
    let _native = native_fixture();
    let _saved = reopen_saved_off_return();
}

struct SavedOffHostedReturn {
    harness: ResidentHarness,
    files: Files,
    saved_off: Event,
    device: DeviceId,
    retired_binding: EffectBinding,
}

fn reopen_saved_off_return() -> SavedOffHostedReturn {
    let files = Files::new();
    let RollingHostedReturn {
        harness: mut original,
        device,
        binding: retired_binding,
        ..
    } = rolling_hosted_return(&files);
    original.audio.as_ref().unwrap().control.stop();
    render(&original);
    let captured_path = files.0.join("captured-return.hex");
    original
        .service
        .submit(Request::SaveNew(captured_path.clone()))
        .unwrap();
    let saved = state::edits::ordinary_terminal(&mut original);
    assert!(
        saved.save_complete && saved.error.is_none(),
        "{:?}",
        saved.error
    );
    let captured_project = hex_storage::load(&captured_path).unwrap();
    let state = captured_project.device(device).unwrap().state().clone();
    assert!(
        hex_plugin_state::decode(&state)
            .unwrap()
            .component
            .is_some()
    );
    drop(original);

    let mut editor = hex_project::Editor::new(captured_project, 8).unwrap();
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let off_project = editor.project();
    let off_path = files.0.join("saved-off-return.hex");
    hex_storage::save_new(&off_path, off_project).unwrap();
    let original_bytes = fs::read(&off_path).unwrap();

    let mut reopened = ResidentHarness::with_plugins_at_rate_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: BRIDGE as u32,
        }),
        48_000,
        files.0.canonicalize().unwrap().join("plugin-catalog.json"),
        4 * u64::from(FRAMES),
    );
    assert!(reopened.next().error.is_none());
    reopened.applied(0);
    let old_output = reopened
        .audio
        .as_ref()
        .unwrap()
        .control
        .output_identity()
        .unwrap();
    let opened = reopened.run(Request::Open(off_path.clone()));
    assert!(
        opened.open_complete && opened.error.is_none(),
        "{:?}",
        opened.error
    );
    assert_eq!(opened.state.project.as_ref(), off_project);
    let source = opened.state.canonical_audio_source.as_ref().unwrap();
    assert_eq!(
        source.audio_mode(),
        hex_session::DocumentAudioMode::Resident
    );
    let mut required = source.required_sources();
    assert!(matches!(
        required.next(),
        Some((_, hex_session::AudioSourceBinding::Resident(_)))
    ));
    assert!(required.next().is_none());
    let revision = super::super::super::resident_lifecycle::finish_replacement(
        &mut reopened,
        &opened,
        old_output,
    );
    let applied = status(&reopened).recording_source().unwrap();
    assert_eq!(
        (applied.context, applied.audio_revision),
        (source.context(), revision)
    );
    let observation = reopened.submitted_observation().unwrap();
    let processing = observation.processor_status(device.value(), state.type_id());
    assert_eq!(
        processing,
        Some(hex_engine::PreparedDeviceProcessing::PoweredOff),
        "saved Return restored as {processing:?}, native records={}",
        reopened.plugin_records.load(Acquire)
    );
    assert_eq!(
        observation
            .processor_latency(device.value(), state.type_id())
            .unwrap()
            .stage_frames,
        0
    );
    assert_eq!(observation.main_latency().unwrap().arrival_frames, 0);
    assert_eq!(fs::read(&off_path).unwrap(), original_bytes);
    let assert_no_worker = |h: &ResidentHarness| {
        assert!(status(h).native_effect_source().is_none());
        assert_eq!(h.plugin_records.load(Acquire), 0);
        assert_eq!(h.plugin_completed.load(Acquire), 0);
        assert!(h.plugin_progress.lock().unwrap().newest_source.is_none());
    };
    assert_no_worker(&reopened);

    assert_eq!(status(&reopened).position.load(Acquire), 0);
    reopened
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_aligned_bypass(&reopened, 0);
    assert_no_worker(&reopened);
    reopened.audio.as_ref().unwrap().control.stop();
    render(&reopened);

    reopened.service.submit(Request::SaveCurrent).unwrap();
    let saved_off = state::edits::ordinary_terminal(&mut reopened);
    assert!(
        saved_off.save_complete && saved_off.error.is_none(),
        "{:?}",
        saved_off.error
    );
    let persisted = hex_storage::load(&off_path).unwrap();
    assert_eq!(&persisted, off_project);
    let kept = persisted.device(device).unwrap();
    assert!(kept.bypassed());
    assert_eq!(kept.state(), &state);
    assert_no_worker(&reopened);
    SavedOffHostedReturn {
        harness: reopened,
        files,
        saved_off,
        device,
        retired_binding,
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn saved_off_resident_return_installs_a_fresh_worker_on_stopped_power_on() {
    let _native = native_fixture();
    let mut off = reopen_saved_off_return();
    let h = &mut off.harness;
    let current = &off.saved_off.state;
    let device = off.device;
    h.enable_confirmed_device_inserts();
    assert!(!status(h).playing.load(Acquire));
    let predecessor = status(h).recording_source().unwrap();
    assert_eq!(status(h).paused_input_interval().unwrap().0, predecessor);
    let command = Command::SetDeviceBypass {
        device,
        bypassed: false,
    };
    let mut expected = hex_project::Editor::new(current.project.as_ref().clone(), 8).unwrap();
    expected.execute(command.clone()).unwrap();

    let catalog = off.files.0.join("plugin-catalog.json");
    let parked = off.files.0.join("parked-catalog.json");
    fs::rename(&catalog, &parked).unwrap();
    h.service.submit(Request::Edit(command.clone())).unwrap();
    let unavailable = state::edits::ordinary_terminal(h);
    assert!(unavailable.error.is_some());
    assert_original(&unavailable, current);
    assert_eq!(status(h).recording_source(), Some(predecessor));
    assert_eq!(h.plugin_records.load(Acquire), 0);
    fs::rename(&parked, &catalog).unwrap();

    h.service.submit(Request::Edit(command.clone())).unwrap();
    let interrupted = queued_without_callback(h, current, predecessor);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let refused = terminal_at_cut(h, current, |h| assert_aligned_bypass(h, 0));
    assert_original(&refused, current);
    assert_eq!(
        status(h).rejected_revision(),
        Some((
            interrupted.audio_revision,
            hex_engine::playback::PlaybackHandoffRejection::RuntimeFenceChanged
        ))
    );
    assert_eq!(status(h).recording_source(), Some(predecessor));
    h.audio.as_ref().unwrap().control.stop();
    render(h);
    wait(|| {
        h.pump();
        render(h);
        h.plugin_records.load(Acquire) == 0
    });

    h.service.submit(Request::Edit(command)).unwrap();
    let submitted = queued_without_callback(h, current, predecessor);
    assert_eq!(status(h).recording_source(), Some(predecessor));
    assert!(status(h).native_effect_source().is_none());
    assert_eq!(h.plugin_completed.load(Acquire), 0);

    let on = terminal_at_cut(h, current, |h| {
        assert_eq!(render(h), [[0.0; 2]; BLOCK]);
    });
    assert!(on.error.is_none(), "{:?}", on.error);
    assert_eq!(on.state.project.as_ref(), expected.project());
    assert_eq!(on.state.revision, current.revision + 1);
    let receipt = on.device_insert_complete.unwrap();
    assert_eq!(receipt.outcome, DeviceInsertOutcome::Installed);
    assert_eq!(receipt.submission, submitted);
    assert_eq!(
        h.applied(predecessor.audio_revision).0,
        submitted.audio_revision
    );
    let applied = status(h).recording_source().unwrap();
    assert_eq!(status(h).native_effect_source(), Some(applied));
    assert_eq!(applied.output_identity, predecessor.output_identity);
    let observation = h.submitted_observation().unwrap();
    assert_eq!(
        observation.processor_status(device.value(), hex_plugin_state::TYPE_ID),
        Some(hex_engine::PreparedDeviceProcessing::Foreign)
    );
    wait(|| {
        h.pump();
        h.plugin_records.load(Acquire) == 1
            && h.plugin_progress.lock().unwrap().source_bindings[0].is_some_and(
                |(id, binding, _)| id == device.value() && binding != off.retired_binding,
            )
    });
    assert_eq!(h.plugin_completed.load(Acquire), 0);

    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(0))
        .unwrap();
    for _ in 0..3 {
        render(h);
    }
    let wet = assert_aligned_wet(h);
    assert_eq!(assert_aligned_wet(h), wet + 0.0625);
}
