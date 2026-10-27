//! Actual Session/desktop/native Root workflow; no device or app execution.
mod state;
mod ui;
use super::resident_preview::ResidentHarness;
use super::*;
use hex_audio::plugin_effects::{EffectPresentation, Hosting};
use hex_project::{Command, Created};
use hex_session::service::{
    PluginEffectControl, PluginEffectDestination, PluginEffectInsertion, PluginEffectSelection,
};
use std::sync::atomic::Ordering::Release;

fn host() -> Hosting {
    Hosting::inspect_fixture_on_control(&PathBuf::from(
        std::env::var_os("HEX_VST3_INSTANCE_RECEIPT").unwrap(),
    ))
    .unwrap()
}
fn selection() -> PluginEffectSelection {
    let receipt = PathBuf::from(std::env::var_os("HEX_VST3_MACOS_RECEIPT").unwrap());
    let module = receipt
        .parent()
        .unwrap()
        .join("hex-vst3-host.app/Contents/Resources/fixtures/processing.vst3");
    let cid = [
        0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    ];
    let executable = "Contents/MacOS/fixture".to_string();
    let (code, info) = Hosting::inspect_fixture_binary(&module, &executable, cid).unwrap();
    let hex = |bytes: [u8; 32]| bytes.iter().map(|b| format!("{b:02x}")).collect();
    PluginEffectSelection {
        module,
        executable,
        cid,
        executable_sha256: hex(code),
        info_sha256: hex(info),
    }
}
fn request(
    state: &hex_session::service::ViewState,
    destination: PluginEffectDestination,
) -> PluginEffectInsertion {
    PluginEffectInsertion {
        context: AudioSourceContext {
            project_generation: state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context()
                .project_generation,
            editor_revision: state.revision,
        },
        selection: selection(),
        destination,
        before: None,
        control: Arc::new(PluginEffectControl::default()),
    }
}
fn wait(mut ready: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(
            Instant::now() < end,
            "actual native coordinator progress timed out"
        );
        std::thread::yield_now();
    }
}
fn submit_cancel_intent(h: &ResidentHarness, request: hex_session::device_gesture::GestureRequest) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match h
            .service
            .submit(Request::CancelPendingPluginParameter { request })
        {
            Ok(()) => return,
            Err(hex_session::service::SubmitError::Busy(_)) => {
                assert!(
                    Instant::now() < deadline,
                    "plugin cancel mailbox stayed busy"
                );
                std::thread::yield_now();
            }
            Err(error) => panic!("plugin cancel was not admitted: {error:?}"),
        }
    }
}
fn terminal(h: &mut ResidentHarness) -> Event {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render();
        if let Ok(event) = h.service.try_event() {
            if !event.plugin_effect_complete.is_empty() {
                return event;
            }
        }
        assert!(
            Instant::now() < end,
            "matching native insertion terminal timed out"
        );
        std::thread::yield_now();
    }
}

fn restored_terminal(h: &mut ResidentHarness) -> Event {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render();
        if let Ok(event) = h.service.try_event()
            && event.stopped_playback_restore_complete
        {
            return event;
        }
        assert!(Instant::now() < end, "stopped restore terminal timed out");
        std::thread::yield_now();
    }
}

