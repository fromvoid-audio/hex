//! Signed stopped history through the catalog path selected at Desktop startup.
use super::*;
use crate::native_audio::plugins::catalog_path_from_home;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, DeviceAutomationTarget,
};

#[test]
#[ignore = "explicit original SDK helper and source fixtures required"]
fn relative_catalog_undo_prepares_a_fresh_worker_after_stopped_delete() {
    const CHILD: &str = "HEX_TEST_RELATIVE_CATALOG_AUTHORED_HISTORY_CHILD";
    const RATE: u32 = 48_000;
    const STEP_FRAME: usize = 2_176;
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                std::thread::current().name().unwrap(),
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "92")
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
    let fixture_home = files.0.canonicalize().unwrap();
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    let mut relative_home = PathBuf::new();
    for _ in cwd.ancestors().skip(1) {
        relative_home.push("..");
    }
    relative_home.push(fixture_home.strip_prefix("/").unwrap());
    let browser_catalog =
        hex_preferences::browser_places_path(&relative_home).with_file_name("plugin-catalog.json");
    let catalog = catalog_path_from_home(&relative_home).expect("resolvable relative catalog");
    assert!(catalog.is_absolute());
    let directory = browser_catalog.parent().unwrap();
    fs::create_dir_all(directory).unwrap();
    hex_ui::write_plugin_effect_test_catalog(directory, &selection(), "Fx", 0).unwrap();
    assert_eq!(
        catalog.canonicalize().unwrap(),
        browser_catalog.canonicalize().unwrap()
    );

    let mut h = ResidentHarness::with_plugins_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        catalog,
    );
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let document = h.run(Request::Import(files.wav(RATE, 4096))).state;
    let (imported, _) = h.applied(initial);
    let insertion = request(&document, PluginEffectDestination::Main);
    let class = insertion.selection.cid;
    h.service
        .submit(Request::InsertPluginEffect(insertion))
        .unwrap();
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    assert!(inserted.plugin_effect_complete[0].accepted);
    let Some(Created::Device(device)) = inserted.created else {
        panic!("signed Main effect");
    };
    let (applied, metadata) = h.applied(imported);
    // Variant 92 exposes automatable SDK parameter ID zero. Seed a real
    // nonempty lane through Session before testing restoration from history.
    let target = DeviceAutomationTarget::Vst3 { parameter: 0 };
    let points = vec![
        AutomationPoint {
            beat: BeatTime::ZERO,
            value: AutomationValue::new(0.125).unwrap(),
            outgoing: AutomationInterpolation::Step,
        },
        AutomationPoint {
            // The fixture project uses 120 BPM.
            beat: BeatTime::from_ratio(2 * STEP_FRAME as u64, u64::from(RATE)).unwrap(),
            value: AutomationValue::new(0.375).unwrap(),
            outgoing: AutomationInterpolation::Step,
        },
    ];
    let authored = automation::edit_with_running_callback(
        &mut h,
        Request::DeviceAutomation {
            project_generation: metadata.context.project_generation,
            revision: inserted.state.revision,
            device,
            target,
            vst3_class: Some(class),
            points: points.clone(),
        },
    );
    assert!(authored.error.is_none(), "{:?}", authored.error);
    let lane = authored
        .state
        .project
        .device(device)
        .unwrap()
        .automation_target(target)
        .unwrap();
    assert_eq!(lane.vst3_class, Some(class));
    assert_eq!(lane.points, points);
    let (applied, _) = h.applied(applied);
    let original_binding = h.plugin_progress.lock().unwrap().newest_source.unwrap().1;
    assert_stopped_hosted_removal_history(&mut h, &authored.state, device, applied);

    // Leave the shared protocol check's stopped Redo intact. A final Undo puts
    // a fresh worker into the authored slot for the playback check.
    let deleted_revision = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .applied_revision
        .load(Acquire);
    h.service.submit(Request::Undo).unwrap();
    let restored = state::edits::ordinary_terminal(&mut h);
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!(restored.state.project, authored.state.project);
    let receipt = restored.device_insert_complete.unwrap();
    assert_eq!(
        receipt.outcome,
        hex_session::service::DeviceInsertOutcome::Installed
    );
    assert_eq!(
        h.applied(deleted_revision).0,
        receipt.submission.audio_revision
    );
    let (restored_device, binding, installed) =
        h.plugin_progress.lock().unwrap().newest_source.unwrap();
    assert_eq!(restored_device, device.value());
    assert!(installed);
    assert_ne!(binding, original_binding);
    assert_restored_automation_pcm(&mut h, STEP_FRAME);
}

fn assert_restored_automation_pcm(h: &mut ResidentHarness, step_frame: usize) {
    const QUANTUM: usize = 256;
    let output_step = step_frame + 2 * QUANTUM;
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::PlayFrom(0))
        .unwrap();
    let status = h.audio.as_ref().unwrap().status().unwrap();
    for block in 0..=output_step / QUANTUM {
        let pcm = instruments::render_completed_block(h);
        assert_eq!(
            status.position.load(Acquire),
            ((block + 1) * QUANTUM) as u64
        );
        if block == output_step / QUANTUM {
            // SDK variant 92 multiplies input by its normalized parameter
            // and the current request's gain. Both taps are in one request.
            let offset = output_step % QUANTUM;
            let before = pcm[offset - 64];
            let after = pcm[offset + 64];
            for (before, after) in before.into_iter().zip(after) {
                assert!(before > 0.01, "silent before the restored step");
                assert!((after - 3.0 * before).abs() < 1e-6, "{before} -> {after}");
            }
        }
    }
}

#[test]
#[ignore = "explicit original SDK source fixture required"]
fn missing_catalog_configuration_refuses_ordinary_insertion_before_a_worker() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = ResidentHarness::without_plugin_catalog_configuration();
    assert!(h.next().error.is_none());
    let (initial, _) = h.applied(0);
    let document = h.run(Request::Import(files.wav(48_000, 4096))).state;
    let (applied, _) = h.applied(initial);
    let recording_source = |h: &ResidentHarness| {
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .recording_source()
            .unwrap()
    };
    let predecessor = recording_source(&h);
    assert_eq!(predecessor.audio_revision, applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &document,
            PluginEffectDestination::Main,
        )))
        .unwrap();
    let refused = terminal(&mut h);
    assert_eq!(
        refused.error.as_deref(),
        Some("vst3 catalog path unavailable · configure a valid home folder")
    );
    assert!(!refused.plugin_effect_complete[0].accepted);
    assert_eq!(refused.state.revision, document.revision);
    assert_eq!(refused.state.project, document.project);
    assert_eq!(refused.state.can_undo, document.can_undo);
    assert_eq!(refused.state.can_redo, document.can_redo);
    assert_eq!(recording_source(&h), predecessor);
    assert_eq!(h.plugin_records.load(Acquire), 0);
    assert!(h.plugin_progress.lock().unwrap().newest_source.is_none());
}
