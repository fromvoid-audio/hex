//! Saved live component/controller bytes restored into an independent instance.
use super::*;

pub(super) fn open_saved(fixture: &SavedEffectFixture) -> ResidentHarness {
    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        fixture.catalog.clone(),
    );
    assert!(h.next().error.is_none());
    h.applied(0);
    let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    play_outgoing_before_open(&mut h);
    let opened = h.run(Request::Open(fixture.path.clone()));
    assert!(opened.project_replaced && opened.open_complete);
    h.release_relinquished_output();
    h.wait_fresh_output(old);
    // Native preparation does not need a callback. Hold the simulated device
    // until its real endpoint is queued, then drive installation/publication.
    wait(|| {
        h.pump();
        assert!(
            !h.plugin_progress.lock().unwrap().restore_refused,
            "original SDK restore refused"
        );
        h.plugin_queued.load(Acquire)
    });
    h.applied(0);
    let device = fixture.project.main_devices()[0].id();
    assert_eq!(
        h.submitted_observation()
            .unwrap()
            .foreign_processor(device.value(), fixture.expected.type_id()),
        Some(true),
        "restore must install the actual native source: records={}, progress={:?}, refusals={:?}, trace={:?}",
        h.plugin_records.load(Acquire),
        h.plugin_progress.lock().unwrap(),
        h.publication_refusals,
        trace()
    );
    let restored = h.next();
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert!(!restored.request_complete);
    assert_eq!(
        restored.state.project.main_devices()[0].state(),
        &fixture.expected
    );
    h
}

