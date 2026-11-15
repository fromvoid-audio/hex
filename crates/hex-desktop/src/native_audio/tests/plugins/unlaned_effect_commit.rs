//! Commit an unlaned effect parameter while preserving the actual source.
use super::*;
use hex_project::DeviceAutomationTarget;
use hex_session::{
    device_gesture::{GestureClient, PluginParameterOrigin, PluginParameterValue},
    service::PluginParameterQuery,
};

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn unlaned_effect_commit_keeps_the_running_source() {
    const CHILD: &str = "HEX_DESKTOP_TEST_UNLANED_EFFECT_COMMIT";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "147")
            .env(
                "HEX_ORIGINAL_INSTANCE_RELEASE",
                std::env::temp_dir().join(format!("hex-7700-editor-{}", std::process::id())),
            )
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{stdout}");
        return;
    }
    let _native = native_fixture();
    let files = Files::new();
    let directory = files.0.canonicalize().unwrap();
    // A small file exceeds this test's resident budget and exercises the
    // verified-file playback handoff without the historical large project.
    let wave = files.wav(48_000, 32_768);
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog_recovery(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        48_000,
        16_384,
        directory.join("unused-catalog.json"),
        directory.join("effect.hex-journal"),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let imported = h.run(Request::Import(wave));
    assert!(imported.error.is_none());
    assert!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_))),
        "fixture must exercise file-backed audio"
    );
    let track = imported.state.project.tracks().last().unwrap().id();
    let (applied, _) = h.applied(applied);
    let mut eq = hex_engine::eq::EqState::default();
    let node_id = hex_engine::eq::NodeId::new(800).unwrap();
    let node = hex_engine::eq::Node::new(node_id, 90.97308, 0.0, 0).unwrap();
    eq.add(node).unwrap();
    let with_eq = h.run(Request::Edit(Command::InsertDevice {
        track,
        before: None,
        state: eq.encode().unwrap(),
    }));
    assert!(
        with_eq.error.is_none(),
        "native EQ insertion: {:?}",
        with_eq.error
    );
    let Some(Created::Device(eq_device)) = with_eq.created else {
        panic!("native EQ was not created");
    };
    let (applied, metadata) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        h.render_worker_step();
    }
    let mut eq_client = GestureClient::default();
    let key = eq_client
        .reserve_plugin_editor_key(metadata.context.project_generation)
        .unwrap();
    let target = hex_session::device_gesture::GestureTarget::DeviceParameterPair {
        device: eq_device,
        first: node_id
            .parameter(hex_engine::eq::NodeParameter::Frequency)
            .unwrap(),
        second: node_id.parameter(hex_engine::eq::NodeParameter::Q).unwrap(),
    };
    let drive = |h: &mut ResidentHarness, request: Request| {
        h.service.submit(request).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            h.pump();
            h.render_worker_step();
            if let Ok(event) = h.service.try_event() {
                return event;
            }
            assert!(Instant::now() < deadline, "EQ gesture timed out");
            std::thread::yield_now();
        }
    };
    let event = drive(
        &mut h,
        Request::DeviceGesture(hex_session::device_gesture::GestureRequest::Begin { key, target }),
    );
    assert!(
        event.gesture_ack.as_ref().unwrap().accepted,
        "EQ begin: {:?}",
        event.gesture_ack
    );
    let pair = hex_session::device_gesture::ParameterPairValue {
        first: hex_session::device_gesture::ParameterValue::Float(478.10144),
        second: hex_session::device_gesture::ParameterValue::Float(0.55396205),
    };
    let event = drive(
        &mut h,
        Request::DeviceGesture(hex_session::device_gesture::GestureRequest::CommitPair {
            key,
            sequence: 2,
            manual_edit: true,
            value: pair,
        }),
    );
    assert!(
        event.gesture_ack.as_ref().unwrap().accepted,
        "EQ commit: {:?}",
        event.gesture_ack
    );
    let (applied, _) = h.applied(applied);
    let insert = request(&event.state, PluginEffectDestination::Track(track));
    let class = insert.selection.cid;
    h.run(Request::InsertPluginEffect(insert));
    let before = terminal(&mut h);
    assert!(before.error.is_none());
    let (applied, metadata) = h.applied(applied);
    let prior = before
        .state
        .project
        .track(track)
        .unwrap()
        .devices()
        .last()
        .unwrap();
    let device = prior.id();
    let target = DeviceAutomationTarget::Vst3 { parameter: 7 };
    assert!(prior.automation_target(target).is_none());
    let binding = h.plugin_progress.lock().unwrap().binding.unwrap();
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    std::fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"editor",
    )
    .unwrap();
    for _ in 0..30 {
        h.render_worker_step();
        h.pump();
        std::thread::sleep(Duration::from_millis(2));
    }
    std::fs::remove_file(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap()).unwrap();
    h.run(Request::QueryPluginParameters(PluginParameterQuery {
        context: metadata.context,
        device,
        output_identity: output,
        request_id: 1,
        page_first: None,
    }));
    let result = h.next().plugin_parameters.unwrap();
    let original_source = result.source.unwrap();
    let rows = result.rows.unwrap();
    let [row] = rows.as_slice() else {
        panic!("one SDK parameter")
    };
    assert_eq!((row.id, row.automation_class), (7, Some(class)));
    let origin = PluginParameterOrigin {
        context: metadata.context,
        device,
        parameter: row.id,
        page_first: row.page_first,
        metadata_revision: row.metadata_revision,
        initial: PluginParameterValue::new(row.normalized).unwrap(),
    };
    assert_eq!(
        origin.initial.normalized(),
        0.75,
        "the original editor input must precede the host gesture"
    );
    let slope = |h: &ResidentHarness| {
        // Variant 92 scales each successful SDK block by 0.5 + block/8.
        // Once the pipeline is settled, consecutive PCM peaks isolate the
        // processor gain from that intentionally advancing fixture clock.
        let peak = || {
            instruments::render_completed_block(h)
                .into_iter()
                .flatten()
                .map(f32::abs)
                .fold(0.0, f32::max)
        };
        for _ in 0..4 {
            peak();
        }
        let before = peak();
        let after = peak();
        assert!(
            before > 0.001,
            "the original SDK must be processing the imported clip"
        );
        after - before
    };
    let base = slope(&h);
    assert!(base > 0.001);
    let send = |h: &mut ResidentHarness, client: &mut GestureClient| {
        let request = client.next_request().unwrap();
        client.mark_submitted();
        h.service.submit(Request::DeviceGesture(request)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let event = loop {
            h.pump();
            if let Ok(event) = h.service.try_event()
                && event.gesture_protocol_ack
                && event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == request.key() && ack.phase == request.phase())
            {
                break event;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {request:?}"
            );
            // A gesture waits on the original SDK reply on the Session
            // Pace each 256-frame block against the actual worker, as in the
            // recorded device workload, without waiting on the control tick.
            h.render_plugin_block();
            while h
                .callback
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .native_workers_idle_on_control()
                != Some(true)
            {
                assert!(Instant::now() < deadline, "native worker timed out");
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(event.error.is_none(), "{:?}", event.error);
        let ack = event.gesture_ack.as_ref().unwrap();
        let mut diagnostics = Vec::new();
        if !ack.accepted {
            while let Some(event) = h.audio.as_mut().unwrap().control.take_audio_diagnostic() {
                diagnostics.push(event);
            }
        }
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(
            ack.accepted,
            "{request:?} refused: {ack:?}; source={:?}, fault={:?}, position={}, diagnostics={diagnostics:?}",
            status.native_parameter_source(true),
            status.render_fault_snapshot(),
            status.position.load(Acquire),
        );
        assert!(
            client.acknowledge_with_snapshot(ack, event.active_device_gesture),
            "acknowledgment did not match {request:?}: {ack:?}, {:?}",
            event.active_device_gesture
        );
        event
    };
    let mut client = eq_client;
    client.begin_plugin(origin).unwrap();
    send(&mut h, &mut client);
    client
        .change_plugin(PluginParameterValue::new(0.5).unwrap())
        .unwrap();
    send(&mut h, &mut client);
    assert!((slope(&h) - base * (2.0 / 3.0)).abs() < 1.0e-6);
    h.audio
        .as_ref()
        .unwrap()
        .control
        .set_audio_diagnostics(hex_audio::audio_diagnostics::AudioDiagnosticsFlags::ALL);
    client
        .finish_plugin(PluginParameterValue::new(0.5).unwrap())
        .unwrap();
    let committed = send(&mut h, &mut client);
    let (commit_applied, _) = h.applied(applied);
    assert_ne!(
        committed
            .state
            .project
            .track(track)
            .unwrap()
            .devices()
            .last()
            .unwrap()
            .state(),
        prior.state()
    );
    assert!(
        committed
            .state
            .project
            .track(track)
            .unwrap()
            .devices()
            .last()
            .unwrap()
            .automation_target(target)
            .is_none()
    );
    let mut serial = 1;
    let source = installed_edits::query(&mut h, device, None, &mut serial)
        .source
        .unwrap();
    let commit_binding = h.plugin_progress.lock().unwrap().binding;
    assert!((slope(&h) - base * (2.0 / 3.0)).abs() < 1.0e-6);
    assert_eq!(source, original_source);
    assert_eq!(commit_binding, Some(binding));
    h.service.submit(Request::Undo).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let undone = loop {
        h.pump();
        h.render_worker_step();
        if let Ok(event) = h.service.try_event()
            && event.request_complete
        {
            break event;
        }
        assert!(Instant::now() < until, "undo terminal timed out");
    };
    assert!(undone.error.is_none(), "{:?}", undone.error);
    h.applied(commit_applied);
    let undo = installed_edits::query(&mut h, device, None, &mut serial);
    let row = undo
        .rows
        .unwrap()
        .into_iter()
        .find(|row| row.id == 7)
        .unwrap();
    assert_eq!(
        row.normalized, 0.75,
        "undo must restore the editor value captured before the host gesture"
    );
}