/// These effect-only edits reuse an already installed immutable file domain.
/// Keep the simulated device held until the existing precommit worker queues
/// its graph; CPU scheduling must not choose the musical cut in a PCM oracle.
fn same_source_edit(h: &mut ResidentHarness, request: Request) -> Event {
    h.service.submit(request).unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(event.playback.is_none());
            return event;
        }
        assert!(
            Instant::now() < end,
            "same-source effect edit did not queue without advancing the device"
        );
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_controller_page_reaches_the_desktop_worker() {
    use hex_session::device_gesture::{
        GestureKey, GestureRequest, PluginParameterOrigin, PluginParameterValue,
    };
    use hex_session::service::{PluginControllerPageRequest, PluginControllerPageResult};
    // The original SDK fixture scales each 256-frame block by 0.5 + n/8,
    // then by its parameter gain. Constant 0.25 input therefore grows by
    // gain/32 per block. This checks delivered PCM rather than worker ACKs.
    fn assert_delivered_gain(h: &mut ResidentHarness, gain: f32, label: &str) {
        fn next_ready_block(
            h: &mut ResidentHarness,
            deadline: Instant,
            buffered: &mut usize,
            label: &str,
        ) -> [hex_engine::Frame; 256] {
            loop {
                h.pump();
                let status = h.audio.as_ref().unwrap().status().unwrap();
                let before = (
                    status.position.load(Acquire),
                    status.performance_frame.load(Acquire),
                );
                let block = h.render_plugin_block();
                state::workers_idle(h);
                if !h
                    .audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .stream_buffering
                    .load(Acquire)
                {
                    return block;
                }
                let status = h.audio.as_ref().unwrap().status().unwrap();
                assert_eq!(
                    status.stream_committed_frames.load(Acquire),
                    0,
                    "{label}: buffering must not commit a partial plugin callback"
                );
                assert_eq!(
                    (
                        status.position.load(Acquire),
                        status.performance_frame.load(Acquire)
                    ),
                    before,
                    "{label}: buffering must preserve both playback clocks"
                );
                assert!(
                    block.iter().all(|frame| *frame == [0.0; 2]),
                    "{label}: buffering must not expose a partial callback"
                );
                *buffered += 1;
                assert!(
                    Instant::now() < deadline,
                    "{label}: streamed input did not become ready"
                );
                std::thread::yield_now();
            }
        }
        eprintln!("diagnostic: checking {label} pcm");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
        {
            h.pump();
            h.render_plugin_block();
            state::workers_idle(h);
            assert!(
                Instant::now() < deadline,
                "{label}: actual playback did not start"
            );
            std::thread::yield_now();
        }
        let mut warm = [0.0; 8];
        let mut buffered = 0;
        for sample in &mut warm {
            *sample = next_ready_block(h, deadline, &mut buffered, label)[0][0];
        }
        let mut previous: Option<f32> = None;
        let mut changes = 0;
        let mut last_change = None;
        for block_index in 0..4 {
            let block = next_ready_block(h, deadline, &mut buffered, label);
            for (offset, frame) in block.iter().enumerate() {
                let value = frame[0];
                let status = h.audio.as_ref().unwrap().status().unwrap();
                assert!(
                    value.is_finite() && value > 0.0 && (frame[1] - value).abs() < 1e-6,
                    "{label}: frame={frame:?}, warm={warm:?}, position={}, buffering={}, rejected={:?}, fault={:?}",
                    status.position.load(Acquire),
                    status.stream_buffering.load(Acquire),
                    status.rejected_revision(),
                    status.render_fault_snapshot(),
                );
                if let Some(previous) = previous {
                    let delta = value - previous;
                    if delta.abs() >= 1e-6 {
                        let index = block_index * block.len() + offset;
                        assert!(
                            (delta - gain / 32.0_f32).abs() < 1e-5,
                            "{label}: previous={previous}, current={value}, gain={gain}"
                        );
                        if let Some(last) = last_change {
                            assert_eq!(index - last, 256, "{label}: native quantum changed");
                        }
                        last_change = Some(index);
                        changes += 1;
                    }
                }
                previous = Some(value);
            }
        }
        assert!(changes >= 3, "{label}: no observed native gain ramp");
        eprintln!("diagnostic: {label} observed {buffered} buffering callbacks");
    }
    if std::env::var_os("HEX_DESKTOP_CONTROLLER_DRIVER").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--test-threads=1"])
            .env("HEX_DESKTOP_CONTROLLER_DRIVER", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "91")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let files = Files::new();
    let mut catalog_selection = selection();
    let catalog_module = files.0.canonicalize().unwrap().join("original.vst3");
    ui::copy_original_module(&catalog_selection.module, &catalog_module);
    catalog_selection.module = catalog_module;
    hex_ui::write_plugin_effect_test_catalog(&files.0, &catalog_selection, "Fx", 0).unwrap();
    let catalog = files.0.canonicalize().unwrap().join("plugin-catalog.json");
    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        catalog,
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let state = h.run(Request::Import(files.wav(48_000, 480_000))).state;
    (applied, _) = h.applied(applied);
    let track = state.project.tracks()[0].id();
    h.service
        .submit(Request::InsertPluginEffect(request(
            &state,
            PluginEffectDestination::Track(track),
        )))
        .unwrap();
    assert!(h.next().plugin_effect_complete.is_empty());
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let Some(Created::Device(device)) = inserted.created else {
        panic!("plugin device")
    };
    (applied, _) = h.applied(applied);
    wait(|| h.plugin_progress.lock().unwrap().binding.is_some());
    let original_binding = h.plugin_progress.lock().unwrap().binding.unwrap();
    let context = AudioSourceContext {
        project_generation: inserted
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        editor_revision: inserted.state.revision,
    };
    let request = PluginControllerPageRequest {
        context,
        device_id: device.value(),
        first: 0,
        revision: 0,
    };
    let state = inserted
        .state
        .project
        .device(device)
        .unwrap()
        .state()
        .clone();
    *h.gesture_probe.controller_start.lock().unwrap() = Some((request, state));
    let end = Instant::now() + Duration::from_secs(10);
    let result = loop {
        h.pump();
        h.render();
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            break result;
        }
        assert!(Instant::now() < end, "desktop controller query timed out");
        std::thread::yield_now();
    };
    let PluginControllerPageResult::Page(page) = result.unwrap() else {
        panic!("actual controller page required")
    };
    assert_eq!(page.request, request);
    assert_ne!(page.revision, 0);
    assert!(!page.parameters.is_empty());
    assert_eq!(page.parameters[0].id, 7);
    assert_eq!(page.parameters[0].normalized, 0.25);
    assert!(page.parameters[0].editable);

    let origin = PluginParameterOrigin {
        context,
        device,
        parameter: 7,
        page_first: 0,
        metadata_revision: page.revision,
        initial: PluginParameterValue::new(0.5).unwrap(),
    };
    let stale_key = GestureKey {
        generation: context.project_generation,
        token: 70,
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: stale_key,
            origin,
        }))
        .unwrap();
    assert!(!h.next().request_complete);
    let refused = h.next();
    assert!(
        refused
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| !ack.accepted && ack.key == stale_key)
    );
    assert_eq!(refused.state.project, inserted.state.project);

    let key = GestureKey {
        generation: context.project_generation,
        token: 71,
    };
    let origin = PluginParameterOrigin {
        initial: PluginParameterValue::new(0.25).unwrap(),
        ..origin
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key,
            origin,
        }))
        .unwrap();
    assert!(!h.next().request_complete);
    let begun = h.next();
    assert!(
        begun
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted && ack.key == key)
    );
    assert_eq!(begun.state.project, inserted.state.project);
    let cancelled = h.run(Request::DeviceGesture(GestureRequest::Cancel { key }));
    assert!(
        cancelled
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted && ack.key == key)
    );
    assert_eq!(cancelled.state.project, inserted.state.project);
    let untouched = GestureKey {
        generation: context.project_generation,
        token: 72,
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: untouched,
            origin,
        }))
        .unwrap();
    assert!(!h.next().request_complete);
    let begun = h.next();
    assert!(
        begun
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted && ack.key == untouched)
    );
    let finished = h.run(Request::DeviceGesture(GestureRequest::CommitPlugin {
        key: untouched,
        sequence: 1,
        value: origin.initial,
    }));
    assert!(
        finished
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted && ack.key == untouched)
    );
    assert_eq!(finished.state.project, inserted.state.project);
    let held_key = GestureKey {
        generation: context.project_generation,
        token: 73,
    };
    let held_begin = GestureRequest::BeginPlugin {
        key: held_key,
        origin,
    };
    h.gesture_probe.hold_real_ready.store(true, Release);
    h.service
        .submit(Request::DeviceGesture(held_begin))
        .unwrap();
    assert!(!h.next().request_complete);
    submit_cancel_intent(&h, held_begin);
    h.service
        .submit_plugin_cancel(GestureRequest::Cancel { key: held_key })
        .unwrap();
    wait(|| {
        h.gesture_probe
            .cancelled
            .lock()
            .unwrap()
            .contains(&held_begin)
    });
    h.gesture_probe.hold_real_ready.store(false, Release);
    let begun = h.next();
    assert!(
        begun
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted && ack.key == held_key)
    );
    let cancelled = h.next();
    assert!(
        cancelled
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted && ack.key == held_key)
    );
    assert_eq!(cancelled.state.project, inserted.state.project);

    #[cfg(feature = "allocation-probe")]
    let held_page = {
        let source = inserted.state.canonical_audio_source.as_ref().unwrap();
        let path = source
            .required_sources()
            .find_map(|(_, binding)| match binding {
                hex_session::AudioSourceBinding::VerifiedFile(file) => Some(file.path().to_owned()),
                _ => None,
            })
            .expect("fixture must use its real streamed file");
        hex_stream::test_control::HeldPage::install(path, 1).unwrap()
    };
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.render_plugin_block();
    state::workers_idle(&h);
    #[cfg(feature = "allocation-probe")]
    {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
        {
            h.pump();
            h.render_plugin_block();
            state::workers_idle(&h);
            assert!(Instant::now() < deadline, "fixture playback did not start");
        }
        let position = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .position
            .load(Acquire);
        assert!(position < 3840, "page boundary reached before alignment");
        if position % 256 == 0 {
            h.render();
            state::workers_idle(&h);
        }
        let held_position = loop {
            h.pump();
            let status = h.audio.as_ref().unwrap().status().unwrap();
            let before = (
                status.position.load(Acquire),
                status.performance_frame.load(Acquire),
            );
            let output = h.render_plugin_block();
            state::workers_idle(&h);
            let status = h.audio.as_ref().unwrap().status().unwrap();
            if held_page.entered() && status.stream_buffering.load(Acquire) {
                assert_eq!(status.stream_committed_frames.load(Acquire), 0);
                assert_eq!(
                    (
                        status.position.load(Acquire),
                        status.performance_frame.load(Acquire)
                    ),
                    before
                );
                assert!(output.iter().all(|frame| *frame == [0.0; 2]));
                assert!(
                    before.0 < 4096 && before.0 + 256 > 4096,
                    "held page must begin inside the real callback: {before:?}"
                );
                break before;
            }
            assert!(
                Instant::now() < deadline,
                "actual page worker did not reach held boundary"
            );
            std::thread::yield_now();
        };
        held_page.release();
        loop {
            h.pump();
            h.render_plugin_block();
            state::workers_idle(&h);
            let status = h.audio.as_ref().unwrap().status().unwrap();
            if !status.stream_buffering.load(Acquire)
                && status.performance_frame.load(Acquire) > held_position.1
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "playback did not resume after page release"
            );
            std::thread::yield_now();
        }
        assert_eq!(
            h.plugin_progress.lock().unwrap().binding.unwrap(),
            original_binding
        );
    }
    assert_delivered_gain(&mut h, 1.0, "initial");
    let live_key = GestureKey {
        generation: context.project_generation,
        token: 74,
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: live_key,
            origin,
        }))
        .unwrap();
    assert!(!h.next().request_complete);
    let until = Instant::now() + Duration::from_secs(10);
    let live_begun = loop {
        let event = h.next();
        if event
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.key == live_key)
        {
            break event;
        }
        assert!(event.error.is_none(), "live begin: {:?}", event.error);
        assert!(Instant::now() < until, "live begin terminal timed out");
    };
    assert!(
        live_begun
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted && ack.key == live_key),
        "live begin: ack={:?}, error={:?}, request_complete={}",
        live_begun.gesture_ack,
        live_begun.error,
        live_begun.request_complete
    );
    h.service
        .submit(Request::DeviceGesture(GestureRequest::ChangePlugin {
            key: live_key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        }))
        .unwrap();
    let pending = h.next();
    assert!(
        !pending.request_complete && pending.gesture_ack.is_none(),
        "real SDK input must wait for its own terminal"
    );
    let until = Instant::now() + Duration::from_secs(10);
    let point_applied = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event() {
            if event
                .gesture_ack
                .as_ref()
                .is_some_and(|ack| ack.key == live_key)
            {
                break event;
            }
            assert!(event.error.is_none(), "live point: {:?}", event.error);
        }
        assert!(Instant::now() < until, "actual SDK point did not complete");
        std::thread::yield_now();
    };
    assert!(
        point_applied
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted && ack.key == live_key && ack.sequence == 1)
    );
    assert_eq!(point_applied.state.project, inserted.state.project);
    // Force contention on the real Root work owner across restore admission.
    // A temporary busy response must retain the exact restore, not refuse it.
    let (held, observed_hold) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let holder = h.hold_native_work_on_control(held, released);
    observed_hold.recv_timeout(Duration::from_secs(10)).unwrap();
    let attempts = h.gesture_probe.real_restore_attempts.load(Acquire);
    h.service
        .submit_plugin_cancel(GestureRequest::Cancel { key: live_key })
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while h.gesture_probe.real_restore_attempts.load(Acquire) == attempts {
        h.pump();
        assert!(
            Instant::now() < until,
            "contended restore was not attempted"
        );
        std::thread::yield_now();
    }
    let mut premature = None;
    while let Ok(event) = h.service.try_event() {
        if event
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.key == live_key)
        {
            premature = event.gesture_ack;
        }
    }
    release.send(()).unwrap();
    holder.join().unwrap();
    assert!(
        !h.gesture_probe.real_restore_busy_refused.load(Acquire),
        "temporary native contention was reported as refusal"
    );
    assert!(
        premature.is_none(),
        "busy restore must remain pending: {premature:?}"
    );
    let until = Instant::now() + Duration::from_secs(10);
    let restored = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event() {
            if event
                .gesture_ack
                .as_ref()
                .is_some_and(|ack| ack.key == live_key)
            {
                break event;
            }
            assert!(event.error.is_none(), "live restore: {:?}", event.error);
        }
        assert!(
            Instant::now() < until,
            "actual SDK restore did not complete"
        );
        std::thread::yield_now();
    };
    assert!(
        restored
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted && ack.key == live_key),
        "actual restore: {:?}",
        restored.gesture_ack
    );
    assert_eq!(restored.state.project, inserted.state.project);

    let state = inserted
        .state
        .project
        .device(device)
        .unwrap()
        .state()
        .clone();
    // Hold the actual Root Work owner across query admission. The pending
    // request must survive contention and later reach the same real worker.
    let (held, observed_hold) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let holder = h.hold_native_work_on_control(held, released);
    observed_hold.recv_timeout(Duration::from_secs(10)).unwrap();
    *h.gesture_probe.controller_start.lock().unwrap() = Some((request, state));
    let held_until = Instant::now() + Duration::from_secs(10);
    while h.gesture_probe.controller_active.lock().unwrap().is_none() {
        assert!(
            h.gesture_probe.controller_result.lock().unwrap().is_none(),
            "transient Root contention must not refuse the query"
        );
        assert!(
            Instant::now() < held_until,
            "controller admission did not remain pending"
        );
        std::thread::yield_now();
    }
    assert!(h.gesture_probe.controller_result.lock().unwrap().is_none());
    release.send(()).unwrap();
    holder.join().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let refreshed = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("refreshed actual controller page required");
            };
            break page;
        }
        assert!(
            Instant::now() < until,
            "refreshed controller page timed out"
        );
        std::thread::yield_now();
    };
    assert_eq!(refreshed.parameters[0].id, 7);
    assert_eq!(
        refreshed.parameters[0].normalized, 0.25,
        "controller readback after restore, revision {}",
        refreshed.revision
    );
    let origin = PluginParameterOrigin {
        metadata_revision: refreshed.revision,
        initial: PluginParameterValue::new(refreshed.parameters[0].normalized).unwrap(),
        ..origin
    };

    let held_point_key = GestureKey {
        generation: context.project_generation,
        token: 75,
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: held_point_key,
            origin,
        }))
        .unwrap();
    assert!(!h.next().request_complete);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let event = h.next();
        if let Some(ack) = event
            .gesture_ack
            .as_ref()
            .filter(|ack| ack.key == held_point_key)
        {
            assert!(ack.accepted, "held point begin: {ack:?}");
            break;
        }
        assert!(event.error.is_none(), "held point begin: {:?}", event.error);
        assert!(Instant::now() < until);
    }
    let held_change = GestureRequest::ChangePlugin {
        key: held_point_key,
        sequence: 1,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    h.gesture_probe.observed_real_ready.store(false, Release);
    h.gesture_probe.hold_real_ready.store(true, Release);
    h.service
        .submit(Request::DeviceGesture(held_change))
        .unwrap();
    assert!(!h.next().request_complete);
    let until = Instant::now() + Duration::from_secs(10);
    while !h.gesture_probe.observed_real_ready.load(Acquire) {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(Instant::now() < until, "real point terminal did not arrive");
        std::thread::yield_now();
    }
    submit_cancel_intent(&h, held_change);
    h.service
        .submit_plugin_cancel(GestureRequest::Cancel {
            key: held_point_key,
        })
        .unwrap();
    wait(|| {
        h.gesture_probe
            .cancelled
            .lock()
            .unwrap()
            .contains(&held_change)
    });
    h.gesture_probe.hold_real_ready.store(false, Release);
    let until = Instant::now() + Duration::from_secs(10);
    let mut saw_change = false;
    let mut saw_restore = false;
    while !saw_restore {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event() {
            if let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == held_point_key)
            {
                assert!(ack.accepted, "held point terminal: {ack:?}");
                match ack.phase {
                    hex_session::device_gesture::GesturePhase::Change => saw_change = true,
                    hex_session::device_gesture::GesturePhase::Cancel => {
                        assert!(saw_change, "restore cannot precede the processed point");
                        saw_restore = true;
                    }
                    _ => {}
                }
            }
            assert!(event.error.is_none(), "held point: {:?}", event.error);
            assert_eq!(event.state.project, inserted.state.project);
        }
        assert!(Instant::now() < until, "held point cancellation timed out");
        std::thread::yield_now();
    }

    let state = inserted
        .state
        .project
        .device(device)
        .unwrap()
        .state()
        .clone();
    *h.gesture_probe.controller_start.lock().unwrap() = Some((request, state));
    let until = Instant::now() + Duration::from_secs(10);
    let page = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("controller page after cancellation required");
            };
            break page;
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    assert_eq!(page.parameters[0].normalized, 0.25);
    let origin = PluginParameterOrigin {
        metadata_revision: page.revision,
        initial: PluginParameterValue::new(0.25).unwrap(),
        ..origin
    };
    // Cancellation of a point held before SDK admission must never apply that
    // point, and must complete the keyed restore without changing history.
    let unadmitted_key = GestureKey {
        generation: context.project_generation,
        token: 76,
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: unadmitted_key,
            origin,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == unadmitted_key)
        {
            assert!(ack.accepted, "unadmitted point begin: {ack:?}");
            break;
        }
        assert!(Instant::now() < until, "unadmitted point begin timed out");
        std::thread::yield_now();
    }
    let (held, observed_hold) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let holder = h.hold_native_work_on_control(held, released);
    observed_hold.recv_timeout(Duration::from_secs(10)).unwrap();
    let attempts = h.gesture_probe.real_preview_attempts.load(Acquire);
    let unadmitted = GestureRequest::ChangePlugin {
        key: unadmitted_key,
        sequence: 1,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    h.service
        .submit(Request::DeviceGesture(unadmitted))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while h.gesture_probe.real_preview_attempts.load(Acquire) == attempts {
        h.pump();
        assert!(Instant::now() < until, "contended point was not attempted");
        std::thread::yield_now();
    }
    submit_cancel_intent(&h, unadmitted);
    h.service
        .submit_plugin_cancel(GestureRequest::Cancel {
            key: unadmitted_key,
        })
        .unwrap();
    wait(|| {
        h.gesture_probe
            .cancelled
            .lock()
            .unwrap()
            .contains(&unadmitted)
    });
    release.send(()).unwrap();
    holder.join().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let mut saw_refusal = false;
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == unadmitted_key)
        {
            assert_eq!(event.state.project, inserted.state.project);
            match ack.phase {
                hex_session::device_gesture::GesturePhase::Change => {
                    assert!(!ack.accepted, "unadmitted point must not claim application");
                    saw_refusal = true;
                }
                hex_session::device_gesture::GesturePhase::Cancel => {
                    assert!(saw_refusal && ack.accepted, "unadmitted cancel: {ack:?}");
                    break;
                }
                _ => {}
            }
        }
        assert!(Instant::now() < until, "unadmitted cancellation timed out");
        std::thread::yield_now();
    }
    let state = inserted
        .state
        .project
        .device(device)
        .unwrap()
        .state()
        .clone();
    *h.gesture_probe.controller_start.lock().unwrap() = Some((request, state));
    let until = Instant::now() + Duration::from_secs(10);
    let unchanged = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("unadmitted point page required")
            };
            break page;
        }
        assert!(
            Instant::now() < until,
            "unadmitted point readback timed out"
        );
        std::thread::yield_now();
    };
    assert_eq!(unchanged.parameters[0].normalized, 0.25);
    let origin = PluginParameterOrigin {
        metadata_revision: unchanged.revision,
        ..origin
    };
    let refused_key = GestureKey {
        generation: context.project_generation,
        token: 77,
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: refused_key,
            origin,
        }))
        .unwrap();
    assert!(!h.next().request_complete);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let event = h.next();
        if let Some(ack) = event
            .gesture_ack
            .as_ref()
            .filter(|ack| ack.key == refused_key)
        {
            assert!(ack.accepted, "refused commit begin: {ack:?}");
            break;
        }
        assert!(
            event.error.is_none(),
            "refused commit begin: {:?}",
            event.error
        );
        assert!(Instant::now() < until);
    }
    let (graph_ready, graph_resume, _busy) = h.probe_next_native_publication();
    h.gesture_probe.hold_commit_finish.store(true, Release);
    let refused_commit = GestureRequest::CommitPlugin {
        key: refused_key,
        sequence: 1,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    h.service
        .submit(Request::DeviceGesture(refused_commit))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while graph_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(
            Instant::now() < until,
            "refused commit did not reach its graph gate"
        );
        std::thread::yield_now();
    }
    graph_resume.send(()).unwrap();
    let rejected_revision = loop {
        h.pump();
        if let Some((audio_revision, metadata)) = h.submitted
            && metadata.context
                == (hex_engine::AudioSourceContext {
                    editor_revision: context.editor_revision + 1,
                    ..context
                })
            && audio_revision > applied
        {
            break audio_revision;
        }
        assert!(Instant::now() < until, "refused commit was not submitted");
        std::thread::yield_now();
    };
    submit_cancel_intent(&h, refused_commit);
    h.service
        .submit_plugin_cancel(GestureRequest::Cancel { key: refused_key })
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !h
        .gesture_probe
        .cancelled
        .lock()
        .unwrap()
        .contains(&refused_commit)
    {
        h.pump();
        assert!(
            Instant::now() < until,
            "queued commit cancellation was not observed"
        );
        std::thread::yield_now();
    }
    while let Ok(event) = h.service.try_event() {
        assert_eq!(event.state.project, inserted.state.project);
        assert!(
            !event
                .gesture_ack
                .as_ref()
                .is_some_and(|ack| ack.key == refused_key),
            "cancellation must wait for the submitted graph terminal"
        );
    }
    let restore_attempts = h.gesture_probe.real_restore_attempts.load(Acquire);
    assert!(
        h.callback
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .reject_publication_on_callback(rejected_revision)
    );
    let until = Instant::now() + Duration::from_secs(10);
    let rejected = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && event
                .gesture_ack
                .as_ref()
                .is_some_and(|ack| ack.key == refused_key)
        {
            break event;
        }
        assert!(Instant::now() < until, "refused commit terminal timed out");
        std::thread::yield_now();
    };
    assert!(!rejected.gesture_ack.as_ref().unwrap().accepted);
    assert_eq!(rejected.state.project, inserted.state.project);
    assert_eq!(rejected.state.revision, inserted.state.revision);
    assert_eq!(rejected.state.can_undo, inserted.state.can_undo);
    assert!(
        h.gesture_probe
            .pending_commit_finish
            .lock()
            .unwrap()
            .is_some()
    );
    let until = Instant::now() + Duration::from_secs(10);
    while h.gesture_probe.real_restore_attempts.load(Acquire) == restore_attempts {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(
            Instant::now() < until,
            "held state approval did not reach restore"
        );
        std::thread::yield_now();
    }
    assert!(
        h.gesture_probe.real_restore_pending.load(Acquire),
        "restore must wait for the retained approval"
    );
    assert!(
        h.gesture_probe
            .pending_commit_finish
            .lock()
            .unwrap()
            .is_some()
    );
    h.gesture_probe.hold_commit_finish.store(false, Release);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event.gesture_ack.as_ref().filter(|ack| {
                ack.key == refused_key
                    && ack.phase == hex_session::device_gesture::GesturePhase::Cancel
            })
        {
            assert!(ack.accepted, "refused commit restore: {ack:?}");
            assert_eq!(event.state.project, inserted.state.project);
            break;
        }
        assert!(Instant::now() < until, "refused commit restore timed out");
        std::thread::yield_now();
    }
    let state = inserted
        .state
        .project
        .device(device)
        .unwrap()
        .state()
        .clone();
    *h.gesture_probe.controller_start.lock().unwrap() = Some((request, state));
    let until = Instant::now() + Duration::from_secs(10);
    let restored_page = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("controller page after refused commit required");
            };
            break page;
        }
        assert!(Instant::now() < until, "restored controller page timed out");
        std::thread::yield_now();
    };
    assert_eq!(restored_page.parameters[0].normalized, 0.25);
    assert_delivered_gain(&mut h, 1.0, "refused commit restored");
    let origin = PluginParameterOrigin {
        metadata_revision: restored_page.revision,
        initial: PluginParameterValue::new(restored_page.parameters[0].normalized).unwrap(),
        ..origin
    };
    applied = rejected_revision;
    let commit_key = GestureKey {
        generation: context.project_generation,
        token: 78,
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: commit_key,
            origin,
        }))
        .unwrap();
    assert!(!h.next().request_complete);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let event = h.next();
        if let Some(ack) = event
            .gesture_ack
            .as_ref()
            .filter(|ack| ack.key == commit_key)
        {
            assert!(ack.accepted, "commit begin: {ack:?}");
            break;
        }
        assert!(event.error.is_none(), "commit begin: {:?}", event.error);
        assert!(Instant::now() < until);
    }
    // Complete the preview first so the following request exercises commit
    // admission itself while its source observation is contended.
    h.service
        .submit(Request::DeviceGesture(GestureRequest::ChangePlugin {
            key: commit_key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == commit_key)
        {
            assert!(ack.accepted, "commit contention preview: {ack:?}");
            assert_eq!(event.state.project, inserted.state.project);
            break;
        }
        assert!(
            Instant::now() < until,
            "commit contention preview timed out"
        );
    }
    let (graph_ready, graph_resume, _busy) = h.probe_next_native_publication();
    let commit_attempts = h.gesture_probe.real_commit_attempts.load(Acquire);
    h.audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .hold_recording_source_observation_for_test(true);
    let accepted_commit = GestureRequest::CommitPlugin {
        key: commit_key,
        sequence: 1,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    h.service
        .submit(Request::DeviceGesture(accepted_commit))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while h.gesture_probe.real_commit_attempts.load(Acquire) == commit_attempts {
        h.pump();
        h.render_plugin_block();
        assert!(Instant::now() < until, "contended commit was not attempted");
        std::thread::yield_now();
    }
    assert!(
        h.gesture_probe.real_commit_pending.load(Acquire),
        "contended commit must remain pending"
    );
    h.audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .hold_recording_source_observation_for_test(false);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if graph_ready.try_recv().is_ok() {
            break;
        }
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(
            Instant::now() < until,
            "plugin graph preparation did not reach its gate"
        );
        std::thread::yield_now();
    }
    graph_resume.send(()).unwrap();
    // The worker can submit but no callback is advanced after the gate opens.
    // Queue admission is not a canonical commit or a gesture terminal.
    let queued_revision = loop {
        h.pump();
        if let Some((audio_revision, metadata)) = h.submitted
            && metadata.context.project_generation == context.project_generation
            && metadata.context.editor_revision == context.editor_revision + 1
            && audio_revision > applied
        {
            break audio_revision;
        }
        assert!(Instant::now() < until, "plugin graph was not submitted");
        std::thread::yield_now();
    };
    assert_ne!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        queued_revision,
        "fixture must hold the callback before graph installation"
    );
    while let Ok(event) = h.service.try_event() {
        assert_eq!(event.state.project, inserted.state.project);
        assert!(
            !event
                .gesture_ack
                .as_ref()
                .is_some_and(|ack| ack.key == commit_key && ack.accepted),
            "submitted graph must not acknowledge the canonical commit"
        );
    }
    submit_cancel_intent(&h, accepted_commit);
    h.service
        .submit_plugin_cancel(GestureRequest::Cancel { key: commit_key })
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !h
        .gesture_probe
        .cancelled
        .lock()
        .unwrap()
        .contains(&accepted_commit)
    {
        h.pump();
        assert!(
            Instant::now() < until,
            "accepted commit cancellation was not observed"
        );
        std::thread::yield_now();
    }
    let until = Instant::now() + Duration::from_secs(10);
    let committed = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event() {
            if event
                .gesture_ack
                .as_ref()
                .is_some_and(|ack| ack.key == commit_key)
            {
                break event;
            }
            assert!(event.error.is_none(), "commit: {:?}", event.error);
        }
        assert!(Instant::now() < until, "plugin commit terminal timed out");
        std::thread::yield_now();
    };
    assert!(
        committed
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted && ack.key == commit_key),
        "plugin commit: {:?}",
        committed.gesture_ack
    );
    assert_ne!(
        committed.state.project.device(device).unwrap().state(),
        inserted.state.project.device(device).unwrap().state()
    );
    let captured =
        hex_plugin_state::decode(committed.state.project.device(device).unwrap().state()).unwrap();
    assert_eq!(&captured.component.unwrap()[8..12], &3.0_f32.to_le_bytes());
    assert!(committed.state.can_undo);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event.gesture_ack.as_ref().filter(|ack| {
                ack.key == commit_key
                    && ack.phase == hex_session::device_gesture::GesturePhase::Cancel
            })
        {
            assert!(
                !ack.accepted,
                "an already applied commit cannot claim cancellation"
            );
            assert_eq!(event.state.project, committed.state.project);
            break;
        }
        assert!(
            Instant::now() < until,
            "post-commit cancellation terminal timed out"
        );
        std::thread::yield_now();
    }
    (applied, _) = h.applied(applied);
    eprintln!(
        "diagnostic: committed revision {} applied as {}",
        committed.state.revision, applied
    );
    assert_eq!(
        h.plugin_progress.lock().unwrap().binding,
        Some(original_binding)
    );
    assert_eq!(
        h.plugin_records.load(Acquire),
        1,
        "commit must keep the original native source"
    );
    assert_delivered_gain(&mut h, 3.0, "committed");
    // Refuse one exact undo at the real callback boundary. The project and
    // history must stay committed, and the next undo must remain usable.
    let (graph_ready, graph_resume, _busy) = h.probe_next_native_publication();
    h.service.submit(Request::Undo).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while graph_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(
            Instant::now() < until,
            "undo preparation did not reach its gate"
        );
        std::thread::yield_now();
    }
    graph_resume.send(()).unwrap();
    let refused_revision = loop {
        h.pump();
        if let Some((revision, metadata)) = h.submitted
            && metadata.context.editor_revision == committed.state.revision + 1
            && revision > applied
        {
            break revision;
        }
        assert!(Instant::now() < until, "undo graph was not submitted");
        std::thread::yield_now();
    };
    assert!(
        h.callback
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .reject_publication_on_callback(refused_revision)
    );
    let refused = state::ordinary_terminal(&mut h);
    assert!(
        refused.error.is_some(),
        "forced callback refusal must be reported"
    );
    assert!(
        refused
            .error
            .as_deref()
            .unwrap()
            .contains("runtimefencechanged")
    );
    assert_eq!(refused.state.project, committed.state.project);
    assert_eq!(refused.state.revision, committed.state.revision);
    assert!(refused.state.can_undo, "refusal must retain the undo edge");
    assert_eq!(refused.state.can_redo, committed.state.can_redo);
    // Future observations must be newer than the refused submission too.
    applied = refused_revision;
    let mut current_revision = committed.state.revision;
    for (request, before, after, gain) in [
        (
            Request::Undo,
            &committed.state.project,
            &inserted.state.project,
            1.0,
        ),
        (
            Request::Redo,
            &inserted.state.project,
            &committed.state.project,
            3.0,
        ),
    ] {
        let (graph_ready, graph_resume, _busy) = h.probe_next_native_publication();
        h.service.submit(request).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while graph_ready.try_recv().is_err() {
            h.pump();
            h.render_plugin_block();
            state::workers_idle(&h);
            assert!(
                Instant::now() < until,
                "history preparation did not reach its gate"
            );
            std::thread::yield_now();
        }
        graph_resume.send(()).unwrap();
        let queued_revision = loop {
            h.pump();
            if let Some((audio_revision, metadata)) = h.submitted
                && metadata.context.editor_revision == current_revision + 1
                && audio_revision > applied
            {
                break audio_revision;
            }
            assert!(Instant::now() < until, "history graph was not submitted");
            std::thread::yield_now();
        };
        assert_ne!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire),
            queued_revision
        );
        while let Ok(event) = h.service.try_event() {
            assert_eq!(&event.state.project, before);
            assert!(
                !event.request_complete,
                "history completed before callback installation"
            );
        }
        let finished = state::ordinary_terminal(&mut h);
        assert!(finished.error.is_none(), "history: {:?}", finished.error);
        assert_eq!(&finished.state.project, after);
        current_revision = finished.state.revision;
        (applied, _) = h.applied(applied);
        assert_delivered_gain(&mut h, gain, "history");
    }
    // An ordinary Undo is also a gesture barrier: it must first commit the
    // live plugin point, then undo that exact edit after its callback terminal.
    let save_path = files.0.join("plugin-gesture-barrier.hex");
    let mut saved_plugin_state = None;
    for (force_refusal, save, token) in [(true, false, 79), (false, false, 80), (false, true, 81)] {
        let barrier_context = AudioSourceContext {
            editor_revision: current_revision,
            ..context
        };
        let barrier_request = PluginControllerPageRequest {
            context: barrier_context,
            revision: 0,
            ..request
        };
        let state = committed
            .state
            .project
            .device(device)
            .unwrap()
            .state()
            .clone();
        *h.gesture_probe.controller_start.lock().unwrap() = Some((barrier_request, state));
        let until = Instant::now() + Duration::from_secs(10);
        let page = loop {
            h.pump();
            h.render_plugin_block();
            state::workers_idle(&h);
            if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
                let PluginControllerPageResult::Page(page) = result.unwrap() else {
                    panic!("barrier page required")
                };
                break page;
            }
            assert!(Instant::now() < until, "barrier page timed out");
            std::thread::yield_now();
        };
        assert_eq!(page.parameters[0].normalized, 0.75);
        let barrier_key = GestureKey {
            generation: context.project_generation,
            token,
        };
        let barrier_origin = PluginParameterOrigin {
            context: barrier_context,
            metadata_revision: page.revision,
            initial: PluginParameterValue::new(0.75).unwrap(),
            ..origin
        };
        h.service
            .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
                key: barrier_key,
                origin: barrier_origin,
            }))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            h.pump();
            h.render_plugin_block();
            state::workers_idle(&h);
            if let Ok(event) = h.service.try_event()
                && let Some(ack) = event
                    .gesture_ack
                    .as_ref()
                    .filter(|ack| ack.key == barrier_key)
            {
                assert!(ack.accepted, "barrier begin: {ack:?}");
                break;
            }
            assert!(Instant::now() < until, "barrier begin timed out");
            std::thread::yield_now();
        }
        h.service
            .submit(Request::DeviceGesture(GestureRequest::ChangePlugin {
                key: barrier_key,
                sequence: 1,
                value: PluginParameterValue::new(0.5).unwrap(),
            }))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            h.pump();
            h.render_plugin_block();
            state::workers_idle(&h);
            if let Ok(event) = h.service.try_event()
                && let Some(ack) = event
                    .gesture_ack
                    .as_ref()
                    .filter(|ack| ack.key == barrier_key && ack.sequence == 1)
            {
                assert!(ack.accepted, "barrier preview: {ack:?}");
                break;
            }
            assert!(Instant::now() < until, "barrier preview timed out");
            std::thread::yield_now();
        }
        let (graph_ready, graph_resume, _busy) = h.probe_next_native_publication();
        h.service
            .submit(if save {
                Request::SaveNew(save_path.clone())
            } else {
                Request::Undo
            })
            .unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while graph_ready.try_recv().is_err() {
            h.pump();
            h.render_plugin_block();
            state::workers_idle(&h);
            if let Ok(event) = h.service.try_event() {
                assert!(event.error.is_none(), "barrier refused: {:?}", event.error);
                assert!(
                    !event.request_complete,
                    "barrier completed before its plugin publication"
                );
            }
            assert!(
                Instant::now() < until,
                "barrier commit did not reach graph gate"
            );
            std::thread::yield_now();
        }
        graph_resume.send(()).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        let barrier_revision = loop {
            h.pump();
            if let Some((audio_revision, metadata)) = h.submitted
                && metadata.context.editor_revision == current_revision + 1
                && audio_revision > applied
            {
                break audio_revision;
            }
            assert!(Instant::now() < until, "barrier commit not submitted");
            std::thread::yield_now();
        };
        while let Ok(event) = h.service.try_event() {
            assert_eq!(event.state.project, committed.state.project);
            assert!(!event.request_complete, "barrier completed before callback");
        }
        if save {
            assert!(
                !save_path.exists(),
                "save must wait for the plugin callback"
            );
        }
        if force_refusal {
            assert!(
                h.callback
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .reject_publication_on_callback(barrier_revision)
            );
            let refused = state::ordinary_terminal(&mut h);
            assert!(
                refused
                    .error
                    .as_deref()
                    .is_some_and(|reason| reason.contains("runtimefencechanged")),
                "barrier refusal: {:?}",
                refused.error
            );
            assert_eq!(refused.state.project, committed.state.project);
            assert_eq!(refused.state.revision, current_revision);
            assert!(
                refused.state.can_undo,
                "refused barrier must preserve earlier undo history"
            );
            h.service
                .submit_plugin_cancel(GestureRequest::Cancel { key: barrier_key })
                .unwrap();
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                h.pump();
                h.render_plugin_block();
                state::workers_idle(&h);
                if let Ok(event) = h.service.try_event()
                    && let Some(ack) = event.gesture_ack.as_ref().filter(|ack| {
                        ack.key == barrier_key
                            && ack.phase == hex_session::device_gesture::GesturePhase::Cancel
                    })
                {
                    assert!(ack.accepted, "refused barrier restore: {ack:?}");
                    assert_eq!(event.state.project, committed.state.project);
                    break;
                }
                assert!(Instant::now() < until, "refused barrier restore timed out");
                std::thread::yield_now();
            }
            applied = barrier_revision;
            continue;
        }
        let finished = state::ordinary_terminal(&mut h);
        assert!(
            finished.error.is_none(),
            "barrier request: {:?}",
            finished.error
        );
        if save {
            assert!(finished.save_complete);
            assert_eq!(finished.state.revision, current_revision + 1);
            let loaded = hex_storage::load(&save_path).unwrap();
            assert_eq!(
                loaded.device(device).unwrap().state(),
                finished.state.project.device(device).unwrap().state()
            );
            saved_plugin_state = Some(loaded.device(device).unwrap().state().clone());
            let captured =
                hex_plugin_state::decode(loaded.device(device).unwrap().state()).unwrap();
            assert_eq!(&captured.component.unwrap()[8..12], &2.0_f32.to_le_bytes());
        } else {
            assert_eq!(finished.state.project, committed.state.project);
            assert_eq!(finished.state.revision, current_revision + 2);
            assert!(finished.state.can_redo);
            current_revision = finished.state.revision;
            (applied, _) = h.applied(barrier_revision);
        }
    }
    let expected = saved_plugin_state.unwrap();
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let opened = h.run(Request::Open(save_path.clone()));
    assert!(
        opened.error.is_none() && opened.project_replaced && opened.open_complete,
        "saved plugin reopen: {:?}",
        opened.error
    );
    assert_eq!(
        opened.state.project.device(device).unwrap().state(),
        &expected
    );
    assert!(!opened.state.can_undo && !opened.state.can_redo);
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    wait(|| {
        h.pump();
        let progress = h.plugin_progress.lock().unwrap();
        assert!(
            !progress.restore_refused,
            "saved plugin restoration refused: {progress:?}"
        );
        drop(progress);
        h.plugin_queued.load(Acquire)
    });
    h.applied(0);
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .foreign_processor(device.value(), expected.type_id()),
        Some(true)
    );
    let reopened_context = opened
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context();
    let reopened_request = PluginControllerPageRequest {
        context: reopened_context,
        revision: 0,
        ..request
    };
    *h.gesture_probe.controller_start.lock().unwrap() = Some((reopened_request, expected));
    let until = Instant::now() + Duration::from_secs(10);
    let page = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("reopened plugin controller page required")
            };
            break page;
        }
        assert!(
            Instant::now() < until,
            "reopened plugin controller timed out"
        );
        std::thread::yield_now();
    };
    assert_eq!(page.parameters[0].normalized, 0.5);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_delivered_gain(&mut h, 2.0, "reopened saved plugin");

    // Lose the actual output while a plugin commit is queued, after an
    // explicit cancellation. The old callback is closed without rendering
    // the candidate; the fresh output must restore the last canonical state
    // and release the original gesture and the following FIFO request.
    let loss_key = GestureKey {
        generation: reopened_context.project_generation,
        token: 1,
    };
    let loss_origin = PluginParameterOrigin {
        context: reopened_context,
        metadata_revision: page.revision,
        initial: PluginParameterValue::new(0.5).unwrap(),
        ..origin
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: loss_key,
            origin: loss_origin,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event.gesture_ack.as_ref().filter(|ack| ack.key == loss_key)
        {
            assert!(ack.accepted, "output-loss begin: {ack:?}");
            break;
        }
        assert!(Instant::now() < until, "output-loss begin timed out");
        std::thread::yield_now();
    }
    let (graph_ready, graph_resume, _busy) = h.probe_next_native_publication();
    let loss_commit = GestureRequest::CommitPlugin {
        key: loss_key,
        sequence: 1,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    h.service
        .submit(Request::DeviceGesture(loss_commit))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while graph_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(
            Instant::now() < until,
            "output-loss commit did not reach graph gate"
        );
        std::thread::yield_now();
    }
    graph_resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        if let Some((_, metadata)) = h.submitted
            && metadata.context.editor_revision == reopened_context.editor_revision + 1
        {
            break;
        }
        assert!(Instant::now() < until, "output-loss commit not submitted");
        std::thread::yield_now();
    }
    submit_cancel_intent(&h, loss_commit);
    h.service
        .submit_plugin_cancel(GestureRequest::Cancel { key: loss_key })
        .unwrap();
    wait(|| {
        h.gesture_probe
            .cancelled
            .lock()
            .unwrap()
            .contains(&loss_commit)
    });
    h.service.submit(Request::Undo).unwrap();
    let lost_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.gesture_probe
        .restart_generation
        .store(reopened_context.project_generation + 1, Release);
    wait(|| h.gesture_probe.restart_result.lock().unwrap().is_some());
    assert!(
        h.gesture_probe
            .restart_result
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .is_ok()
    );
    let restore_finishes_before = h.gesture_probe.restore_finishes.lock().unwrap().len();
    let (recovery_ready, recovery_resume, _busy) = h.probe_next_native_publication();
    h.release_relinquished_output();
    h.wait_fresh_output(lost_output);
    let until = Instant::now() + Duration::from_secs(10);
    while !h.gesture_probe.restore_ready.load(Acquire) {
        h.pump();
        assert!(
            Instant::now() < until,
            "fresh-output plugin restoration did not become ready"
        );
        std::thread::yield_now();
    }
    let until = Instant::now() + Duration::from_secs(10);
    while recovery_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == loss_key),
                "retired gesture must wait for fresh graph installation"
            );
            assert!(
                !event.request_complete,
                "queued requests must wait for fresh graph installation"
            );
        }
        assert!(
            Instant::now() < until,
            "fresh recovery did not reach graph gate"
        );
        std::thread::yield_now();
    }
    recovery_resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        if h.submitted
            .is_some_and(|(_, metadata)| metadata.context == reopened_context)
        {
            break;
        }
        assert!(Instant::now() < until, "fresh recovery graph not submitted");
        std::thread::yield_now();
    }
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        0
    );
    assert_eq!(
        h.gesture_probe.restore_finishes.lock().unwrap().len(),
        restore_finishes_before,
        "fresh recovery cannot finish before callback installation"
    );
    let until = Instant::now() + Duration::from_secs(5);
    let mut gesture_source_lost = false;
    loop {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert_eq!(event.state.project, opened.state.project);
            if let Some(ack) = event.gesture_ack.as_ref().filter(|ack| ack.key == loss_key) {
                assert!(
                    !ack.accepted || ack.phase == hex_session::device_gesture::GesturePhase::Cancel
                );
                if ack.phase == hex_session::device_gesture::GesturePhase::Commit {
                    assert!(
                        !ack.accepted && ack.source_lost,
                        "retired commit terminal: {ack:?}"
                    );
                    gesture_source_lost = true;
                }
            }
            if event.request_complete && event.gesture_ack.is_none() {
                assert!(
                    gesture_source_lost,
                    "queued Undo must follow gesture resolution"
                );
                assert!(event.error.is_none(), "queued Undo: {:?}", event.error);
                break;
            }
        }
        assert!(
            Instant::now() < until,
            "output loss stranded the pending gesture and FIFO"
        );
        std::thread::yield_now();
    }
    let source = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .recording_source()
        .unwrap();
    assert_eq!(source.context, reopened_context);
    assert_ne!(source.output_identity, lost_output);
    let finishes = h.gesture_probe.restore_finishes.lock().unwrap();
    assert!(finishes.len() > restore_finishes_before);
    assert_eq!(finishes.last(), Some(&(reopened_context, true)));
    drop(finishes);
    let unchanged = opened.state.project.device(device).unwrap().state().clone();
    *h.gesture_probe.controller_start.lock().unwrap() = Some((reopened_request, unchanged));
    let until = Instant::now() + Duration::from_secs(10);
    let page = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("recovered plugin page required")
            };
            break page;
        }
        assert!(
            Instant::now() < until,
            "recovered plugin readback timed out"
        );
        std::thread::yield_now();
    };
    assert_eq!(page.parameters[0].normalized, 0.5);
    // Let the old callback install an edit while withholding its control
    // observation, then replace the output before the worker can ACK it.
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let applied_key = GestureKey {
        generation: reopened_context.project_generation,
        token: 2,
    };
    let applied_origin = PluginParameterOrigin {
        context: reopened_context,
        metadata_revision: page.revision,
        initial: PluginParameterValue::new(0.5).unwrap(),
        ..origin
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: applied_key,
            origin: applied_origin,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == applied_key)
        {
            assert!(ack.accepted, "applied-loss begin: {ack:?}");
            break;
        }
        assert!(Instant::now() < until, "applied-loss begin timed out");
        std::thread::yield_now();
    }
    let (ready, resume, _busy) = h.probe_next_native_publication();
    h.gesture_probe
        .hold_publication_terminal
        .store(true, Release);
    h.gesture_probe
        .saw_publication_applied
        .store(false, Release);
    h.service
        .submit(Request::DeviceGesture(GestureRequest::CommitPlugin {
            key: applied_key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(
            Instant::now() < until,
            "applied-loss commit did not reach graph gate"
        );
        std::thread::yield_now();
    }
    resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !h.gesture_probe.saw_publication_applied.load(Acquire) {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == applied_key),
                "held old callback must not be acknowledged"
            );
        }
        assert!(
            Instant::now() < until,
            "old callback did not apply the plugin edit"
        );
        std::thread::yield_now();
    }
    h.service.submit(Request::Undo).unwrap();
    let applied_old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.gesture_probe.restore_ready.store(false, Release);
    h.gesture_probe
        .restart_generation
        .store(reopened_context.project_generation + 1, Release);
    wait(|| h.gesture_probe.restart_result.lock().unwrap().is_some());
    assert!(
        h.gesture_probe
            .restart_result
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .is_ok()
    );
    let restore_finishes_before = h.gesture_probe.restore_finishes.lock().unwrap().len();
    let (recovery_ready, recovery_resume, _busy) = h.probe_next_native_publication();
    h.release_relinquished_output();
    h.wait_fresh_output(applied_old_output);
    h.gesture_probe
        .hold_publication_terminal
        .store(false, Release);
    let until = Instant::now() + Duration::from_secs(10);
    while recovery_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == applied_key),
                "applied edit must wait for replacement graph installation"
            );
            assert!(
                !event.request_complete,
                "Undo must remain queued during recovery"
            );
        }
        assert!(
            Instant::now() < until,
            "applied recovery did not reach graph gate"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        h.gesture_probe.restore_finishes.lock().unwrap().len(),
        restore_finishes_before
    );
    // Discard any old-output observation before the held replacement is
    // admitted; only a subsequent submitted event can satisfy the check.
    h.submitted = None;
    recovery_resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == applied_key),
                "admission must not acknowledge before the fresh callback"
            );
            assert!(
                !event.request_complete,
                "Undo must wait for the fresh callback"
            );
        }
        if h.submitted.is_some_and(|(_, metadata)| {
            metadata.context.project_generation == reopened_context.project_generation
                && metadata.context.editor_revision == reopened_context.editor_revision + 1
        }) {
            break;
        }
        assert!(
            Instant::now() < until,
            "applied recovery graph was not submitted"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        0,
        "the replacement callback has not yet installed the graph"
    );
    assert_eq!(
        h.gesture_probe.restore_finishes.lock().unwrap().len(),
        restore_finishes_before
    );
    let until = Instant::now() + Duration::from_secs(10);
    let mut committed_after_reopen = false;
    let recovered = loop {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            if let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == applied_key)
            {
                assert!(
                    ack.accepted && !ack.source_lost,
                    "applied-loss commit: {ack:?}"
                );
                assert_ne!(
                    event.state.project.device(device).unwrap().state(),
                    opened.state.project.device(device).unwrap().state(),
                    "accepted recovery must commit the captured plugin state"
                );
                let source = h
                    .audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .recording_source()
                    .unwrap();
                assert_eq!(
                    source.context.editor_revision,
                    reopened_context.editor_revision + 1
                );
                committed_after_reopen = true;
            } else if event.request_complete {
                assert!(committed_after_reopen, "Undo must follow recovered commit");
                assert!(event.error.is_none(), "recovered Undo: {:?}", event.error);
                assert_eq!(event.state.project, opened.state.project);
                break event.state;
            }
        }
        assert!(
            Instant::now() < until,
            "applied-before-loss commit and Undo remain stranded"
        );
        std::thread::yield_now();
    };

    // A callback refusal is different from a graph which was never observed
    // by the callback. Hold its terminal, retire that output, and require the
    // replacement callback before reporting the refused edit and releasing FIFO.
    let refused_context = AudioSourceContext {
        editor_revision: recovered.revision,
        ..reopened_context
    };
    let refused_request = PluginControllerPageRequest {
        context: refused_context,
        revision: 0,
        ..reopened_request
    };
    let state = recovered.project.device(device).unwrap().state().clone();
    *h.gesture_probe.controller_start.lock().unwrap() = Some((refused_request, state));
    let until = Instant::now() + Duration::from_secs(10);
    let page = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("refused-loss controller page required")
            };
            break page;
        }
        assert!(Instant::now() < until, "refused-loss page timed out");
        std::thread::yield_now();
    };
    assert_eq!(page.parameters[0].normalized, 0.5);
    assert!(
        !h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire),
        "fresh output must be stopped for the stopped-gesture reproducer"
    );
    let late_key = GestureKey {
        generation: refused_context.project_generation,
        token: 3,
    };
    let late_origin = PluginParameterOrigin {
        context: refused_context,
        metadata_revision: page.revision,
        initial: PluginParameterValue::new(0.5).unwrap(),
        ..origin
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: late_key,
            origin: late_origin,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event.gesture_ack.as_ref().filter(|ack| ack.key == late_key)
        {
            assert!(ack.accepted, "late stopped begin: {ack:?}");
            break;
        }
        assert!(Instant::now() < until, "late stopped begin timed out");
        std::thread::yield_now();
    }
    let (ready, resume, _busy) = h.probe_next_native_publication();
    let late_change = GestureRequest::ChangePlugin {
        key: late_key,
        sequence: 1,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    h.service
        .submit(Request::DeviceGesture(late_change))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event.gesture_ack.as_ref().filter(|ack| ack.key == late_key)
        {
            panic!(
                "stopped point ended before publication gate: {ack:?}, error={:?}",
                event.error
            );
        }
        assert!(
            Instant::now() < until,
            "stopped point did not reach publication gate: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::yield_now();
    }
    h.submitted = None;
    resume.send(()).unwrap();
    let first_revision = loop {
        h.pump();
        if let Some((revision, metadata)) = h.submitted
            && metadata.context == refused_context
        {
            break revision;
        }
        assert!(Instant::now() < until, "stopped point graph not submitted");
        std::thread::yield_now();
    };
    submit_cancel_intent(&h, late_change);
    h.service
        .submit_plugin_cancel(GestureRequest::Cancel { key: late_key })
        .unwrap();
    wait(|| {
        h.gesture_probe
            .cancelled
            .lock()
            .unwrap()
            .contains(&late_change)
    });
    let (rollback_ready, rollback_resume, _busy) = h.probe_next_native_publication();
    let until = Instant::now() + Duration::from_secs(10);
    while rollback_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == late_key),
                "late cancellation must await rollback graph"
            );
        }
        assert!(
            Instant::now() < until,
            "stopped cancellation rollback did not reach graph gate"
        );
        std::thread::yield_now();
    }
    h.submitted = None;
    rollback_resume.send(()).unwrap();
    loop {
        h.pump();
        if let Some((revision, metadata)) = h.submitted
            && metadata.context == refused_context
            && revision > first_revision
        {
            break;
        }
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == late_key),
                "late cancellation must await rollback callback"
            );
        }
        assert!(
            Instant::now() < until,
            "stopped rollback graph not submitted"
        );
        std::thread::yield_now();
    }
    let mut saw_withdrawn = false;
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event.gesture_ack.as_ref().filter(|ack| ack.key == late_key)
        {
            assert_eq!(event.state.project, recovered.project);
            match ack.phase {
                hex_session::device_gesture::GesturePhase::Change => {
                    assert!(!ack.accepted);
                    saw_withdrawn = true;
                }
                hex_session::device_gesture::GesturePhase::Cancel => {
                    assert!(saw_withdrawn && ack.accepted);
                    break;
                }
                _ => {}
            }
        }
        assert!(
            Instant::now() < until,
            "stopped cancellation did not finish"
        );
        std::thread::yield_now();
    }
    let state = recovered.project.device(device).unwrap().state().clone();
    *h.gesture_probe.controller_start.lock().unwrap() = Some((refused_request, state));
    let until = Instant::now() + Duration::from_secs(10);
    let page = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("controller page after late cancel required")
            };
            break page;
        }
        assert!(
            Instant::now() < until,
            "controller page after late cancel timed out"
        );
        std::thread::yield_now();
    };
    assert_eq!(page.parameters[0].normalized, 0.5);
    let stopped_key = GestureKey {
        generation: refused_context.project_generation,
        token: 4,
    };
    let stopped_origin = PluginParameterOrigin {
        context: refused_context,
        metadata_revision: page.revision,
        initial: PluginParameterValue::new(0.5).unwrap(),
        ..origin
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: stopped_key,
            origin: stopped_origin,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == stopped_key)
        {
            assert!(ack.accepted, "stopped begin: {ack:?}");
            break;
        }
        assert!(Instant::now() < until, "stopped begin timed out");
        std::thread::yield_now();
    }
    h.gesture_probe.contend_stopped_finish.store(true, Release);
    h.service
        .submit(Request::DeviceGesture(GestureRequest::ChangePlugin {
            key: stopped_key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !h.gesture_probe.stopped_finish_contended.load(Acquire) {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(Instant::now() < until, "stopped finish was not contended");
        std::thread::yield_now();
    }
    while let Ok(event) = h.service.try_event() {
        assert!(
            !event
                .gesture_ack
                .as_ref()
                .is_some_and(|ack| ack.key == stopped_key),
            "contended finish must not acknowledge the stopped gesture"
        );
    }
    h.audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .hold_recording_source_observation_for_test(false);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == stopped_key)
        {
            assert!(ack.accepted, "stopped change: {ack:?}");
            assert_eq!(event.state.project, recovered.project);
            break;
        }
        assert!(
            Instant::now() < until,
            "stopped plugin change did not reach its terminal"
        );
        std::thread::yield_now();
    }
    h.service
        .submit_plugin_cancel(GestureRequest::Cancel { key: stopped_key })
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == stopped_key)
        {
            assert!(ack.accepted, "stopped cancel: {ack:?}");
            assert_eq!(event.state.project, recovered.project);
            break;
        }
        assert!(
            Instant::now() < until,
            "stopped plugin cancel did not reach its terminal"
        );
        std::thread::yield_now();
    }
    // Renewal mints a new source epoch; obtain fresh metadata before the
    // next gesture and verify cancellation restored the actual controller.
    let state = recovered.project.device(device).unwrap().state().clone();
    *h.gesture_probe.controller_start.lock().unwrap() = Some((refused_request, state));
    let until = Instant::now() + Duration::from_secs(10);
    let page = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("controller page after stopped cancel required")
            };
            break page;
        }
        assert!(
            Instant::now() < until,
            "controller page after stopped cancel timed out"
        );
        std::thread::yield_now();
    };
    assert_eq!(page.parameters[0].normalized, 0.5);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let refused_key = GestureKey {
        generation: refused_context.project_generation,
        token: 5,
    };
    let refused_origin = PluginParameterOrigin {
        context: refused_context,
        metadata_revision: page.revision,
        initial: PluginParameterValue::new(0.5).unwrap(),
        ..origin
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: refused_key,
            origin: refused_origin,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == refused_key)
        {
            assert!(ack.accepted, "refused-loss begin: {ack:?}");
            break;
        }
        assert!(Instant::now() < until, "refused-loss begin timed out");
        std::thread::yield_now();
    }
    let (ready, resume, _busy) = h.probe_next_native_publication();
    h.gesture_probe
        .hold_publication_terminal
        .store(true, Release);
    let attempts = h.gesture_probe.real_preview_attempts.load(Acquire);
    h.audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .hold_recording_source_observation_for_test(true);
    h.service
        .submit(Request::DeviceGesture(GestureRequest::CommitPlugin {
            key: refused_key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while h.gesture_probe.real_preview_attempts.load(Acquire) == attempts {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == refused_key),
                "contended source observation refused the gesture: {:?}",
                event.gesture_ack
            );
        }
        assert!(
            Instant::now() < until,
            "contended source observation was not exercised"
        );
        std::thread::yield_now();
    }
    assert!(
        h.gesture_probe.real_preview_pending.load(Acquire),
        "contended source must remain pending"
    );
    h.audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .hold_recording_source_observation_for_test(false);
    let until = Instant::now() + Duration::from_secs(10);
    while ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && event
                .gesture_ack
                .as_ref()
                .is_some_and(|ack| ack.key == refused_key)
        {
            panic!(
                "refused-loss commit ended before graph gate: ack={:?}, error={:?}",
                event.gesture_ack, event.error
            );
        }
        assert!(
            Instant::now() < until,
            "refused-loss graph did not reach its gate"
        );
        std::thread::yield_now();
    }
    h.submitted = None;
    resume.send(()).unwrap();
    let refused_revision = loop {
        h.pump();
        if let Some((revision, metadata)) = h.submitted
            && metadata.context.editor_revision == refused_context.editor_revision + 1
        {
            break revision;
        }
        assert!(Instant::now() < until, "refused-loss graph not submitted");
        std::thread::yield_now();
    };
    assert!(
        h.callback
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .reject_publication_on_callback(refused_revision)
    );
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .rejected_revision()
        .is_none_or(|(revision, _)| revision != refused_revision)
    {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == refused_key)
            );
        }
        assert!(
            Instant::now() < until,
            "old callback did not refuse the edit"
        );
        std::thread::yield_now();
    }
    h.service.submit(Request::Undo).unwrap();
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.gesture_probe
        .restart_generation
        .store(refused_context.project_generation + 1, Release);
    wait(|| h.gesture_probe.restart_result.lock().unwrap().is_some());
    assert!(
        h.gesture_probe
            .restart_result
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .is_ok()
    );
    let finishes_before = h.gesture_probe.restore_finishes.lock().unwrap().len();
    let (ready, resume, _busy) = h.probe_next_native_publication();
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    h.gesture_probe
        .hold_publication_terminal
        .store(false, Release);
    let until = Instant::now() + Duration::from_secs(10);
    while ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == refused_key)
            );
            assert!(
                !event.request_complete,
                "Undo must wait for refused-edit recovery"
            );
        }
        assert!(
            Instant::now() < until,
            "refused recovery did not reach graph gate"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        h.gesture_probe.restore_finishes.lock().unwrap().len(),
        finishes_before
    );
    h.submitted = None;
    resume.send(()).unwrap();
    while !h
        .submitted
        .is_some_and(|(_, metadata)| metadata.context == refused_context)
    {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == refused_key)
            );
            assert!(
                !event.request_complete,
                "Undo must wait for recovery callback"
            );
        }
        assert!(
            Instant::now() < until,
            "refused recovery graph not submitted"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        0
    );
    let mut refused_terminal = false;
    let after_refusal = loop {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert_eq!(event.state.project, recovered.project);
            if let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == refused_key)
            {
                assert!(
                    !ack.accepted && ack.source_lost,
                    "refused-loss commit: {ack:?}"
                );
                refused_terminal = true;
            } else if event.request_complete {
                assert!(refused_terminal, "Undo must follow refused terminal");
                assert!(
                    event.error.is_none(),
                    "Undo after refusal: {:?}",
                    event.error
                );
                break event.state;
            }
        }
        assert!(
            Instant::now() < until,
            "refused output-loss recovery stranded"
        );
        std::thread::yield_now();
    };

    // Queue Play before the stopped fence's callback witness. The point must
    // follow the actual playing source if that fence is refused.
    let race_context = AudioSourceContext {
        editor_revision: after_refusal.revision,
        ..refused_context
    };
    let race_request = PluginControllerPageRequest {
        context: race_context,
        revision: 0,
        ..refused_request
    };
    let state = after_refusal
        .project
        .device(device)
        .unwrap()
        .state()
        .clone();
    *h.gesture_probe.controller_start.lock().unwrap() = Some((race_request, state));
    let until = Instant::now() + Duration::from_secs(10);
    let race_page = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("play-race controller page required")
            };
            break page;
        }
        assert!(Instant::now() < until, "play-race page timed out");
        std::thread::yield_now();
    };
    assert_eq!(race_page.parameters[0].normalized, 0.5);
    assert!(
        !h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    let race_key = GestureKey {
        generation: race_context.project_generation,
        token: 6,
    };
    let race_origin = PluginParameterOrigin {
        context: race_context,
        metadata_revision: race_page.revision,
        initial: PluginParameterValue::new(0.5).unwrap(),
        ..origin
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: race_key,
            origin: race_origin,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event.gesture_ack.as_ref().filter(|ack| ack.key == race_key)
        {
            assert!(ack.accepted, "play-race begin: {ack:?}");
            break;
        }
        assert!(Instant::now() < until, "play-race begin timed out");
        std::thread::yield_now();
    }
    let attempts = h.gesture_probe.real_preview_attempts.load(Acquire);
    let play_ticket = h
        .audio
        .as_mut()
        .unwrap()
        .control
        .command_with_ticket(PlaybackCommand::Play)
        .unwrap();
    h.service
        .submit(Request::DeviceGesture(GestureRequest::ChangePlugin {
            key: race_key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while h.gesture_probe.real_preview_attempts.load(Acquire) == attempts {
        h.pump();
        assert!(
            !h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
        assert!(
            Instant::now() < until,
            "play-race point was not admitted before callback"
        );
        std::thread::yield_now();
    }
    h.render_plugin_block();
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(status.stream_applied_command_prefix.load(Acquire) < play_ticket);
    assert!(!status.playing.load(Acquire));
    assert!(status.stream_buffering.load(Acquire));
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event.gesture_ack.as_ref().filter(|ack| ack.key == race_key)
        {
            assert!(ack.accepted, "play-race change: {ack:?}");
            assert_eq!(event.state.project, after_refusal.project);
            break;
        }
        assert!(
            Instant::now() < until,
            "playing source stranded after stopped fence refusal"
        );
        std::thread::yield_now();
    }
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    h.service
        .submit_plugin_cancel(GestureRequest::Cancel { key: race_key })
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event.gesture_ack.as_ref().filter(|ack| ack.key == race_key)
        {
            assert!(ack.accepted, "play-race cancel: {ack:?}");
            assert_eq!(event.state.project, after_refusal.project);
            break;
        }
        assert!(Instant::now() < until, "play-race cancel timed out");
        std::thread::yield_now();
    }

    // A fault before the stopped fence's callback witness must refuse this
    // exact point without leaving the gesture and FIFO pending forever. No
    // source return or SDK parameter input has occurred at this boundary.
    h.audio.as_ref().unwrap().control.stop();
    let until = Instant::now() + Duration::from_secs(10);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .playing
        .load(Acquire)
    {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(Instant::now() < until, "pre-fence stop timed out");
    }
    *h.gesture_probe.controller_start.lock().unwrap() = Some((
        race_request,
        after_refusal
            .project
            .device(device)
            .unwrap()
            .state()
            .clone(),
    ));
    let until = Instant::now() + Duration::from_secs(10);
    let fault_page = loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("pre-fence controller page required")
            };
            break page;
        }
        assert!(Instant::now() < until, "pre-fence page timed out");
    };
    assert_eq!(fault_page.parameters[0].normalized, 0.5);
    // The stopped source has already consumed this point by the time its
    // replacement graph is submitted. A callback refusal must restore the
    // original point and install that graph before reporting refusal.
    let graph_refusal_key = GestureKey {
        generation: race_context.project_generation,
        token: 7,
    };
    let graph_refusal_origin = PluginParameterOrigin {
        context: race_context,
        metadata_revision: fault_page.revision,
        initial: PluginParameterValue::new(0.5).unwrap(),
        ..origin
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: graph_refusal_key,
            origin: graph_refusal_origin,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == graph_refusal_key)
        {
            assert!(ack.accepted, "graph-refusal begin: {ack:?}");
            break;
        }
        assert!(Instant::now() < until, "graph-refusal begin timed out");
    }
    let (first_ready, first_resume, _busy) = h.probe_next_native_publication();
    h.submitted = None;
    h.service
        .submit(Request::DeviceGesture(GestureRequest::ChangePlugin {
            key: graph_refusal_key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while first_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        assert!(
            Instant::now() < until,
            "graph-refusal point did not reach publication gate"
        );
        std::thread::yield_now();
    }
    first_resume.send(()).unwrap();
    let rejected_revision = loop {
        h.pump();
        if let Some((revision, metadata)) = h.submitted
            && metadata.stopped_restore
            && metadata.context == race_context
        {
            break revision;
        }
        assert!(Instant::now() < until, "graph-refusal point not submitted");
    };
    let (rollback_ready, rollback_resume, _busy) = h.probe_next_native_publication();
    assert!(
        h.callback
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .reject_publication_on_callback(rejected_revision)
    );
    let until = Instant::now() + Duration::from_secs(10);
    while rollback_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == graph_refusal_key),
                "refusal acknowledged before restoration: {:?}",
                event.gesture_ack
            );
        }
        assert!(
            Instant::now() < until,
            "refused stopped graph did not reach restoration gate: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::yield_now();
    }
    // Refuse the rollback graph too; the restored value is not a graph ACK.
    h.submitted = None;
    rollback_resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let rejected_rollback = loop {
        h.pump();
        if let Some((revision, metadata)) = h.submitted
            && metadata.stopped_restore
            && metadata.context == race_context
        {
            assert_ne!(revision, rejected_revision);
            break revision;
        }
        assert!(Instant::now() < until, "rollback graph was not submitted");
        std::thread::yield_now();
    };
    let (retry_ready, retry_resume, _busy) = h.probe_next_native_publication();
    assert!(
        h.callback
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .reject_publication_on_callback(rejected_rollback)
    );
    let until = Instant::now() + Duration::from_secs(10);
    while retry_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == graph_refusal_key),
                "second refusal acknowledged before restoration: {:?}",
                event.gesture_ack
            );
        }
        assert!(
            Instant::now() < until,
            "twice-refused stopped graph did not retry: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::yield_now();
    }
    retry_resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == graph_refusal_key)
        {
            assert!(
                !ack.accepted,
                "rejected stopped graph must refuse the point: {ack:?}"
            );
            assert!(
                ack.reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("stopped graph publication was refused")),
                "unexpected stopped graph terminal: {ack:?}"
            );
            assert_eq!(event.state.project, after_refusal.project);
            break;
        }
        assert!(
            Instant::now() < until,
            "refused stopped graph restoration did not finish"
        );
    }
    h.service
        .submit_plugin_cancel(GestureRequest::Cancel {
            key: graph_refusal_key,
        })
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == graph_refusal_key)
        {
            assert!(ack.accepted, "restored gesture cancel: {ack:?}");
            break;
        }
        assert!(Instant::now() < until, "restored gesture cancel timed out");
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_delivered_gain(&mut h, 2.0, "refused stopped graph restored");
    h.audio.as_ref().unwrap().control.stop();
    let until = Instant::now() + Duration::from_secs(10);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .playing
        .load(Acquire)
    {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(Instant::now() < until, "restored playback stop timed out");
    }
    *h.gesture_probe.controller_start.lock().unwrap() = Some((
        race_request,
        after_refusal
            .project
            .device(device)
            .unwrap()
            .state()
            .clone(),
    ));
    let until = Instant::now() + Duration::from_secs(10);
    let fault_page = loop {
        h.pump();
        h.render_plugin_block();
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("restored controller page required")
            };
            break page;
        }
        assert!(
            Instant::now() < until,
            "restored controller page timed out: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::yield_now();
    };
    assert_eq!(
        fault_page.parameters[0].normalized, 0.5,
        "refused point changed plugin state"
    );
    // Lose the output after the SDK point has been flushed but before its
    // graph reaches the callback. A replacement must restore the canonical
    // state before retiring the exact old gesture.
    let stopped_loss_key = GestureKey {
        generation: race_context.project_generation,
        token: 8,
    };
    let stopped_loss_origin = PluginParameterOrigin {
        context: race_context,
        metadata_revision: fault_page.revision,
        initial: PluginParameterValue::new(0.5).unwrap(),
        ..origin
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: stopped_loss_key,
            origin: stopped_loss_origin,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == stopped_loss_key)
        {
            assert!(ack.accepted, "stopped output-loss begin: {ack:?}");
            break;
        }
        assert!(
            Instant::now() < until,
            "stopped output-loss begin timed out"
        );
    }
    let (loss_ready, loss_resume, _busy) = h.probe_next_native_publication();
    h.service
        .submit(Request::DeviceGesture(GestureRequest::ChangePlugin {
            key: stopped_loss_key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while loss_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == stopped_loss_key),
                "stopped loss acknowledged before graph: {:?}",
                event.gesture_ack
            );
        }
        assert!(
            Instant::now() < until,
            "stopped loss point did not reach graph gate"
        );
        std::thread::yield_now();
    }
    h.submitted = None;
    loss_resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        if h.submitted.is_some_and(|(_, metadata)| {
            metadata.stopped_restore && metadata.context == race_context
        }) {
            break;
        }
        assert!(
            Instant::now() < until,
            "stopped loss point was not submitted"
        );
    }
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.gesture_probe
        .restart_generation
        .store(race_context.project_generation + 1, Release);
    wait(|| h.gesture_probe.restart_result.lock().unwrap().is_some());
    assert!(
        h.gesture_probe
            .restart_result
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .is_ok()
    );
    let (fresh_ready, fresh_resume, _busy) = h.probe_next_native_publication();
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    let until = Instant::now() + Duration::from_secs(10);
    while fresh_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == stopped_loss_key),
                "stopped loss acknowledged before fresh graph: {:?}",
                event.gesture_ack
            );
        }
        assert!(
            Instant::now() < until,
            "stopped loss did not reach fresh canonical graph: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::yield_now();
    }
    h.submitted = None;
    fresh_resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == stopped_loss_key),
                "stopped loss acknowledged before fresh callback: {:?}",
                event.gesture_ack
            );
        }
        if h.submitted
            .is_some_and(|(_, metadata)| metadata.context == race_context)
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "fresh canonical graph was not submitted"
        );
        std::thread::yield_now();
    }
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == stopped_loss_key)
        {
            assert!(
                !ack.accepted && ack.source_lost,
                "stopped loss terminal: {ack:?}"
            );
            assert_eq!(event.state.project, after_refusal.project);
            break;
        }
        assert!(
            Instant::now() < until,
            "stopped loss recovery did not complete"
        );
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_delivered_gain(&mut h, 2.0, "stopped output loss restored");
    h.audio.as_ref().unwrap().control.stop();
    let until = Instant::now() + Duration::from_secs(10);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .playing
        .load(Acquire)
    {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(
            Instant::now() < until,
            "stopped loss playback stop timed out"
        );
    }
    *h.gesture_probe.controller_start.lock().unwrap() = Some((
        race_request,
        after_refusal
            .project
            .device(device)
            .unwrap()
            .state()
            .clone(),
    ));
    let until = Instant::now() + Duration::from_secs(10);
    let fault_page = loop {
        h.pump();
        h.render_plugin_block();
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("stopped loss restored controller page required")
            };
            break page;
        }
        assert!(
            Instant::now() < until,
            "stopped loss restored page timed out"
        );
    };
    assert_eq!(fault_page.parameters[0].normalized, 0.5);
    // Also retire an old stopped graph which actually applied while its
    // control acknowledgement was held. Its SDK change is still only a
    // preview, so the fresh output must recover the canonical baseline.
    let stopped_applied_key = GestureKey {
        generation: race_context.project_generation,
        token: 9,
    };
    let stopped_applied_origin = PluginParameterOrigin {
        context: race_context,
        metadata_revision: fault_page.revision,
        initial: PluginParameterValue::new(0.5).unwrap(),
        ..origin
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: stopped_applied_key,
            origin: stopped_applied_origin,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == stopped_applied_key)
        {
            assert!(ack.accepted, "stopped applied-loss begin: {ack:?}");
            break;
        }
        assert!(
            Instant::now() < until,
            "stopped applied-loss begin timed out"
        );
    }
    h.gesture_probe
        .hold_publication_terminal
        .store(true, Release);
    h.gesture_probe
        .saw_publication_applied
        .store(false, Release);
    h.service
        .submit(Request::DeviceGesture(GestureRequest::ChangePlugin {
            key: stopped_applied_key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !h.gesture_probe.saw_publication_applied.load(Acquire) {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == stopped_applied_key),
                "old applied stopped point acknowledged while held: {:?}",
                event.gesture_ack
            );
        }
        assert!(Instant::now() < until, "old stopped callback did not apply");
        std::thread::yield_now();
    }
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.gesture_probe
        .restart_generation
        .store(race_context.project_generation + 1, Release);
    wait(|| h.gesture_probe.restart_result.lock().unwrap().is_some());
    assert!(
        h.gesture_probe
            .restart_result
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .is_ok()
    );
    let (fresh_ready, fresh_resume, _busy) = h.probe_next_native_publication();
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    h.gesture_probe
        .hold_publication_terminal
        .store(false, Release);
    let until = Instant::now() + Duration::from_secs(10);
    while fresh_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == stopped_applied_key),
                "stopped applied-loss acknowledged before fresh graph: {:?}",
                event.gesture_ack
            );
        }
        assert!(
            Instant::now() < until,
            "stopped applied-loss did not reach fresh graph: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::yield_now();
    }
    h.submitted = None;
    fresh_resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == stopped_applied_key),
                "stopped applied-loss acknowledged before fresh callback: {:?}",
                event.gesture_ack
            );
        }
        if h.submitted
            .is_some_and(|(_, metadata)| metadata.context == race_context)
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "stopped applied-loss fresh graph not submitted"
        );
    }
    // Lose the first replacement before its callback. The original stopped
    // gesture and its original terminal proof must survive another lease.
    let replacement_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.gesture_probe
        .restart_generation
        .store(race_context.project_generation + 1, Release);
    wait(|| h.gesture_probe.restart_result.lock().unwrap().is_some());
    assert!(
        h.gesture_probe
            .restart_result
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .is_ok()
    );
    let (next_ready, next_resume, _busy) = h.probe_next_native_publication();
    h.release_relinquished_output();
    h.wait_fresh_output(replacement_output);
    let until = Instant::now() + Duration::from_secs(10);
    while next_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == stopped_applied_key),
                "repeated output loss acknowledged before replacement callback: {:?}",
                event.gesture_ack
            );
        }
        assert!(
            Instant::now() < until,
            "repeated stopped output loss did not prepare another canonical graph: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::yield_now();
    }
    h.submitted = None;
    next_resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let replacement_revision = loop {
        h.pump();
        if let Some((revision, metadata)) = h.submitted
            && metadata.context == race_context
        {
            break revision;
        }
        assert!(
            Instant::now() < until,
            "repeated stopped output loss graph not submitted"
        );
        std::thread::yield_now();
    };
    let (refusal_ready, refusal_resume, _busy) = h.probe_next_native_publication();
    assert!(
        h.callback
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .reject_publication_on_callback(replacement_revision)
    );
    let until = Instant::now() + Duration::from_secs(10);
    while refusal_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == stopped_applied_key),
                "recovery refusal acknowledged before canonical graph: {:?}",
                event.gesture_ack
            );
        }
        assert!(
            Instant::now() < until,
            "refused fresh canonical graph did not retry: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::yield_now();
    }
    refusal_resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == stopped_applied_key)
        {
            assert!(
                !ack.accepted && ack.source_lost,
                "stopped applied-loss terminal: {ack:?}"
            );
            assert_eq!(event.state.project, after_refusal.project);
            break;
        }
        assert!(
            Instant::now() < until,
            "stopped applied-loss recovery did not finish"
        );
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_delivered_gain(&mut h, 2.0, "applied stopped output loss restored");
    h.audio.as_ref().unwrap().control.stop();
    let until = Instant::now() + Duration::from_secs(10);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .playing
        .load(Acquire)
    {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(
            Instant::now() < until,
            "applied stopped loss playback stop timed out"
        );
    }
    *h.gesture_probe.controller_start.lock().unwrap() = Some((
        race_request,
        after_refusal
            .project
            .device(device)
            .unwrap()
            .state()
            .clone(),
    ));
    let until = Instant::now() + Duration::from_secs(10);
    let fault_page = loop {
        h.pump();
        h.render_plugin_block();
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("applied stopped loss restored page required")
            };
            break page;
        }
        assert!(
            Instant::now() < until,
            "applied stopped loss page timed out"
        );
    };
    assert_eq!(fault_page.parameters[0].normalized, 0.5);
    // Retire the output before the stopped fence has a callback witness or
    // any graph receipt. The fresh canonical graph must still install before
    // the old gesture can report source loss.
    let pre_submit_key = GestureKey {
        generation: race_context.project_generation,
        token: 10,
    };
    let pre_submit_origin = PluginParameterOrigin {
        context: race_context,
        metadata_revision: fault_page.revision,
        initial: PluginParameterValue::new(0.5).unwrap(),
        ..origin
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: pre_submit_key,
            origin: pre_submit_origin,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == pre_submit_key)
        {
            assert!(ack.accepted, "pre-submit loss begin: {ack:?}");
            break;
        }
        assert!(Instant::now() < until, "pre-submit loss begin timed out");
    }
    h.service
        .submit(Request::DeviceGesture(GestureRequest::ChangePlugin {
            key: pre_submit_key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        let progress = h.plugin_progress.lock().unwrap();
        if progress.stopped_stage == Some("fence")
            && progress.stopped_fence == Some(hex_audio::prepared::StoppedEditFenceState::Pending)
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "pre-submit loss fence not observed: {progress:?}"
        );
        std::thread::yield_now();
    }
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.gesture_probe
        .restart_generation
        .store(race_context.project_generation + 1, Release);
    wait(|| h.gesture_probe.restart_result.lock().unwrap().is_some());
    assert!(
        h.gesture_probe
            .restart_result
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .is_ok()
    );
    let (fresh_ready, fresh_resume, _busy) = h.probe_next_native_publication();
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    let until = Instant::now() + Duration::from_secs(10);
    while fresh_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == pre_submit_key),
                "pre-submit loss acknowledged before fresh graph: {:?}",
                event.gesture_ack
            );
        }
        assert!(
            Instant::now() < until,
            "pre-submit loss did not reach fresh graph: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::yield_now();
    }
    h.submitted = None;
    fresh_resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == pre_submit_key),
                "pre-submit loss acknowledged before fresh callback: {:?}",
                event.gesture_ack
            );
        }
        if h.submitted
            .is_some_and(|(_, metadata)| metadata.context == race_context)
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "pre-submit loss fresh graph not submitted"
        );
    }
    // Lose this first replacement before its callback as well. Its exact
    // receipt remains retained while the second fresh graph is prepared.
    let first_replacement = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.gesture_probe
        .restart_generation
        .store(race_context.project_generation + 1, Release);
    wait(|| h.gesture_probe.restart_result.lock().unwrap().is_some());
    assert!(
        h.gesture_probe
            .restart_result
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .is_ok()
    );
    let (second_ready, second_resume, _busy) = h.probe_next_native_publication();
    h.release_relinquished_output();
    h.wait_fresh_output(first_replacement);
    let until = Instant::now() + Duration::from_secs(10);
    while second_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == pre_submit_key),
                "second loss acknowledged before replacement: {:?}",
                event.gesture_ack
            );
        }
        assert!(
            Instant::now() < until,
            "second pre-submit loss did not reach fresh graph: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::yield_now();
    }
    h.submitted = None;
    second_resume.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let second_revision = loop {
        h.pump();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == pre_submit_key),
                "second loss acknowledged before callback: {:?}",
                event.gesture_ack
            );
        }
        if let Some((revision, metadata)) = h.submitted
            && metadata.context == race_context
        {
            break revision;
        }
        assert!(
            Instant::now() < until,
            "second replacement was not submitted"
        );
    };
    let (retry_ready, retry_resume, _busy) = h.probe_next_native_publication();
    assert!(
        h.callback
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .reject_publication_on_callback(second_revision)
    );
    let until = Instant::now() + Duration::from_secs(10);
    while retry_ready.try_recv().is_err() {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event() {
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == pre_submit_key),
                "unsubmitted recovery refusal acknowledged before canonical callback: {:?}",
                event.gesture_ack
            );
        }
        assert!(
            Instant::now() < until,
            "unsubmitted refused canonical graph did not retry: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::yield_now();
    }
    retry_resume.send(()).unwrap();
    if std::env::var_os("HEX_TEST_STOPPED_RECOVERY_OPEN").is_some() {
        h.submitted = None;
        let until = Instant::now() + Duration::from_secs(10);
        while !h
            .submitted
            .is_some_and(|(_, metadata)| metadata.context == race_context)
        {
            h.pump();
            assert!(
                Instant::now() < until,
                "open barrier recovery graph not submitted"
            );
            std::thread::yield_now();
        }
        let expected_project = hex_storage::load(&save_path).unwrap();
        let pending_request = GestureRequest::ChangePlugin {
            key: pre_submit_key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        };
        h.service.submit(Request::Open(save_path.clone())).unwrap();
        wait(|| {
            h.gesture_probe
                .cancelled
                .lock()
                .unwrap()
                .contains(&pending_request)
        });
        while let Ok(event) = h.service.try_event() {
            assert!(
                !event.project_replaced,
                "document opened before recovery callback"
            );
            assert!(
                !event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == pre_submit_key),
                "gesture ended before recovery callback"
            );
        }
        let until = Instant::now() + Duration::from_secs(10);
        let mut source_lost = false;
        loop {
            h.pump();
            h.render_plugin_block();
            if let Ok(event) = h.service.try_event() {
                if let Some(ack) = event
                    .gesture_ack
                    .as_ref()
                    .filter(|ack| ack.key == pre_submit_key)
                {
                    assert!(
                        !ack.accepted && ack.source_lost,
                        "stale recovery terminal: {ack:?}"
                    );
                    source_lost = true;
                }
                if event.open_complete {
                    assert!(source_lost, "open completed before recovery terminal");
                    assert!(
                        event.error.is_none() && event.project_replaced,
                        "open after recovery: {:?}",
                        event.error
                    );
                    assert_eq!(*event.state.project, expected_project);
                    break;
                }
            }
            assert!(
                Instant::now() < until,
                "open remained deferred after recovery"
            );
            std::thread::yield_now();
        }
        return;
    }
    if std::env::var_os("HEX_TEST_STOPPED_RECOVERY_SHUTDOWN").is_some() {
        h.submitted = None;
        let until = Instant::now() + Duration::from_secs(10);
        while !h
            .submitted
            .is_some_and(|(_, metadata)| metadata.context == race_context)
        {
            h.pump();
            assert!(
                Instant::now() < until,
                "shutdown recovery graph not submitted"
            );
            std::thread::yield_now();
        }
        // Leave the actual callback held, then release Session and its client.
        // A new exclusive lease is proof that the previous output retired.
        let callback = Arc::clone(&h.callback);
        drop(h);
        let until = Instant::now() + Duration::from_secs(10);
        while !callback
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(HeadlessBackend::shutdown_requested)
        {
            assert!(
                Instant::now() < until,
                "pending recovery did not request output shutdown"
            );
            std::thread::yield_now();
        }
        callback.lock().unwrap().take().unwrap().close();
        loop {
            match hex_audio::worker_fixture::open(48_000) {
                Ok((output, mut backend)) => {
                    backend.close();
                    drop(output);
                    break;
                }
                Err(error) if error == "audio output is open or still shutting down" => {}
                Err(error) => panic!("shutdown replacement output: {error}"),
            }
            assert!(
                Instant::now() < until,
                "pending recovery retained the output lease after shutdown"
            );
            std::thread::yield_now();
        }
        return;
    }
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == pre_submit_key)
        {
            assert!(
                !ack.accepted && ack.source_lost,
                "pre-submit loss terminal: {ack:?}"
            );
            assert_eq!(event.state.project, after_refusal.project);
            break;
        }
        assert!(Instant::now() < until, "pre-submit loss recovery timed out");
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_delivered_gain(&mut h, 2.0, "pre-submit stopped output loss restored");
    h.audio.as_ref().unwrap().control.stop();
    let until = Instant::now() + Duration::from_secs(10);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .playing
        .load(Acquire)
    {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        assert!(
            Instant::now() < until,
            "pre-submit loss playback stop timed out"
        );
    }
    *h.gesture_probe.controller_start.lock().unwrap() = Some((
        race_request,
        after_refusal
            .project
            .device(device)
            .unwrap()
            .state()
            .clone(),
    ));
    let until = Instant::now() + Duration::from_secs(10);
    let fault_page = loop {
        h.pump();
        h.render_plugin_block();
        if let Some(result) = h.gesture_probe.controller_result.lock().unwrap().take() {
            let PluginControllerPageResult::Page(page) = result.unwrap() else {
                panic!("pre-submit loss restored page required")
            };
            break page;
        }
        assert!(
            Instant::now() < until,
            "pre-submit loss restored page timed out"
        );
    };
    assert_eq!(fault_page.parameters[0].normalized, 0.5);
    let fault_key = GestureKey {
        generation: race_context.project_generation,
        token: 11,
    };
    let fault_origin = PluginParameterOrigin {
        context: race_context,
        metadata_revision: fault_page.revision,
        initial: PluginParameterValue::new(0.5).unwrap(),
        ..origin
    };
    h.service
        .submit(Request::DeviceGesture(GestureRequest::BeginPlugin {
            key: fault_key,
            origin: fault_origin,
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        state::workers_idle(&h);
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == fault_key)
        {
            assert!(ack.accepted, "pre-fence begin: {ack:?}");
            break;
        }
        assert!(Instant::now() < until, "pre-fence begin timed out");
    }
    let original_source = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .recording_source()
        .unwrap();
    h.submitted = None;
    h.service
        .submit(Request::DeviceGesture(GestureRequest::ChangePlugin {
            key: fault_key,
            sequence: 1,
            value: PluginParameterValue::new(0.75).unwrap(),
        }))
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        let progress = h.plugin_progress.lock().unwrap();
        if progress.stopped_stage == Some("fence")
            && progress.stopped_fence == Some(hex_audio::prepared::StoppedEditFenceState::Pending)
        {
            break;
        }
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == fault_key)
        {
            panic!(
                "pre-fence request ended before the fence: {ack:?}, error={:?}",
                event.error
            );
        }
        assert!(
            Instant::now() < until,
            "pre-fence request not observed: {progress:?}"
        );
        std::thread::yield_now();
    }
    h.audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .render_fault
        .store(true, Release);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_plugin_block();
        if let Ok(event) = h.service.try_event()
            && let Some(ack) = event
                .gesture_ack
                .as_ref()
                .filter(|ack| ack.key == fault_key)
        {
            assert!(!ack.accepted, "pre-fence fault must refuse: {ack:?}");
            assert!(
                ack.reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("audio output fault"))
            );
            assert_eq!(event.state.project, after_refusal.project);
            assert_eq!(
                h.audio
                    .as_ref()
                    .unwrap()
                    .status()
                    .unwrap()
                    .recording_source(),
                Some(original_source)
            );
            assert!(
                h.submitted.is_none(),
                "pre-fence refusal cannot publish a replacement graph"
            );
            break;
        }
        assert!(
            Instant::now() < until,
            "pre-fence refusal stranded: {:?}",
            h.plugin_progress.lock().unwrap()
        );
        std::thread::yield_now();
    }
    let until = Instant::now() + Duration::from_secs(10);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .stopped_edit_pending()
    {
        h.pump();
        h.render_plugin_block();
        assert!(
            Instant::now() < until,
            "refused pre-fence operation did not retire"
        );
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn shutdown_during_stopped_plugin_recovery_retires_output_lease() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "native_audio::tests::plugins::native_controller_page_reaches_the_desktop_worker",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env("HEX_DESKTOP_CONTROLLER_DRIVER", "1")
        .env("HEX_ORIGINAL_INSTANCE_VARIANT", "91")
        .env("HEX_TEST_STOPPED_RECOVERY_SHUTDOWN", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn opening_document_waits_for_stopped_plugin_recovery() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "native_audio::tests::plugins::native_controller_page_reaches_the_desktop_worker",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env("HEX_DESKTOP_CONTROLLER_DRIVER", "1")
        .env("HEX_ORIGINAL_INSTANCE_VARIANT", "91")
        .env("HEX_TEST_STOPPED_RECOVERY_OPEN", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

// The source and Session worker are real. The parameter response is a
// deliberately delayed control-side test double; it does not assert SDK DSP.
#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn delayed_plugin_change_restores_before_the_held_undo() {
    delayed_plugin_change_restores_before_held_request(false);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn correlated_save_retains_identity_behind_delayed_plugin_change() {
    delayed_plugin_change_restores_before_held_request(true);
}

fn delayed_plugin_change_restores_before_held_request(correlated_save: bool) {
    use hex_session::device_gesture::{
        GestureKey, GestureRequest, PluginParameterOrigin, PluginParameterValue,
    };
    let _native = NATIVE_FIXTURE.lock().unwrap();
    // The headless output has a known two-quantum bridge and no physical
    // device contribution, as in the existing insertion fixtures.
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let files = Files::new();
    let state = h.run(Request::Import(files.wav(48_000, 2048))).state;
    (applied, _) = h.applied(applied);
    let track = state.project.tracks()[0].id();
    let input = request(&state, PluginEffectDestination::Track(track));
    h.service
        .submit(Request::InsertPluginEffect(input))
        .unwrap();
    let pending_insertion = h.next();
    assert!(pending_insertion.plugin_effect_complete.is_empty());
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let Some(Created::Device(device)) = inserted.created else {
        panic!("plugin device")
    };
    h.applied(applied);
    h.gesture_probe.enabled.store(true, Release);
    let context = AudioSourceContext {
        project_generation: inserted
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        editor_revision: inserted.state.revision,
    };
    let key = GestureKey {
        generation: context.project_generation,
        token: 71,
    };
    let begin = GestureRequest::BeginPlugin {
        key,
        origin: PluginParameterOrigin {
            context,
            device,
            parameter: 0,
            page_first: 0,
            metadata_revision: 1,
            initial: PluginParameterValue::new(0.25).unwrap(),
        },
    };
    let begun = h.run(Request::DeviceGesture(begin));
    assert!(
        begun
            .gesture_ack
            .as_ref()
            .is_some_and(|ack| ack.accepted && ack.key == key)
    );
    let change = GestureRequest::ChangePlugin {
        key,
        sequence: 1,
        value: PluginParameterValue::new(0.75).unwrap(),
    };
    h.service.submit(Request::DeviceGesture(change)).unwrap();
    let pending = h.next();
    assert!(!pending.request_complete && pending.gesture_ack.is_none());
    wait(|| *h.gesture_probe.pending.lock().unwrap() == Some(change));
    let save_path = files.0.join("correlated-plugin-save.hex");
    let correlation = hex_session::service::SaveCorrelation {
        request_id: 91,
        project_generation: context.project_generation,
        revision: inserted.state.revision,
    };
    if correlated_save {
        h.service
            .submit_correlated_save(Request::SaveNew(save_path.clone()), correlation)
            .unwrap();
    } else {
        h.service.submit(Request::Undo).unwrap();
    }
    submit_cancel_intent(&h, change);
    wait(|| h.gesture_probe.cancelled.lock().unwrap().contains(&change));
    h.gesture_probe.ready.store(true, Release);
    let changed = h.next();
    assert!(
        changed.request_complete
            && changed
                .gesture_ack
                .as_ref()
                .is_some_and(|ack| ack.accepted && ack.key == key && ack.sequence == 1)
    );
    assert_eq!(changed.state.project, inserted.state.project);
    h.service
        .submit_plugin_cancel(GestureRequest::Cancel { key })
        .unwrap();
    let restored = h.next();
    assert!(
        !restored.request_complete
            && restored.gesture_protocol_ack
            && restored.gesture_ack.as_ref().is_some_and(|ack| ack.accepted
                && ack.key == key
                && ack.phase == hex_session::device_gesture::GesturePhase::Cancel)
    );
    assert_eq!(restored.state.project, inserted.state.project);
    let end = Instant::now() + Duration::from_secs(10);
    let undone = loop {
        h.pump();
        h.render();
        if let Ok(event) = h.service.try_event()
            && event.request_complete
        {
            break event;
        }
        assert!(Instant::now() < end, "held undo did not finish");
        std::thread::yield_now();
    };
    assert!(undone.error.is_none(), "{:?}", undone.error);
    if correlated_save {
        assert!(undone.save_complete);
        assert_eq!(undone.save_correlation, Some(correlation));
        assert!(save_path.exists());
        assert_eq!(undone.state.project, inserted.state.project);
    } else {
        assert!(undone.state.project.device(device).is_none());
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_waits_for_actual_endpoint_then_commits_track_return_main() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    for location in 0..3 {
        // A headless sink has no physical-device contribution. This original
        // zero-latency fixture has a known2Q copied bridge and no downstream
        // processor; file input acquisition contributes0. No hardware claim.
        let mut h = ResidentHarness::with_plugins(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        assert!(h.next().error.is_none());
        let (mut applied, _) = h.applied(0);
        let files = Files::new();
        let mut state = h.run(Request::Import(files.wav(48_000, 2048))).state;
        (applied, _) = h.applied(applied);
        let track = state.project.tracks()[0].id();
        let destination = if location == 1 {
            let added = h.run(Request::Edit(Command::AddReturnTrack {
                name: "effect".into(),
            }));
            (applied, _) = h.applied(applied);
            let Some(Created::ReturnTrack(bus)) = added.created else {
                panic!("return")
            };
            state = h
                .run(Request::Edit(Command::SetReturnSend {
                    track,
                    return_track: bus,
                    gain: 0.5,
                }))
                .state;
            (applied, _) = h.applied(applied);
            PluginEffectDestination::Return(bus)
        } else if location == 0 {
            PluginEffectDestination::Track(track)
        } else {
            PluginEffectDestination::Main
        };
        let input = request(&state, destination);
        h.service
            .submit(Request::InsertPluginEffect(input.clone()))
            .unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(pending.request_complete && pending.plugin_effect_complete.is_empty());
        assert_eq!(pending.state.project, state.project);
        // Hold the real callback. Observe an actual queued endpoint, then prove
        // it did not become canonical while its installation is still withheld.
        wait(|| h.plugin_queued.load(Acquire));
        assert!(matches!(
            h.service.try_event(),
            Err(mpsc::TryRecvError::Empty)
        ));
        let accepted = terminal(&mut h);
        assert!(accepted.error.is_none(), "{:?}", accepted.error);
        assert!(!accepted.request_complete);
        assert_eq!(accepted.plugin_effect_complete.len(), 1);
        assert!(accepted.plugin_effect_complete[0].accepted);
        assert!(Arc::ptr_eq(
            &accepted.plugin_effect_complete[0].control,
            &input.control
        ));
        assert_eq!(accepted.state.revision, state.revision + 1);
        assert!(
            matches!(accepted.created, Some(Created::Device(id)) if id.value() == state.project.next_identity())
        );
        assert_eq!(
            accepted.state.project.track(track).unwrap().clips(),
            state.project.track(track).unwrap().clips(),
            "effect insertion changed authored clips"
        );
        assert_eq!(
            accepted.state.project.assets().collect::<Vec<_>>(),
            state.project.assets().collect::<Vec<_>>(),
            "effect insertion changed original media references"
        );
        h.applied(applied);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        let completed_before_play = h.plugin_completed.load(Acquire);
        let mut output = Vec::new();
        for block in 0..10 {
            output.extend(h.render_plugin_block());
            wait(|| h.plugin_completed.load(Acquire) > block);
        }
        assert!(output[..512].iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(
            completed_before_play, 0,
            "fresh stopped effect dispatched unexpected DSP"
        );
        // The independent original SDK fixture deliberately ramps its gain by
        // native block:0.5 + block/8. This distinguishes replayed/dropped blocks
        // from a renderer that merely happens to output the first correct gain.
        let expected_at = |index: usize| {
            let scale = 0.5 + ((index - 512) / 256) as f32 * 0.125;
            if location == 1 {
                [0.25 + 0.125 * scale; 2]
            } else {
                [0.25 * scale; 2]
            }
        };
        let mismatch = output
            .iter()
            .enumerate()
            .skip(512)
            .find(|(index, frame)| **frame != expected_at(*index));
        assert!(
            mismatch.is_none(),
            "native PCM differs at location{location}: first={mismatch:?}, completed={}, fault={:?}",
            h.plugin_completed.load(Acquire),
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .render_fault_snapshot()
        );
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_restores_committed_effect_after_stop() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    for location in 0..3 {
        let mut h = ResidentHarness::with_plugins(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        assert!(h.next().error.is_none());
        let (mut applied, _) = h.applied(0);
        let files = Files::new();
        let mut state = h.run(Request::Import(files.wav(48_000, 2048))).state;
        (applied, _) = h.applied(applied);
        let track = state.project.tracks()[0].id();
        let destination = match location {
            0 => PluginEffectDestination::Track(track),
            1 => {
                let added = h.run(Request::Edit(Command::AddReturnTrack {
                    name: "effect".into(),
                }));
                (applied, _) = h.applied(applied);
                let Some(Created::ReturnTrack(bus)) = added.created else {
                    panic!("return")
                };
                state = h
                    .run(Request::Edit(Command::SetReturnSend {
                        track,
                        return_track: bus,
                        gain: 0.5,
                    }))
                    .state;
                (applied, _) = h.applied(applied);
                PluginEffectDestination::Return(bus)
            }
            _ => PluginEffectDestination::Main,
        };
        h.service
            .submit(Request::InsertPluginEffect(request(&state, destination)))
            .unwrap();
        assert!(h.next().error.is_none());
        let accepted = terminal(&mut h);
        assert!(accepted.error.is_none(), "{:?}", accepted.error);
        assert!(accepted.plugin_effect_complete[0].accepted);
        (applied, _) = h.applied(applied);
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        h.render_plugin_block();
        wait(|| h.plugin_completed.load(Acquire) == 1);
        h.audio.as_ref().unwrap().control.stop();
        h.render_plugin_block();
        h.service.submit(Request::RestoreStoppedPlayback).unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(!pending.stopped_playback_restore_complete);
        assert_eq!(pending.state.project, accepted.state.project);
        assert!(matches!(
            h.service.try_event(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .is_err(),
            "held restore fence admitted Play before returns and native ACK"
        );
        let end = Instant::now() + Duration::from_secs(10);
        let restored = loop {
            h.pump();
            h.render();
            if let Ok(event) = h.service.try_event() {
                if event.stopped_playback_restore_complete {
                    break event;
                }
            }
            assert!(Instant::now() < end, "stopped restore terminal timed out");
            std::thread::yield_now();
        };
        assert!(restored.error.is_none(), "{:?}", restored.error);
        assert!(restored.stopped_playback_restore_complete);
        assert_eq!(restored.state.project, accepted.state.project);
        assert_eq!(restored.state.revision, accepted.state.revision);
        assert_eq!(restored.state.can_undo, accepted.state.can_undo);
        let (_, metadata) = h.applied(applied);
        assert!(metadata.stopped_restore);
        // A new endpoint epoch has a fresh transport completion prefix; the
        // native child's independent DSP gain-ramp counter remains at1.
        wait(|| h.plugin_completed.load(Acquire) == 0);
        wait(|| {
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .is_ok()
        });
        for block in 0..3 {
            let output = h.render_plugin_block();
            wait(|| h.plugin_completed.load(Acquire) > block);
            if block < 2 {
                // Return-path compensation delays the dry path too, as in the
                // original insertion PCM oracle above.
                assert!(output.iter().all(|frame| *frame == [0.0; 2]));
            } else {
                // The original child keeps its gain-ramp history across Stop.
                let expected = if location == 1 {
                    0.25 + 0.125 * 0.625
                } else {
                    0.25 * 0.625
                };
                assert!(
                    output.iter().all(|frame| *frame == [expected; 2]),
                    "location{location}, first={:?}, expected={expected}",
                    output[0]
                );
            }
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_pending_restore_document_change_does_not_strand_next_output() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    let initial = h.next();
    assert!(initial.error.is_none());
    let (applied, _) = h.applied(0);
    let old_output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.service
        .submit(Request::InsertPluginEffect(request(
            &initial.state,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    h.applied(applied);
    h.audio.as_ref().unwrap().control.stop();
    h.render();
    h.service.submit(Request::RestoreStoppedPlayback).unwrap();
    let pending = h.next();
    assert!(pending.error.is_none());
    assert!(!pending.stopped_playback_restore_complete);
    // Hold the old callback's fence ACK, then replace the document on the
    // canonical worker. Closing output custody must not strand the request.
    let new = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(new.project_replaced && new.stopped_playback_restore_complete);
    assert!(!new.state.can_undo);
    h.release_relinquished_output();
    h.wait_fresh_output(old_output);
    h.applied(0);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &new.state,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let next = terminal(&mut h);
    assert!(next.error.is_none(), "{:?}", next.error);
    assert!(next.plugin_effect_complete[0].accepted);
    assert_eq!(next.state.revision, new.state.revision + 1);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_history_reuses_original_after_undo_with_and_without_stop() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    for stop in [false, true] {
        native_effect_history_case(stop, false);
    }
    native_effect_history_case(true, true);
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_history_after_observed_tail_terminal() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    native_effect_history_case(false, true);
}

fn native_effect_history_case(stop_while_undone: bool, drain_tail: bool) {
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let files = Files::new();
    let original = h.run(Request::Import(files.wav(48_000, 4096))).state;
    (applied, _) = h.applied(applied);
    let track = original.project.tracks()[0].id();
    h.service
        .submit(Request::InsertPluginEffect(request(
            &original,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.render_plugin_block();
    wait(|| h.plugin_completed.load(Acquire) == 1);
    let undone = same_source_edit(&mut h, Request::Undo);
    (applied, _) = h.applied(applied);
    assert!(undone.state.project.main_devices().is_empty());
    assert_eq!(
        undone.state.project.track(track).unwrap().clips(),
        original.project.track(track).unwrap().clips()
    );
    assert_eq!(
        undone.state.project.assets().collect::<Vec<_>>(),
        original.project.assets().collect::<Vec<_>>()
    );
    assert!(undone.state.can_redo);
    if drain_tail {
        let end = Instant::now() + Duration::from_secs(10);
        let mut blocks = 0;
        while h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .native_effect_source()
            .is_some()
        {
            let before = h.plugin_completed.load(Acquire);
            h.render_plugin_block();
            blocks += 1;
            if h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .native_effect_source()
                .is_some()
            {
                wait(|| h.plugin_completed.load(Acquire) > before);
            }
            assert!(
                Instant::now() < end,
                "actual native tail terminal timed out"
            );
        }
        assert!(
            blocks > 0,
            "control did not exercise the real outgoing tail"
        );
    }
    let processed_before_stop = h.plugin_completed.load(Acquire);
    if stop_while_undone {
        h.audio.as_ref().unwrap().control.stop();
        h.render_plugin_block();
        let mut restored = h.run(Request::RestoreStoppedPlayback);
        if !restored.stopped_playback_restore_complete {
            restored = restored_terminal(&mut h);
        }
        assert!(restored.error.is_none(), "{:?}", restored.error);
        assert!(restored.stopped_playback_restore_complete);
        (applied, _) = h.applied(applied);
    }
    let redone = same_source_edit(&mut h, Request::Redo);
    assert!(
        redone.error.is_none(),
        "stop_while_undone={stop_while_undone}: {:?}",
        redone.error
    );
    assert_eq!(
        redone.state.project.snapshot(),
        inserted.state.project.snapshot()
    );
    assert_eq!(redone.state.revision, inserted.state.revision + 2);
    assert!(!redone.state.can_redo);
    let end = Instant::now() + Duration::from_secs(10);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_revision
        .load(Acquire)
        <= applied
    {
        h.pump();
        let before = h.plugin_completed.load(Acquire);
        h.render_plugin_block();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(
            status
                .rejected_revision()
                .is_none_or(|(revision, _)| revision <= applied),
            "redo callback refusal: {:?}",
            status.rejected_revision()
        );
        if status.native_effect_source().is_some() && status.playing.load(Acquire) {
            wait(|| h.plugin_completed.load(Acquire) > before);
        }
        assert!(Instant::now() < end, "actual history handoff timed out");
    }
    h.applied(applied);
    assert_eq!(
        h.plugin_records.load(Acquire),
        1,
        "history loaded a substitute source"
    );
    if stop_while_undone {
        wait(|| h.plugin_completed.load(Acquire) == 0);
        wait(|| {
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .is_ok()
        });
        for block in 0..3 {
            let output = h.render_plugin_block();
            wait(|| h.plugin_completed.load(Acquire) > block);
            let expected = if block < 2 {
                0.0
            } else {
                0.25 * (0.5 + processed_before_stop as f32 * 0.125)
            };
            assert!(
                output.iter().all(|frame| *frame == [expected; 2]),
                "history PCM: stop={stop_while_undone}, drained={drain_tail}, old DSP blocks={processed_before_stop}, block={block}, first={:?}, expected={expected}",
                output[0]
            );
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_keeps_registered_sampler_and_file_pcm_across_stop() {
    use hex_project::BeatTime;
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let files = Files::new();
    let sampler_files = Files::new();
    h.run(Request::Import(files.wav(48_000, 4096)));
    (applied, _) = h.applied(applied);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "sampler".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("sampler track")
    };
    (applied, _) = h.applied(applied);
    h.run(Request::LoadFirstSampler {
        request_id: 1,
        project_generation: 0,
        revision: added.state.revision,
        track,
        empty_sampler: None,
        path: sampler_files.wav(48_000, 4096),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    (applied, _) = h.applied(applied);
    let placed = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
    }));
    let Some(Created::Clip(clip)) = placed.created else {
        panic!("sampler midi clip")
    };
    (applied, _) = h.applied(applied);
    let original = h
        .run(Request::Edit(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        }))
        .state;
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let dry: Vec<_> = (0..3).flat_map(|_| h.render_plugin_block()).collect();
    assert!(
        dry.iter().any(|frame| frame[0] > 0.25),
        "registered sampler did not contribute beside the original file clip"
    );
    h.audio.as_ref().unwrap().control.stop();
    h.render_plugin_block();
    assert!(
        h.run(Request::RestoreStoppedPlayback)
            .stopped_playback_restore_complete
    );
    (applied, _) = h.applied(applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &original,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    assert_eq!(accepted.state.revision, original.revision + 1);
    assert_eq!(accepted.state.project.tracks(), original.project.tracks());
    assert_eq!(
        accepted.state.project.assets().collect::<Vec<_>>(),
        original.project.assets().collect::<Vec<_>>()
    );
    (applied, _) = h.applied(applied);
    for epoch in 0..2 {
        if epoch > 0 {
            h.audio.as_ref().unwrap().control.stop();
            h.render_plugin_block();
            let mut restored = h.run(Request::RestoreStoppedPlayback);
            if !restored.stopped_playback_restore_complete {
                restored = restored_terminal(&mut h);
            }
            assert!(restored.error.is_none(), "{:?}", restored.error);
            assert_eq!(restored.state.project, accepted.state.project);
            assert_eq!(restored.state.revision, accepted.state.revision);
            (applied, _) = h.applied(applied);
        }
        wait(|| h.plugin_completed.load(Acquire) == 0);
        wait(|| {
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Play)
                .is_ok()
        });
        let mut output = Vec::new();
        for block in 0..5 {
            output.extend(h.render_plugin_block());
            wait(|| h.plugin_completed.load(Acquire) > block);
        }
        assert!(output[..512].iter().all(|frame| *frame == [0.0; 2]));
        for (index, (actual, reference)) in output[512..].iter().zip(&dry).enumerate() {
            let scale = 0.5 + (epoch * 5 + index / 256) as f32 * 0.125;
            assert_eq!(
                *actual,
                reference.map(|sample| sample * scale),
                "original mixed PCM: epoch={epoch}, frame={index}"
            );
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_live_bypass_and_removal_keep_original_clock_and_tail_pcm() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let files = Files::new();
    let original = h.run(Request::Import(files.wav(48_000, 4096))).state;
    (applied, _) = h.applied(applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &original,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let device = inserted.state.project.main_devices()[0].id();
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.render_plugin_block();
    wait(|| h.plugin_completed.load(Acquire) == 1);
    for bypassed in [true, false] {
        let changed = same_source_edit(
            &mut h,
            Request::Edit(Command::SetDeviceBypass { device, bypassed }),
        );
        assert_eq!(
            changed.state.project.device(device).unwrap().bypassed(),
            bypassed
        );
        assert_eq!(changed.state.project.tracks(), original.project.tracks());
        (applied, _) = h.applied(applied);
        let start = h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .performance_frame
            .load(Acquire);
        for block in 0..3 {
            let before = h.plugin_completed.load(Acquire);
            let output = h.render_plugin_block();
            wait(|| h.plugin_completed.load(Acquire) > before);
            for (offset, frame) in output.into_iter().enumerate() {
                let absolute = start + block * 256 + offset as u64;
                let expected = if absolute < 512 {
                    0.0
                } else if bypassed {
                    0.25
                } else {
                    0.25 * (0.5 + ((absolute - 512) / 256) as f32 * 0.125)
                };
                assert_eq!(frame, [expected; 2], "bypass={bypassed}, frame={absolute}");
            }
        }
        assert_eq!(h.plugin_records.load(Acquire), 1);
    }
    let cut = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .performance_frame
        .load(Acquire);
    let removed = same_source_edit(&mut h, Request::Edit(Command::RemoveDevice(device)));
    assert!(removed.state.project.main_devices().is_empty());
    assert_eq!(removed.state.project.tracks(), original.project.tracks());
    assert_eq!(
        removed.state.project.assets().collect::<Vec<_>>(),
        original.project.assets().collect::<Vec<_>>()
    );
    h.applied(applied);
    let start = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .performance_frame
        .load(Acquire);
    for block in 0..4 {
        let before = h.plugin_completed.load(Acquire);
        let output = h.render_plugin_block();
        if h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .native_effect_source()
            .is_some()
        {
            wait(|| h.plugin_completed.load(Acquire) > before);
        }
        for (offset, frame) in output.into_iter().enumerate() {
            let absolute = start + block * 256 + offset as u64;
            let tail = if absolute >= 512 && absolute < cut + 512 {
                0.25 * (0.5 + ((absolute - 512) / 256) as f32 * 0.125)
            } else {
                0.0
            };
            assert_eq!(
                frame,
                [0.25 + tail; 2],
                "removed Main original tail: cut={cut}, frame={absolute}"
            );
        }
    }
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .native_effect_source()
            .is_none()
    );
    assert_eq!(
        h.plugin_records.load(Acquire),
        1,
        "original source must remain available for actual Undo"
    );
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_reorder_waits_for_original_zero_input_prefix_and_undo() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let files = Files::new();
    let original = h.run(Request::Import(files.wav(48_000, 4096))).state;
    (applied, _) = h.applied(applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &original,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    assert!(h.next().error.is_none());
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let native = inserted.state.project.main_devices()[0].id();
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.render_plugin_block();
    wait(|| h.plugin_completed.load(Acquire) == 1);
    let class = hex_engine::NativeDeviceClass::Effect;
    let utility_type = hex_engine::native_device_catalog_for_class(class)
        .unwrap()
        .into_iter()
        .find(|entry| entry.type_id.as_str() == Ok("hex.utility"))
        .unwrap()
        .type_id;
    let added = same_source_edit(
        &mut h,
        Request::Edit(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state_for_class(class, utility_type).unwrap(),
        }),
    );
    let Some(Created::Device(utility)) = added.created else {
        panic!("utility")
    };
    (applied, _) = h.applied(applied);
    let moved = same_source_edit(
        &mut h,
        Request::Edit(Command::MoveMainDevice {
            device: utility,
            before: Some(native),
        }),
    );
    assert_eq!(
        moved
            .state
            .project
            .main_devices()
            .iter()
            .map(|device| device.id())
            .collect::<Vec<_>>(),
        [utility, native]
    );
    assert_eq!(moved.state.project.tracks(), original.project.tracks());
    let before = h.plugin_completed.load(Acquire);
    h.render_plugin_block();
    wait(|| h.plugin_completed.load(Acquire) > before);
    assert_eq!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        applied,
        "one endpoint cannot be current and an outgoing tail before its real zero-input prefix"
    );
    let end = Instant::now() + Duration::from_secs(10);
    while h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_revision
        .load(Acquire)
        <= applied
    {
        let before = h.plugin_completed.load(Acquire);
        h.render_plugin_block();
        wait(|| h.plugin_completed.load(Acquire) > before);
        assert!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .rejected_revision()
                .is_none(),
            "actual reorder was refused"
        );
        assert!(
            Instant::now() < end,
            "actual native reorder did not reach its zero-input boundary"
        );
    }
    (applied, _) = h.applied(applied);
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .performance_frame
            .load(Acquire)
            >= 4096
    );
    assert_eq!(h.plugin_records.load(Acquire), 1);
    let undone = same_source_edit(&mut h, Request::Undo);
    assert_eq!(
        undone
            .state
            .project
            .main_devices()
            .iter()
            .map(|device| device.id())
            .collect::<Vec<_>>(),
        [native, utility]
    );
    assert_eq!(undone.state.revision, moved.state.revision + 1);
    h.applied(applied);
    let before = h.plugin_completed.load(Acquire);
    assert_eq!(h.render_plugin_block(), [[0.0; 2]; 256]);
    wait(|| h.plugin_completed.load(Acquire) > before);
    assert_eq!(
        h.plugin_records.load(Acquire),
        1,
        "reorder/Undo replaced the original native source"
    );
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_real_work_contention_waits_then_keeps_exact_accept_or_refusal() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    for known_presentation in [false, true] {
        let (reached, facts) = mpsc::sync_channel(1);
        let (resume, resumed) = mpsc::sync_channel(1);
        let mut h = ResidentHarness::with_plugins_paused_at_facts(
            host(),
            known_presentation.then_some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
            crate::native_audio::plugins::FactsGate::new(reached, resumed),
        );
        let initial = h.next();
        assert!(initial.error.is_none());
        h.applied(0);
        let input = request(&initial.state, PluginEffectDestination::Main);
        h.service
            .submit(Request::InsertPluginEffect(input.clone()))
            .unwrap();
        facts.recv_timeout(Duration::from_secs(10)).unwrap();
        let (held, observed_hold) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        let holder = h.hold_native_work_on_control(held, released);
        observed_hold.recv_timeout(Duration::from_secs(10)).unwrap();
        let before_attempt = h.plugin_progress.lock().unwrap().attempts;
        resume.send(()).unwrap();
        wait(|| h.plugin_progress.lock().unwrap().attempts > before_attempt);
        let pending = h.next();
        release.send(()).unwrap();
        holder.join().unwrap();
        assert!(
            pending.error.is_none(),
            "actual Work contention became a terminal: {:?}",
            pending.error
        );
        assert!(pending.request_complete && pending.plugin_effect_complete.is_empty());
        assert_eq!(pending.state.project, initial.state.project);
        assert_eq!(pending.state.revision, initial.state.revision);
        let finished = terminal(&mut h);
        assert!(Arc::ptr_eq(
            &finished.plugin_effect_complete[0].control,
            &input.control
        ));
        assert_eq!(
            finished.plugin_effect_complete[0].accepted,
            known_presentation
        );
        if known_presentation {
            assert!(finished.error.is_none(), "{:?}", finished.error);
            assert_eq!(finished.state.revision, initial.state.revision + 1);
            assert_eq!(finished.state.project.main_devices().len(), 1);
        } else {
            assert!(
                finished.error.as_ref().unwrap().contains("presentation"),
                "{:?}",
                finished.error
            );
            assert_eq!(finished.state.project, initial.state.project);
            assert_eq!(finished.state.revision, initial.state.revision);
            assert!(!finished.state.can_undo);
        }
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_refuses_unknown_presentation_without_canonical_or_history_edit() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    let mut h = ResidentHarness::with_plugins(host(), None);
    let initial = h.next();
    assert!(initial.error.is_none());
    h.applied(0);
    let input = request(&initial.state, PluginEffectDestination::Main);
    h.service
        .submit(Request::InsertPluginEffect(input.clone()))
        .unwrap();
    let mut refused = h.next();
    if refused.plugin_effect_complete.is_empty() {
        refused = terminal(&mut h);
    }
    assert!(
        refused.error.as_ref().unwrap().contains("presentation"),
        "unexpected native refusal: {:?}",
        refused.error
    );
    assert_eq!(refused.state.project, initial.state.project);
    assert!(!refused.state.can_undo);
    assert!(!refused.plugin_effect_complete[0].accepted);
    assert!(Arc::ptr_eq(
        &refused.plugin_effect_complete[0].control,
        &input.control
    ));
    let end = Instant::now() + Duration::from_secs(10);
    while h.plugin_records.load(Acquire) != 0 {
        h.render();
        assert!(
            Instant::now() < end,
            "original refused source did not reach actual report recovery"
        );
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn native_effect_coordinator_cancel_or_stop_before_ack_preserves_history_and_recovers_original() {
    let _native = NATIVE_FIXTURE.lock().unwrap();
    for stop in [false, true] {
        let mut h = ResidentHarness::with_plugins(
            host(),
            Some(EffectPresentation {
                input_frames: 0,
                output_frames: 512,
            }),
        );
        let initial = h.next();
        assert!(initial.error.is_none(), "{:?}", initial.error);
        h.applied(0);
        let first = request(&initial.state, PluginEffectDestination::Main);
        h.service
            .submit(Request::InsertPluginEffect(first.clone()))
            .unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert!(pending.plugin_effect_complete.is_empty());
        wait(|| h.plugin_queued.load(Acquire));
        if stop {
            h.audio.as_ref().unwrap().control.stop();
        } else {
            assert!(first.control.cancel());
        }
        let refused = terminal(&mut h);
        assert!(!refused.request_complete);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, initial.state.project);
        assert_eq!(refused.state.revision, initial.state.revision);
        assert!(!refused.state.can_undo);
        assert_eq!(refused.plugin_effect_complete.len(), 1);
        assert!(!refused.plugin_effect_complete[0].accepted);
        assert!(Arc::ptr_eq(
            &refused.plugin_effect_complete[0].control,
            &first.control
        ));
        let end = Instant::now() + Duration::from_secs(10);
        while h.plugin_records.load(Acquire) != 0 {
            h.render();
            assert!(
                Instant::now() < end,
                "original source/endpoint did not reach report recovery"
            );
            std::thread::yield_now();
        }
        let next = request(&refused.state, PluginEffectDestination::Main);
        h.service
            .submit(Request::InsertPluginEffect(next.clone()))
            .unwrap();
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        let accepted = if pending.plugin_effect_complete.is_empty() {
            terminal(&mut h)
        } else {
            pending
        };
        assert!(accepted.error.is_none(), "{:?}", accepted.error);
        assert_eq!(accepted.state.revision, initial.state.revision + 1);
        assert!(
            matches!(accepted.created, Some(Created::Device(id)) if id.value() == initial.state.project.next_identity())
        );
        assert!(accepted.plugin_effect_complete[0].accepted);
        assert!(Arc::ptr_eq(
            &accepted.plugin_effect_complete[0].control,
            &next.control
        ));
    }
}

#[test]
#[ignore = "explicit original SDK helper receipt required"]
fn native_effect_metadata_hold_survives_profile_refusal_without_opening_a_device() {
    let (sender, _events) = mpsc::sync_channel(4);
    let mut sink = NativeAudioSink::new(sender);
    sink.plugins = crate::native_audio::plugins::Plugins::with_fixture(host(), None);
    assert!(!sink.plugins.pool_held);
    let held = sink.native_sampler_hold();
    assert_eq!(
        held.retained_bytes,
        crate::audio_policy::PLUGIN_CONTROL_DATA_BYTES
    );
    assert_eq!(
        held.construction_bytes,
        crate::audio_policy::PLUGIN_CONTROL_DATA_BYTES
    );
    sink.plugins
        .refuse_profile("fixture backend profile unavailable".into());
    sink.plugins.fresh_output();
    assert!(!sink.plugins.pool_held);
    let retained = sink.native_sampler_hold();
    assert_eq!(retained.retained_bytes, held.retained_bytes);
    assert_eq!(retained.construction_bytes, held.construction_bytes);
}
