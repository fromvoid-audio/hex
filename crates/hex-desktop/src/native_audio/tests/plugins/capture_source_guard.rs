//! Commit an unlaned effect parameter while preserving the actual source.
use super::*;
use hex_project::DeviceAutomationTarget;
use hex_session::{
    device_gesture::{GestureClient, PluginParameterOrigin, PluginParameterValue},
    service::PluginParameterQuery,
};

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn pause_before_publication_preserves_previous_graph() {
    pause_guard_case(false, true, false, false);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn pause_after_install_keeps_canonical_commit() {
    pause_guard_case(true, true, false, false);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn unlaned_pause_before_publication_preserves_previous_graph() {
    pause_guard_case(false, false, false, false);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn unlaned_pause_after_install_keeps_canonical_commit() {
    pause_guard_case(true, false, false, false);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn late_editor_value_remains_on_original_worker_after_unlaned_publication() {
    pause_guard_case(false, false, true, false);
}

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn late_editor_value_survives_a_later_track_addition() {
    pause_guard_case(false, false, true, true);
}

fn pause_guard_case(after_install: bool, with_lane: bool, late_editor: bool, add_track: bool) {
    const CHILD: &str = "HEX_DESKTOP_TEST_UNLANED_EFFECT_COMMIT";
    if std::env::var_os(CHILD).is_none() {
        let gates = Files::new();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env(
                "HEX_ORIGINAL_INSTANCE_VARIANT",
                if late_editor { "147" } else { "92" },
            )
            .env("HEX_ORIGINAL_INSTANCE_RELEASE", gates.0.join("release"))
            .env("HEX_ORIGINAL_INSTANCE_TRACE", gates.0.join("trace"))
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
    let wave = files.wav(48_000, 524_288);
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog_recovery(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        48_000,
        524_288,
        directory.join("unused-catalog.json"),
        directory.join("effect.hex-journal"),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let imported = h.run(Request::Import(wave));
    assert!(imported.error.is_none());
    let (applied, _) = h.applied(applied);
    let insert = request(&imported.state, PluginEffectDestination::Main);
    let class = insert.selection.cid;
    h.run(Request::InsertPluginEffect(insert));
    let before = terminal(&mut h);
    assert!(before.error.is_none());
    let (applied, metadata) = h.applied(applied);
    let prior = &before.state.project.main_devices()[0];
    let device = prior.id();
    let parameter = if late_editor { 7 } else { 0 };
    let target = DeviceAutomationTarget::Vst3 { parameter };
    assert!(prior.automation_target(target).is_none());
    let metadata = if with_lane {
        let lane = Request::DeviceAutomation {
            project_generation: metadata.context.project_generation,
            revision: metadata.context.editor_revision,
            device,
            target,
            vst3_class: Some(class),
            points: vec![hex_project::AutomationPoint {
                beat: hex_project::BeatTime::ZERO,
                value: hex_project::AutomationValue::new(0.25).unwrap(),
                outgoing: hex_project::AutomationInterpolation::Step,
            }],
        };
        let edited = automation::edit_with_running_callback(&mut h, lane);
        assert!(edited.error.is_none(), "{:?}", edited.error);
        h.applied(applied).1
    } else {
        metadata
    };
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
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
    assert_eq!((row.id, row.automation_class), (parameter, Some(class)));
    let origin = PluginParameterOrigin {
        context: metadata.context,
        device,
        parameter: row.id,
        page_first: row.page_first,
        metadata_revision: row.metadata_revision,
        initial: PluginParameterValue::new(row.normalized).unwrap(),
    };
    assert_eq!(origin.initial.normalized(), 0.25);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
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
            // thread. Pace these 64-frame callbacks by their SDK completion
            // and give that thread time to poll; do not wait for its outer
            // control tick before sending the next physical block.
            h.render_worker_step();
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(event.error.is_none(), "{:?}", event.error);
        let ack = event.gesture_ack.as_ref().unwrap();
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(
            ack.accepted,
            "{request:?} refused: {ack:?}; source={:?}, fault={:?}, position={}",
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
    let mut client = GestureClient::default();
    client.begin_plugin(origin).unwrap();
    send(&mut h, &mut client);
    client
        .change_plugin(PluginParameterValue::new(0.5).unwrap())
        .unwrap();
    send(&mut h, &mut client);
    assert!((slope(&h) - base * 2.0).abs() < 1.0e-6);
    client
        .finish_plugin(PluginParameterValue::new(0.5).unwrap())
        .unwrap();
    h.audio
        .as_ref()
        .unwrap()
        .control
        .set_audio_diagnostics(hex_audio::audio_diagnostics::AudioDiagnosticsFlags::ALL);
    let before_revision = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_revision
        .load(Acquire);
    let (ready, resume) = if after_install {
        h.probe_next_parameter_confirmation()
    } else {
        let (ready, resume, _) = h.probe_next_native_publication();
        (ready, resume)
    };
    let request = client.next_request().unwrap();
    client.mark_submitted();
    h.service.submit(Request::DeviceGesture(request)).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if ready.try_recv().is_ok() {
            break;
        }
        h.pump();
        h.render_worker_step();
        assert!(Instant::now() < until, "publication gate not reached");
        std::thread::yield_now();
    }
    if after_install {
        while h
            .audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire)
            == before_revision
        {
            h.render_worker_step();
            assert!(Instant::now() < until, "candidate not installed");
            std::thread::yield_now();
        }
    }
    let old_output = h.audio.as_ref().unwrap().control.output_identity();
    if late_editor {
        let release = std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap();
        let trace = std::env::var_os("HEX_ORIGINAL_INSTANCE_TRACE").unwrap();
        std::fs::write(release, b"editor").unwrap();
        loop {
            let observed = std::fs::read(&trace)
                .ok()
                .and_then(|bytes| Some(i32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?)));
            if observed == Some(25) {
                break;
            }
            h.render_worker_step();
            assert!(Instant::now() < until, "late editor event was not emitted");
            std::thread::yield_now();
        }
    } else {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Pause)
            .unwrap();
        for _ in 0..16 {
            h.render_worker_step();
            if !h
                .audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
            {
                break;
            }
        }
        assert!(
            !h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
    }
    resume.send(()).unwrap();
    let event = loop {
        h.pump();
        if let Ok(event) = h.service.try_event()
            && event.gesture_ack.is_some()
        {
            break event;
        }
        h.render_worker_step();
        assert!(Instant::now() < until, "gesture completion timed out");
        std::thread::yield_now();
    };
    let ack = event.gesture_ack.unwrap();
    if late_editor {
        assert!(ack.accepted, "{ack:?}");
        let deadline = Instant::now() + Duration::from_millis(250);
        let receipts = loop {
            let observed = *h.plugin_progress.lock().unwrap();
            if observed.editor_receipt_count >= 3 {
                break observed.editor_receipts;
            }
            assert!(
                Instant::now() < deadline,
                "late editor receipts did not arrive"
            );
            h.pump();
            h.render_worker_step();
        };
        use hex_audio::plugin_worker::ParameterEditKind;
        let [Some(begin), Some(perform), Some(end), ..] = receipts else {
            panic!("missing editor receipts");
        };
        assert_eq!(
            (begin.kind, perform.kind, end.kind),
            (
                ParameterEditKind::Begin,
                ParameterEditKind::Perform,
                ParameterEditKind::End
            )
        );
        assert_eq!(
            (begin.id, perform.id, end.id),
            (parameter, parameter, parameter)
        );
        assert_eq!(perform.normalized, 0.75);
        assert_eq!(
            (perform.gesture, end.gesture),
            (begin.sequence, begin.sequence)
        );
        assert!(begin.sequence < perform.sequence && perform.sequence < end.sequence);
        let mut serial = 1;
        let result = installed_edits::query(&mut h, device, None, &mut serial);
        assert_eq!(result.source, Some(original_source));
        let value = result
            .rows
            .unwrap()
            .into_iter()
            .find(|row| row.id == parameter)
            .unwrap()
            .normalized;
        assert_eq!(value, 0.75, "late editor value must survive publication");
        assert!(
            (slope(&h) - base * 3.0).abs() < 1.0e-6,
            "the late editor value must also reach the running processor"
        );
        if add_track {
            let previous_revision = h.submitted.unwrap().0;
            h.service
                .submit(Request::Edit(Command::AddTrack {
                    name: "later".into(),
                }))
                .unwrap();
            let until = Instant::now() + Duration::from_secs(3);
            let event = loop {
                h.pump();
                h.render_worker_step();
                if let Ok(event) = h.service.try_event()
                    && event.request_complete
                {
                    break event;
                }
                assert!(
                    Instant::now() < until,
                    "later track addition did not complete"
                );
                std::thread::yield_now();
            };
            assert!(event.error.is_none(), "{:?}", event.error);
            assert!(matches!(event.created, Some(Created::Track(_))));
            h.applied(previous_revision);
            let current = installed_edits::query(&mut h, device, None, &mut serial);
            let current_value = current
                .rows
                .unwrap()
                .into_iter()
                .find(|row| row.id == parameter)
                .unwrap()
                .normalized;
            assert_eq!(
                current_value, 0.75,
                "unrelated graph update must preserve the editor value"
            );
            assert!(
                (slope(&h) - base * 3.0).abs() < 1.0e-6,
                "the later graph must keep the editor value in the processor"
            );
            let path = directory.join("late-editor.hex");
            h.service.submit(Request::SaveNew(path.clone())).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                h.pump();
                h.render_worker_step();
                if let Ok(event) = h.service.try_event()
                    && event.request_complete
                {
                    assert!(event.error.is_none(), "{:?}", event.error);
                    assert!(event.save_complete);
                    break;
                }
                assert!(Instant::now() < deadline, "save did not complete");
                std::thread::yield_now();
            }
            let saved = hex_storage::load(&path).unwrap();
            let state = hex_plugin_state::decode(saved.device(device).unwrap().state()).unwrap();
            let component = state.component.unwrap();
            let gain = f32::from_le_bytes(component[8..12].try_into().unwrap());
            assert_eq!(gain, 3.0, "saved state must retain the late editor value");
        }
        return;
    }
    assert_eq!(
        event.state.revision,
        metadata.context.editor_revision + u64::from(after_install),
        "the session must agree with the installed graph: ack={ack:?}, applied={}, before={before_revision}, error={:?}",
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .applied_revision
            .load(Acquire),
        event.error
    );
    let output = h
        .audio
        .as_ref()
        .and_then(|audio| audio.control.output_identity());
    let mut diagnostics = Vec::new();
    while let Some(event) = h.audio.as_mut().unwrap().control.take_audio_diagnostic() {
        diagnostics.push(event);
    }
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert!(event.error.is_none(), "{:?}", event.error);
    if after_install {
        assert!(
            ack.accepted,
            "a gesture already installed before Pause must commit"
        );
        assert!(status.applied_revision.load(Acquire) > before_revision);
        assert_eq!(output, old_output);
        return;
    }
    assert!(
        !ack.accepted,
        "the changed transport must refuse the gesture"
    );
    assert_eq!(
        status.applied_revision.load(Acquire),
        before_revision,
        "the refused gesture must not install its candidate; diagnostics {diagnostics:?}"
    );
    assert_eq!(output, old_output);
    if !with_lane {
        assert!(ack.preview_retained, "{ack:?}");
        assert!(client.acknowledge_with_snapshot(&ack, event.active_device_gesture));
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        client.cancel().unwrap();
        send(&mut h, &mut client);
        let mut serial = 1;
        let deadline = Instant::now() + Duration::from_millis(250);
        let result = loop {
            let result = installed_edits::query_once(&mut h, device, None, &mut serial);
            if result.source.is_some() || Instant::now() >= deadline {
                break result;
            }
            h.pump();
            h.render_worker_step();
        };
        assert_eq!(result.source, Some(original_source), "{:?}", result.rows);
        let value = result
            .rows
            .unwrap()
            .into_iter()
            .find(|row| row.id == parameter)
            .unwrap()
            .normalized;
        assert_eq!(value, 0.25, "cancel must restore the original parameter");
    }
}
