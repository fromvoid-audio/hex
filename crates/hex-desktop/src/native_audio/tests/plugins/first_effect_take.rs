//! Record the first lane of an actual raw-zero VST3 effect while it plays.
use super::*;
use hex_project::DeviceAutomationTarget;
use hex_session::{
    device_gesture::{GestureClient, PluginParameterOrigin, PluginParameterValue},
    service::PluginParameterQuery,
};

#[test]
#[ignore = "explicit source-bound signed SDK receipts required"]
fn records_first_raw_zero_effect_lane_without_mutating_the_base() {
    const CHILD: &str = "HEX_DESKTOP_TEST_FIRST_VST3_EFFECT_TAKE";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "92")
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
    let wave = files.wav(48_000, 8192);
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog_recovery(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        48_000,
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
    let target = DeviceAutomationTarget::Vst3 { parameter: 0 };
    assert!(prior.automation_target(target).is_none());
    let binding = h.plugin_progress.lock().unwrap().binding.unwrap();
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    h.run(Request::QueryPluginParameters(PluginParameterQuery {
        context: metadata.context,
        device,
        output_identity: output,
        request_id: 1,
        page_first: None,
    }));
    let rows = h.next().plugin_parameters.unwrap().rows.unwrap();
    let [row] = rows.as_slice() else {
        panic!("one SDK parameter")
    };
    assert_eq!((row.id, row.automation_class), (0, Some(class)));
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
    let peak = |h: &ResidentHarness| {
        (0..4)
            .map(|_| {
                instruments::render_completed_block(h)
                    .into_iter()
                    .flatten()
                    .map(f32::abs)
                    .fold(0.0, f32::max)
            })
            .last()
            .unwrap()
    };
    let base = peak(&h);
    assert!(base > 0.001);
    let send = |h: &mut ResidentHarness, client: &mut GestureClient| {
        let request = client.next_request().unwrap();
        client.mark_submitted();
        let event = automation::edit_with_running_callback(h, Request::DeviceGesture(request));
        assert!(event.error.is_none(), "{:?}", event.error);
        let ack = event.gesture_ack.as_ref().unwrap();
        assert!(ack.accepted && client.acknowledge_with_snapshot(ack, event.active_device_gesture));
        event
    };
    let mut client = GestureClient::default();
    client.begin_plugin_recording(origin).unwrap();
    assert_eq!(
        send(&mut h, &mut client).state.project,
        before.state.project
    );
    client
        .change_plugin(PluginParameterValue::new(0.5).unwrap())
        .unwrap();
    assert_eq!(
        send(&mut h, &mut client).state.project,
        before.state.project
    );
    assert!((peak(&h) - base * 2.0).abs() < 1.0e-6);
    client
        .finish_plugin(PluginParameterValue::new(0.75).unwrap())
        .unwrap();
    let take = send(&mut h, &mut client);
    let applied = h.applied(applied).0;
    assert_eq!(h.plugin_progress.lock().unwrap().binding, Some(binding));
    let device = &take.state.project.main_devices()[0];
    let lane = device.automation_target(target).unwrap();
    assert_eq!(device.state(), prior.state());
    assert_eq!(lane.vst3_class, Some(class));
    assert!(lane.is_reading() && lane.points.is_empty());
    assert_eq!(lane.touch.len(), 1);
    assert_eq!(
        lane.touch[0]
            .steps
            .iter()
            .map(|step| step.value.get())
            .collect::<Vec<_>>(),
        [0.5, 0.75]
    );
    assert!((peak(&h) - base).abs() < 1.0e-6);
    let undo = automation::edit_with_running_callback(&mut h, Request::Undo);
    assert_eq!(undo.state.project, before.state.project);
    let applied = h.applied(applied).0;
    let redo = automation::edit_with_running_callback(&mut h, Request::Redo);
    assert_eq!(redo.state.project, take.state.project);
    h.applied(applied);
    let path = directory.join("raw-zero-touch.hex");
    let saved = h.run(Request::SaveNew(path.clone()));
    assert!(
        saved.save_complete && saved.error.is_none(),
        "{:?}",
        saved.error
    );
    assert_eq!(hex_storage::load(path).unwrap(), *redo.state.project);
}
