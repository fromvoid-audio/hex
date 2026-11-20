//! Source-free saved Off Main and exact fresh stopped On lifecycle.
//! One valid saved Off Main must remain source-free until an exact fresh On.
use super::*;
use hex_engine::PreparedDeviceProcessing;

fn observed(h: &ResidentHarness, device: hex_project::DeviceId) -> PreparedDeviceProcessing {
    let (revision, metadata) = h.submitted.unwrap();
    let applied = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .recording_source()
        .unwrap();
    assert_eq!(
        (applied.audio_revision, applied.context),
        (revision, metadata.context)
    );
    h.submitted_observation()
        .unwrap()
        .processor_status(device.value(), hex_plugin_state::TYPE_ID)
        .unwrap()
}

// Drive only the old Off callback while SDK preparation is private. The two
// existing gates then hold the graph before enqueue and immediately after its
// exact submission, so callback ACK cannot accidentally make the test pass.
fn prepare_on_before_ack(
    h: &mut ResidentHarness,
    device: hex_project::DeviceId,
    source: hex_audio::prepared::AppliedAudioSource,
) -> hex_session::service::Event {
    let (private, enqueue, _) = h.probe_next_native_publication();
    let (submitted, release) = h.probe_next_submitted_device_insert();
    h.service
        .submit(Request::Edit(Command::ToggleDeviceBypass { device }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        h.pump();
        if private.try_recv().is_ok() {
            break;
        }
        if let Ok(event) = h.service.try_event() {
            assert!(
                event.error.is_none(),
                "private On refusal: {:?}",
                event.error
            );
            assert!(!event.request_complete, "On completed while still private");
        }
        h.render_plugin_block();
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .recording_source(),
            Some(source)
        );
        if Instant::now() >= until {
            let progress = format!("{:?}", h.plugin_progress.lock().unwrap());
            panic!(
                "On private stage: loaded={} completed={} {progress}",
                h.plugin_records.load(Acquire),
                h.plugin_completed.load(Acquire)
            );
        }
        std::thread::yield_now();
    }
    assert_eq!(h.plugin_completed.load(Acquire), 0);
    enqueue.send(()).unwrap();
    let queued = submitted.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(queued.output_identity, source.output_identity);
    assert_ne!(queued.audio_revision, source.audio_revision);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source(),
        Some(source)
    );
    release.send(()).unwrap();
    h.next()
}