fn trace() -> Vec<i32> {
    fs::read(PathBuf::from(
        std::env::var_os("HEX_ORIGINAL_RESTORE_TRACE").unwrap(),
    ))
    .unwrap()
    .chunks_exact(4)
    .map(|bytes| i32::from_ne_bytes(bytes.try_into().unwrap()))
    .collect()
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn native_effect_save_reopens_component_and_controller_into_independent_instance() {
    const NAME: &str = "native_audio::tests::plugins::state::save::reopen::native_effect_save_reopens_component_and_controller_into_independent_instance";
    if std::env::var_os("HEX_DESKTOP_SAVE_REOPEN_CHILD").is_none() {
        let files = Files::new();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                NAME,
                "--ignored",
                "--test-threads=1",
                "--nocapture",
            ])
            .env("HEX_DESKTOP_SAVE_REOPEN_CHILD", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "37")
            .env("HEX_ORIGINAL_RESTORE_TRACE", files.0.join("restore-trace"))
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(String::from_utf8_lossy(&child.stdout).contains("1 passed"));
        return;
    }
    let _native = native_fixture();
    let fixture = nondefault_fixture();
    let wave = fs::read(fixture.files.0.join("mixed-rate.wav")).unwrap();
    let mut writer = open_saved(&fixture);
    let original_output = writer
        .audio
        .as_ref()
        .unwrap()
        .control
        .output_identity()
        .unwrap();
    writer
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..6 {
        writer.render_plugin_block();
        wait(|| writer.plugin_completed.load(Acquire) > block);
    }
    let saved = writer.run(Request::SaveCurrent);
    assert!(saved.save_complete && !saved.state.unsaved);
    assert_eq!(saved.state.project.as_ref(), &fixture.project);
    assert!(!saved.state.can_undo && !saved.state.can_redo);
    assert_eq!(
        hex_storage::load(&fixture.path).unwrap().main_devices()[0].state(),
        &fixture.expected
    );
    let captured = trace();
    assert_eq!(captured.len(), 15);
    assert_eq!(
        &captured[10..13],
        &[2, 2, 1],
        "fresh getters on the original component/controller"
    );
    assert_eq!(captured[7], 91);
    drop(writer);

    let mut reader = open_saved(&fixture);
    assert_ne!(
        reader
            .audio
            .as_ref()
            .unwrap()
            .control
            .output_identity()
            .unwrap(),
        original_output
    );
    let restored = trace();
    assert_eq!(&restored[..3], &[1, 1, 1]);
    assert_eq!(
        &restored[5..8],
        &[
            i32::from_ne_bytes(0.75_f32.to_ne_bytes()),
            i32::from_ne_bytes(0.75_f32.to_ne_bytes()),
            91
        ]
    );
    reader
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..3 {
        let pcm = reader.render_plugin_block();
        wait(|| reader.plugin_completed.load(Acquire) > block);
        let expected = if block < 2 {
            [0.0; 2]
        } else {
            [0.25 * 0.75 * 0.5; 2]
        };
        assert!(
            pcm.iter().all(|frame| *frame == expected),
            "fresh instance starts its own DSP counter with saved gain"
        );
    }
    assert_eq!(
        fs::read(fixture.files.0.join("mixed-rate.wav")).unwrap(),
        wave
    );
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn live_vst3_drag_changes_original_pcm_once_and_survives_undo_save_reopen() {
    vst3_drag_roundtrip(true, false, false);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn idle_vst3_drag_survives_undo_save_reopen() {
    vst3_drag_roundtrip(false, false, false);
}

fn gesture_fixture() -> SavedEffectFixture {
    // Keep resident backing below this harness's 8192-frame source budget.
    // A long loop of the same source keeps native control work audible even
    // when headless callback driving runs much faster than wall time.
    let mut fixture = nondefault_fixture_with_frames(8192);
    let track = fixture.project.tracks()[0].id();
    let asset = fixture.project.assets().next().unwrap().id();
    let mut editor = hex_project::Editor::new(fixture.project.clone(), 8).unwrap();
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 8192,
            duration: 8192 * 256,
            source_offset: 0,
            source_length: 8192,
            looping: true,
        })
        .unwrap();
    fixture.project = editor.project().clone();
    fixture.path = fixture.files.0.join("gesture.hex");
    hex_storage::save_new(&fixture.path, &fixture.project).unwrap();
    fixture
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn stopped_vst3_drag_restores_sources_without_play_or_history() {
    vst3_drag_roundtrip(false, true, false);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn stopped_vst3_source_renewal_refuses_changed_sdk_steps_and_keeps_saveable_resources() {
    vst3_drag_roundtrip(false, true, true);
}

fn vst3_drag_roundtrip(playing: bool, stopped: bool, changed_steps: bool) {
    use hex_session::device_gesture::{
        GestureClient, GestureRequest, PluginParameterOrigin, PluginParameterValue,
    };
    use hex_session::service::PluginParameterQuery;
    const CHILD: &str = "HEX_DESKTOP_TEST_NATIVE_PARAMETER_DRAG";
    if std::env::var_os(CHILD).is_none() {
        let files = Files::new();
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env(
                "HEX_ORIGINAL_INSTANCE_VARIANT",
                if changed_steps { "141" } else { "37" },
            )
            .env(
                "HEX_ORIGINAL_INSTANCE_RELEASE",
                files.0.join("metadata-release"),
            )
            .env("HEX_ORIGINAL_RESTORE_TRACE", files.0.join("restore-trace"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let _native = native_fixture();
    // Keep producing accepted PCM while the control worker captures before
    // and after the drag. The short save-only fixture can end mid-gesture.
    let mut fixture = gesture_fixture();
    let mut h = open_saved(&fixture);
    let (before_revision, metadata) = h.submitted.unwrap();
    let device = fixture.project.main_devices()[0].id();
    let query = PluginParameterQuery {
        context: metadata.context,
        device,
        output_identity: h.audio.as_ref().unwrap().control.output_identity().unwrap(),
        request_id: 1,
        page_first: None,
    };
    h.run(Request::QueryPluginParameters(query));
    let result = h.next().plugin_parameters.unwrap().rows.unwrap();
    let gain = &result[0];
    assert!(gain.editable);
    assert_eq!(gain.normalized, 0.75 / 4.0);
    if changed_steps {
        fs::write(
            std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
            b"release",
        )
        .unwrap();
    }
    if stopped {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        for block in 0..3 {
            h.render_plugin_block();
            wait(|| h.plugin_completed.load(Acquire) > block);
        }
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Stop)
            .unwrap();
        h.render_worker_step();
    }
    if playing {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    let render = |h: &ResidentHarness| {
        if !playing {
            h.render_worker_step();
            return [[0.0; 2]; 256];
        }
        let samples = h.render_plugin_block();
        // The Session worker is waiting for this gesture's PCM point. Its
        // copied completion counter cannot advance until the request returns;
        // wait on the actual endpoint before offering the next callback.
        wait(|| {
            h.callback
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .native_workers_idle_on_control()
                == Some(true)
        });
        samples
    };
    for _ in 0..3 {
        render(&h);
    }

    let mut client = GestureClient::default();
    client
        .begin_plugin(PluginParameterOrigin {
            context: metadata.context,
            device,
            parameter: gain.id,
            page_first: gain.page_first,
            metadata_revision: gain.metadata_revision,
            initial: PluginParameterValue::new(gain.normalized).unwrap(),
        })
        .unwrap();
    let begin = client.next_request().unwrap();
    client.mark_submitted();
    let event = if stopped {
        h.service.submit(Request::DeviceGesture(begin)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            h.pump();
            if let Ok(event) = h.service.try_event() {
                break event;
            }
            assert!(Instant::now() < deadline, "stopped Begin did not complete");
            h.render_worker_step();
            std::thread::sleep(Duration::from_millis(5));
        }
    } else {
        h.run(Request::DeviceGesture(begin))
    };
    let ack = event.gesture_ack.unwrap();
    if changed_steps {
        assert!(!ack.accepted && !ack.source_lost, "{ack:?}");
        assert!(ack.reason.unwrap().contains("metadata changed"));
        assert_eq!(event.state.project.as_ref(), &fixture.project);
        assert!(!event.state.can_undo && !event.state.can_redo);
        let saved = h.run(Request::SaveCurrent);
        assert!(saved.save_complete && !saved.state.unsaved);
        assert_eq!(hex_storage::load(&fixture.path).unwrap(), fixture.project);
        h.run(Request::QueryPluginParameters(PluginParameterQuery {
            request_id: 2,
            ..query
        }));
        let current = h.next().plugin_parameters.unwrap().rows.unwrap();
        assert_eq!(current[0].steps, 3);
        return;
    }
    assert!(ack.accepted, "{ack:?}");
    assert!(client.acknowledge(&ack));
    assert_eq!(event.state.project.as_ref(), &fixture.project);
    assert!(!event.state.can_undo && !event.state.can_redo);

    let drive = |h: &mut ResidentHarness,
                 request: GestureRequest,
                 render_live: &mut dyn FnMut(&ResidentHarness)| {
        h.service.submit(Request::DeviceGesture(request)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            h.pump();
            if let Ok(event) = h.service.try_event()
                && event
                    .gesture_ack
                    .as_ref()
                    .is_some_and(|ack| ack.key == request.key() && ack.phase == request.phase())
            {
                return event;
            }
            assert!(Instant::now() < deadline, "gesture {request:?} timed out");
            std::thread::sleep(Duration::from_millis(5));
            render_live(h);
        }
    };
    if stopped {
        client
            .change_plugin(PluginParameterValue::new(0.9).unwrap())
            .unwrap();
        let change = client.next_request().unwrap();
        client.mark_submitted();
        let changed = drive(&mut h, change, &mut ResidentHarness::render_worker_step);
        let ack = changed.gesture_ack.unwrap();
        assert!(ack.accepted, "{ack:?}");
        assert!(client.acknowledge(&ack));
        client.cancel().unwrap();
        let cancel = client.next_request().unwrap();
        client.mark_submitted();
        let cancelled = drive(&mut h, cancel, &mut ResidentHarness::render_worker_step);
        let ack = cancelled.gesture_ack.unwrap();
        assert!(ack.accepted, "{ack:?}");
        assert!(client.acknowledge(&ack));
        assert_eq!(cancelled.state.project.as_ref(), &fixture.project);
        assert!(!cancelled.state.can_undo && !cancelled.state.can_redo);
        client
            .begin_plugin(PluginParameterOrigin {
                context: metadata.context,
                device,
                parameter: gain.id,
                page_first: gain.page_first,
                metadata_revision: gain.metadata_revision,
                initial: PluginParameterValue::new(gain.normalized).unwrap(),
            })
            .unwrap();
        let begin = client.next_request().unwrap();
        client.mark_submitted();
        let begun = drive(&mut h, begin, &mut ResidentHarness::render_worker_step);
        let ack = begun.gesture_ack.unwrap();
        assert!(ack.accepted, "{ack:?}");
        assert!(client.acknowledge(&ack));
    }
    let completed_before_preview = h.plugin_completed.load(Acquire);
    client
        .change_plugin(PluginParameterValue::new(0.5).unwrap())
        .unwrap();
    let change = client.next_request().unwrap();
    client.mark_submitted();
    let preview = drive(&mut h, change, &mut |h| {
        render(h);
    });
    let ack = preview.gesture_ack.as_ref().unwrap();
    assert!(ack.accepted, "{ack:?}");
    assert!(client.acknowledge(ack));
    assert_eq!(preview.state.project.as_ref(), &fixture.project);
    // Render the original pipeline's accepted point through to its PCM.
    if playing {
        let pcm = (0..3).map(|_| render(&h)).last().unwrap();
        assert!(pcm.iter().any(|frame| frame[0] > 0.25), "live preview PCM");
    } else {
        assert_eq!(h.plugin_completed.load(Acquire), completed_before_preview);
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
    client
        .finish_plugin(PluginParameterValue::new(0.5).unwrap())
        .unwrap();
    let request = client.next_request().unwrap();
    client.mark_submitted();
    let committed = drive(&mut h, request, &mut ResidentHarness::render_worker_step);
    let ack = committed.gesture_ack.as_ref().unwrap();
    assert!(ack.accepted, "{ack:?}");
    assert!(client.acknowledge(ack));
    let (applied, _) = h.applied(before_revision);
    let state = committed
        .state
        .project
        .device(device)
        .unwrap()
        .state()
        .clone();
    assert_eq!(captured_gain(&state), 2.0);
    let undo_or_redo = |h: &mut ResidentHarness, request| {
        h.service.submit(request).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            h.pump();
            if let Ok(event) = h.service.try_event()
                && event.request_complete
            {
                assert!(event.error.is_none(), "{:?}", event.error);
                break event;
            }
            assert!(Instant::now() < deadline, "history did not complete");
            h.render_worker_step();
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let undone = undo_or_redo(&mut h, Request::Undo);
    assert_eq!(
        captured_gain(undone.state.project.device(device).unwrap().state()),
        0.75
    );
    assert!(!undone.state.can_undo);
    let (applied, _) = h.applied(applied);
    let redone = undo_or_redo(&mut h, Request::Redo);
    assert_eq!(redone.state.project.device(device).unwrap().state(), &state);
    h.applied(applied);
    let saved = h.run(Request::SaveCurrent);
    assert!(saved.save_complete && !saved.state.unsaved);
    fixture.project = hex_storage::load(&fixture.path).unwrap();
    fixture.expected = fixture.project.device(device).unwrap().state().clone();
    assert_eq!(captured_gain(&fixture.expected), 2.0);
    drop(h);
    let mut reopened = open_saved(&fixture);
    reopened
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..3 {
        let pcm = reopened.render_plugin_block();
        wait(|| reopened.plugin_completed.load(Acquire) > block);
        if block == 2 {
            assert!(pcm.iter().all(|frame| *frame == [0.25; 2]));
        }
    }
}

fn captured_gain(state: &NativeDeviceState) -> f32 {
    let state = hex_plugin_state::decode(state).unwrap();
    f32::from_le_bytes(state.component.unwrap()[8..12].try_into().unwrap())
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn live_reading_vst3_drag_cancels_or_commits_exact_lane_and_preserves_other_read() {
    authored_drag(true, false, GestureTransport::Running);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn live_unautomated_vst3_drag_preserves_the_other_lanes_canonical_base() {
    authored_drag(false, false, GestureTransport::Running);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn live_authored_vst3_save_preserves_fresh_opaque_editor_state() {
    authored_drag(false, true, GestureTransport::Running);
}

#[derive(Clone, Copy)]
enum GestureTransport {
    Running,
    Idle,
    IdleThenPlay,
    Paused,
    Stopped,
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn idle_reading_vst3_drag_restores_or_commits_without_pcm() {
    authored_drag(true, false, GestureTransport::Idle);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn stopped_reading_vst3_drag_keeps_the_open_parameter_list() {
    authored_drag(true, false, GestureTransport::Stopped);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn paused_reading_vst3_drag_preserves_other_base_and_original_cancel_value() {
    authored_drag(true, false, GestureTransport::Paused);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn stopped_unautomated_vst3_drag_preserves_the_other_reading_lane() {
    authored_drag(false, false, GestureTransport::Stopped);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn idle_reading_vst3_gesture_yields_to_authored_playback_without_stale_rollback() {
    authored_drag(true, false, GestureTransport::IdleThenPlay);
}

fn authored_drag(reading: bool, editor_changed: bool, transport: GestureTransport) {
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, DeviceAutomationTarget,
    };
    use hex_session::device_gesture::{GestureClient, PluginParameterOrigin, PluginParameterValue};
    use hex_session::service::PluginParameterQuery;

    const CHILD: &str = "HEX_DESKTOP_TEST_READING_VST3_DRAG";
    if std::env::var_os(CHILD).is_none() {
        let files = Files::new();
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "144")
            .env(
                "HEX_ORIGINAL_CAPTURE_SCROLL_WHILE_PLAYING",
                if editor_changed { "1" } else { "0" },
            )
            .env("HEX_ORIGINAL_RESTORE_TRACE", files.0.join("restore-trace"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
        return;
    }

    let _native = native_fixture();
    let mut fixture = gesture_fixture();
    let device = fixture.project.main_devices()[0].id();
    let gain_target = DeviceAutomationTarget::Vst3 { parameter: 7 };
    let other_target = DeviceAutomationTarget::Vst3 { parameter: 11 };
    let mut editor = hex_project::Editor::new(fixture.project.clone(), 8).unwrap();
    for (target, value) in [(gain_target, 0.25), (other_target, 0.5)] {
        if target == gain_target && !reading {
            continue;
        }
        editor
            .execute(Command::ReplaceDeviceAutomationTarget {
                device,
                target,
                points: vec![AutomationPoint {
                    beat: BeatTime::ZERO,
                    value: AutomationValue::new(value).unwrap(),
                    outgoing: AutomationInterpolation::Step,
                }],
            })
            .unwrap();
    }
    fixture.project = editor.project().clone();
    fixture.path = fixture.files.0.join("reading.hex");
    hex_storage::save_new(&fixture.path, &fixture.project).unwrap();
    let before = fixture.project.clone();

    let mut h = open_saved(&fixture);
    let (original_revision, metadata) = h.submitted.unwrap();
    let query = |h: &mut ResidentHarness, request_id| {
        let query = PluginParameterQuery {
            context: metadata.context,
            device,
            output_identity: h.audio.as_ref().unwrap().control.output_identity().unwrap(),
            request_id,
            page_first: None,
        };
        h.run(Request::QueryPluginParameters(query));
        let rows = h.next().plugin_parameters.unwrap().rows.unwrap();
        let gain = rows.into_iter().find(|row| row.id == 7).unwrap();
        assert!(gain.editable && gain.automation_class.is_some());
        PluginParameterOrigin {
            context: metadata.context,
            device,
            parameter: gain.id,
            page_first: gain.page_first,
            metadata_revision: gain.metadata_revision,
            initial: PluginParameterValue::new(gain.normalized).unwrap(),
        }
    };
    let before_play_origin = matches!(
        transport,
        GestureTransport::Stopped | GestureTransport::Paused
    )
    .then(|| query(&mut h, 0));
    if !matches!(
        transport,
        GestureTransport::Idle | GestureTransport::IdleThenPlay
    ) {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    let render_steady = |h: &ResidentHarness, phase: &str| {
        let mut block = [[0.0; 2]; 256];
        for _ in 0..3 {
            block = h.render_plugin_block();
            let status = h.audio.as_ref().unwrap().status().unwrap();
            assert!(
                status.playing.load(Acquire),
                "{phase}: transport did not play, position={}, rejected={:?}",
                status.position.load(Acquire),
                status.rejected_action()
            );
            wait(|| {
                h.callback
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .native_workers_idle_on_control()
                    == Some(true)
            });
        }
        let status = h.audio.as_ref().unwrap().status().unwrap();
        assert!(
            block.iter().all(|&frame| frame == block[0]) && block[0][0] > 0.0,
            "{phase}: first={:?}, last={:?}, completed={}, fault={:?}, source={:?}",
            block[0],
            block[255],
            h.plugin_completed.load(Acquire),
            status.render_fault_snapshot(),
            status.native_effect_source(),
        );
        block[0][0]
    };
    let original_pcm = if matches!(
        transport,
        GestureTransport::Idle | GestureTransport::IdleThenPlay
    ) {
        0.5
    } else {
        render_steady(&h, "original reading")
    };
    assert_eq!(original_pcm, if reading { 0.5 } else { 0.375 });
    if reading && matches!(transport, GestureTransport::Running) {
        // Both parameters are actively authored here, before a manual edit.
        let saved = h.run(Request::SaveCurrent);
        assert!(saved.save_complete && saved.error.is_none());
        assert_eq!(hex_storage::load(&fixture.path).unwrap(), before);
        assert_eq!(render_steady(&h, "after two-target save"), original_pcm);
    }
    let first = before_play_origin.unwrap_or_else(|| query(&mut h, 1));
    if matches!(
        transport,
        GestureTransport::Stopped | GestureTransport::Paused
    ) {
        if matches!(transport, GestureTransport::Paused) {
            h.audio
                .as_mut()
                .unwrap()
                .control
                .command(PlaybackCommand::Pause)
                .unwrap();
        } else {
            h.audio.as_ref().unwrap().control.stop();
        }
        h.render_plugin_block();
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
    let send = |h: &mut ResidentHarness, client: &mut GestureClient| {
        let request = client.next_request().unwrap();
        client.mark_submitted();
        h.service.submit(Request::DeviceGesture(request)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            h.pump();
            if let Ok(event) = h.service.try_event()
                && let Some(ack) = event.gesture_ack.as_ref()
                && ack.key == request.key()
                && ack.phase == request.phase()
            {
                assert!(ack.accepted, "{ack:?} {:?}", event.error);
                assert!(client.acknowledge(ack));
                return event;
            }
            assert!(
                Instant::now() < deadline,
                "reading vst3 gesture {request:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
            h.render_worker_step();
        }
    };
    let mut client = GestureClient::default();
    let manual = PluginParameterValue::new(0.5).unwrap();
    client.begin_plugin(first).unwrap();
    let begun = send(&mut h, &mut client);
    assert_eq!(begun.state.project.as_ref(), &before);
    assert!(!begun.state.can_undo && !begun.state.can_redo);
    let completed_before = h.plugin_completed.load(Acquire);
    client.change_plugin(manual).unwrap();
    let preview = send(&mut h, &mut client);
    assert_eq!(preview.state.project.as_ref(), &before);
    if matches!(transport, GestureTransport::Running) {
        assert_eq!(
            render_steady(&h, "first held preview"),
            if reading { original_pcm * 2.0 } else { 1.0 }
        );
    } else {
        assert_eq!(
            h.plugin_completed.load(Acquire),
            completed_before,
            "idle preview cannot advance PCM"
        );
    }
    if matches!(transport, GestureTransport::IdleThenPlay) {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
        assert_eq!(
            render_steady(&h, "play supersedes idle preview"),
            original_pcm
        );
        client
            .change_plugin(PluginParameterValue::new(0.75).unwrap())
            .unwrap();
        let change = client.next_request().unwrap();
        client.mark_submitted();
        let refused = h.run(Request::DeviceGesture(change));
        let ack = refused.gesture_ack.unwrap();
        assert!(
            !ack.accepted && ack.source_lost && !ack.preview_retained,
            "{ack:?}"
        );
        assert!(client.acknowledge(&ack));
        assert_eq!(refused.state.project.as_ref(), &before);
        assert!(!refused.state.can_undo && !refused.state.can_redo);
        assert_eq!(
            render_steady(&h, "after stale idle input refused"),
            original_pcm
        );
        let saved = h.run(Request::SaveCurrent);
        assert!(saved.save_complete && !saved.state.unsaved);
        assert_eq!(hex_storage::load(&fixture.path).unwrap(), before);
        return;
    }
    client.cancel().unwrap();
    assert_eq!(send(&mut h, &mut client).state.project.as_ref(), &before);
    if matches!(transport, GestureTransport::Running) {
        assert_eq!(render_steady(&h, "after cancel"), original_pcm);
    } else {
        assert_eq!(
            h.plugin_completed.load(Acquire),
            completed_before,
            "idle cancel cannot advance PCM"
        );
    }

    let second = query(&mut h, 2);
    if !matches!(transport, GestureTransport::Running) {
        assert_eq!(
            second.initial,
            if reading
                && matches!(
                    transport,
                    GestureTransport::Stopped | GestureTransport::Paused
                )
            {
                PluginParameterValue::new(0.25).unwrap()
            } else {
                first.initial
            },
            "idle Cancel must restore the actual Begin value, even when the list predates playback"
        );
    }
    client.begin_plugin(second).unwrap();
    send(&mut h, &mut client);
    client.change_plugin(manual).unwrap();
    send(&mut h, &mut client);
    if matches!(transport, GestureTransport::Running) {
        assert_eq!(
            render_steady(&h, "second held preview"),
            if reading { original_pcm * 2.0 } else { 1.0 }
        );
    } else {
        assert_eq!(h.plugin_completed.load(Acquire), completed_before);
    }
    client.finish_plugin(manual).unwrap();
    let committed = send(&mut h, &mut client);
    let (revision, _) = h.applied(original_revision);
    let result = committed.state.project.device(device).unwrap();
    let original = before.device(device).unwrap();
    if reading {
        let gain = result.automation_target(gain_target).unwrap();
        assert!(!gain.is_reading());
        assert_eq!(
            gain.points,
            original.automation_target(gain_target).unwrap().points
        );
    } else {
        assert!(result.automation_target(gain_target).is_none());
    }
    assert_eq!(
        result.automation_target(other_target),
        original.automation_target(other_target)
    );
    assert_eq!(captured_gain(result.state()), 2.0);
    let decoded = hex_plugin_state::decode(result.state()).unwrap();
    assert_eq!(
        f64::from_le_bytes(decoded.component.unwrap()[32..40].try_into().unwrap()),
        1.0,
        "other reading lane must retain its canonical base"
    );
    if !matches!(transport, GestureTransport::Running) {
        assert!(
            !h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .playing
                .load(Acquire)
        );
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::Play)
            .unwrap();
    }
    assert_eq!(
        render_steady(&h, "after commit"),
        if reading { original_pcm * 2.0 } else { 1.0 }
    );

    // Save after the committed worker has really processed both parameters,
    // before history can recreate it from the canonical base.
    let active_save = h.run(Request::SaveCurrent);
    assert!(
        active_save.save_complete && active_save.error.is_none(),
        "{:?}",
        active_save.error
    );
    let expected_active = if editor_changed {
        let mut snapshot = committed.state.project.snapshot();
        let state = &snapshot.main_devices[0].state;
        let decoded = hex_plugin_state::decode(state).unwrap();
        let mut controller = decoded.controller.unwrap().to_vec();
        controller[4..8].copy_from_slice(&123_u32.to_le_bytes());
        snapshot.main_devices[0].state = hex_plugin_state::encode(hex_plugin_state::State {
            identity: decoded.identity,
            component: decoded.component,
            controller: Some(&controller),
        })
        .unwrap()
        .with_host_metadata_from(state)
        .unwrap();
        hex_project::Project::from_snapshot(snapshot).unwrap()
    } else {
        committed.state.project.as_ref().clone()
    };
    assert_eq!(
        hex_storage::load(&fixture.path).unwrap(),
        expected_active,
        "active authored values must not replace saved manual bases"
    );

    let history = |h: &mut ResidentHarness, request| {
        h.service.submit(request).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            h.pump();
            if let Ok(event) = h.service.try_event()
                && event.request_complete
            {
                assert!(event.error.is_none(), "{:?}", event.error);
                return event;
            }
            assert!(Instant::now() < deadline, "reading vst3 history");
            h.render_worker_step();
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let undone = history(&mut h, Request::Undo);
    assert_eq!(undone.state.project.as_ref(), &before);
    assert!(!undone.state.can_undo);
    let (revision, _) = h.applied(revision);
    let redone = history(&mut h, Request::Redo);
    assert_eq!(redone.state.project, committed.state.project);
    h.applied(revision);
    let saved = h.run(Request::SaveCurrent);
    assert!(saved.save_complete && !saved.state.unsaved);
    assert_eq!(
        hex_storage::load(&fixture.path).unwrap(),
        *committed.state.project
    );
    drop(h);
    fixture.project = expected_active;
    fixture.expected = fixture.project.device(device).unwrap().state().clone();
    fixture.path = fixture.files.0.join("active-save-reopen.hex");
    hex_storage::save_new(&fixture.path, &fixture.project).unwrap();
    let mut reopened = open_saved(&fixture);
    let (revision, _) = reopened.submitted.unwrap();
    reopened
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    assert_eq!(render_steady(&reopened, "reopened authored save"), 1.0);
    let base = history(
        &mut reopened,
        Request::Edit(Command::SetDeviceAutomationReadSelection {
            device,
            target: other_target,
            selection: hex_project::AutomationReadSelection::Base,
        }),
    );
    reopened.applied(revision);
    assert!(
        !base
            .state
            .project
            .device(device)
            .unwrap()
            .automation_target(other_target)
            .unwrap()
            .is_reading()
    );
    assert_eq!(render_steady(&reopened, "reopened canonical base"), 0.5);
}