#[test]
#[ignore = "source-only product follow-up oracle; original signed SDK/helper receipts required"]
fn saved_off_main_reopens_without_worker_then_installs_fresh_stopped_on() {
    let _native = native_fixture();
    let saved = nondefault_fixture_with_frames(8192);
    let device = saved.project.main_devices()[0].id();
    let mut editor = hex_project::Editor::new(saved.project.clone(), 8).unwrap();
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let off_path = saved.files.0.join("saved-off-main.hex");
    hex_storage::save_new(&off_path, editor.project()).unwrap();
    let off_project = editor.project().clone();
    let original_bytes = fs::read(&off_path).unwrap();

    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        saved.catalog.clone(),
    );
    assert!(h.next().error.is_none());
    h.applied(0);
    h.enable_confirmed_device_inserts();
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    assert_ne!(output, saved.old_output);
    let opened = h.run(Request::Open(off_path.clone()));
    assert_eq!(opened.state.project.as_ref(), &off_project);
    super::super::super::resident_lifecycle::finish_replacement(&mut h, &opened, output);
    assert_eq!(observed(&h, device), PreparedDeviceProcessing::PoweredOff);
    assert_eq!(h.plugin_records.load(Acquire), 0);
    assert!(h.plugin_progress.lock().unwrap().binding.is_none());
    assert_eq!(h.plugin_completed.load(Acquire), 0);
    let stage = h
        .submitted_observation()
        .unwrap()
        .processor_latency(device.value(), hex_plugin_state::TYPE_ID)
        .unwrap();
    assert_eq!(stage.stage_frames, 0);
    assert_eq!(fs::read(&off_path).unwrap(), original_bytes);

    // Source-free Save must keep the original authored blob, not captured defaults.
    h.service.submit(Request::SaveCurrent).unwrap();
    let saved_off = edits::ordinary_terminal(&mut h);
    assert!(saved_off.error.is_none(), "{:?}", saved_off.error);
    assert!(saved_off.save_complete);
    let loaded = hex_storage::load(&off_path).unwrap();
    assert_eq!(loaded, off_project);
    assert_eq!(loaded.device(device).unwrap().state(), &saved.expected);
    assert!(loaded.device(device).unwrap().bypassed());
    assert_eq!(h.plugin_records.load(Acquire), 0);
    let off_source = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .recording_source()
        .unwrap();
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .paused_input_interval()
            .is_some()
    );

    // Missing code may refuse On, but cannot replace intentional Off with unavailable.
    let parked_catalog = saved.files.0.join("catalog-parked.json");
    fs::rename(&saved.catalog, &parked_catalog).unwrap();
    h.service
        .submit(Request::Edit(Command::SetDeviceBypass {
            device,
            bypassed: false,
        }))
        .unwrap();
    let refused = edits::ordinary_terminal(&mut h);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, saved_off.state.project);
    assert_eq!(refused.state.revision, saved_off.state.revision);
    assert_eq!(
        (refused.state.can_undo, refused.state.can_redo),
        (saved_off.state.can_undo, saved_off.state.can_redo)
    );
    assert_eq!(observed(&h, device), PreparedDeviceProcessing::PoweredOff);
    assert_eq!(h.plugin_records.load(Acquire), 0);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source(),
        Some(off_source)
    );
    fs::rename(&parked_catalog, &saved.catalog).unwrap();

    // A valid catalog On is prepared fresh, while canonical Off waits for its ACK.
    let pending = prepare_on_before_ack(&mut h, device, off_source);
    assert!(pending.error.is_none(), "{:?}", pending.error);
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, saved_off.state.project);
    assert_eq!(pending.state.revision, saved_off.state.revision);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source(),
        Some(off_source)
    );
    h.applied(off_source.audio_revision);
    let on = edits::ordinary_terminal(&mut h);
    assert!(on.error.is_none(), "{:?}", on.error);
    assert!(on.request_complete);
    assert!(!on.state.project.device(device).unwrap().bypassed());
    assert_eq!(
        on.state.project.device(device).unwrap().state(),
        &saved.expected
    );
    assert_eq!(on.state.revision, saved_off.state.revision + 1);
    assert_eq!(observed(&h, device), PreparedDeviceProcessing::Foreign);
    wait(|| h.plugin_records.load(Acquire) == 1);
    assert!(h.plugin_progress.lock().unwrap().binding.is_some());
    assert_eq!(h.plugin_completed.load(Acquire), 0);

    // Nondefault scale and a zero native processing counter distinguish fresh On.
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..3 {
        let pcm = h.render_plugin_block();
        wait(|| h.plugin_completed.load(Acquire) > block);
        assert_eq!(pcm, [[if block == 2 { 0.09375 } else { 0.0 }; 2]; 256]);
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn play_after_source_free_on_staging_keeps_canonical_off() {
    let _native = native_fixture();
    let saved = nondefault_fixture_with_frames(8192);
    let device = saved.project.main_devices()[0].id();
    let mut editor = hex_project::Editor::new(saved.project.clone(), 8).unwrap();
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let path = saved.files.0.join("off-race.hex");
    hex_storage::save_new(&path, editor.project()).unwrap();
    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        saved.catalog.clone(),
    );
    assert!(h.next().error.is_none());
    h.applied(0);
    h.enable_confirmed_device_inserts();
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let opened = h.run(Request::Open(path));
    super::super::super::resident_lifecycle::finish_replacement(&mut h, &opened, output);
    assert_eq!(observed(&h, device), PreparedDeviceProcessing::PoweredOff);
    let off_source = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .recording_source()
        .unwrap();
    let pending = prepare_on_before_ack(&mut h, device, off_source);
    assert!(pending.error.is_none(), "{:?}", pending.error);
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, opened.state.project);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let refused = edits::ordinary_terminal(&mut h);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, opened.state.project);
    assert_eq!(refused.state.revision, opened.state.revision);
    assert_eq!(
        (refused.state.can_undo, refused.state.can_redo),
        (opened.state.can_undo, opened.state.can_redo)
    );
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source(),
        Some(off_source)
    );
    // Off remains authoritative after refusal; running On must not take the old fallback.
    let records = h.plugin_records.load(Acquire);
    h.service
        .submit(Request::Edit(Command::ToggleDeviceBypass { device }))
        .unwrap();
    let running = edits::ordinary_terminal(&mut h);
    assert!(running.error.is_some());
    assert_eq!(running.state.project, opened.state.project);
    assert_eq!(h.plugin_records.load(Acquire), records);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Stop)
        .unwrap();
    wait(|| {
        h.render();
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .paused_input_interval()
            .is_some()
    });
    let retry = prepare_on_before_ack(&mut h, device, off_source);
    assert!(retry.error.is_none(), "{:?}", retry.error);
    assert!(!retry.request_complete);
    assert_eq!(retry.state.project, opened.state.project);
    h.applied(off_source.audio_revision);
    let installed = edits::ordinary_terminal(&mut h);
    assert!(installed.error.is_none(), "{:?}", installed.error);
    assert!(!installed.state.project.device(device).unwrap().bypassed());
    assert_eq!(installed.state.revision, opened.state.revision + 1);
}
