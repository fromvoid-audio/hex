//! Staged unavailable VST3 placeholders keep authored lanes during open.
use super::*;
use hex_project::{
    AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, DeviceAutomationTarget,
};

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn saved_authored_effect_opens_with_restored_or_unavailable_source() {
    const CHILD: &str = "HEX_DESKTOP_TEST_SAVED_AUTHORED_VST3";
    if std::env::var_os(CHILD).is_none() {
        let files = Files::new();
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "144")
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
    let mut fixture = nondefault_fixture_with_frames(8192);
    let device = fixture.project.main_devices()[0].id();
    let mut editor = hex_project::Editor::new(fixture.project.clone(), 8).unwrap();
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::Vst3 { parameter: 7 },
            points: vec![AutomationPoint {
                beat: BeatTime::ZERO,
                value: AutomationValue::new(0.5).unwrap(),
                outgoing: AutomationInterpolation::Step,
            }],
        })
        .unwrap();
    fixture.path = fixture.files.0.join("reading.hex");
    fixture.project = editor.project().clone();
    hex_storage::save_new(&fixture.path, &fixture.project).unwrap();
    let bytes = fs::read(&fixture.path).unwrap();

    let mut restored = reopen::open_saved(&fixture);
    restored
        .audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..3 {
        let pcm = restored.render_plugin_block();
        wait(|| restored.plugin_completed.load(Acquire) > block);
        if block == 2 {
            assert!(pcm.iter().all(|frame| *frame == [0.5; 2]), "{:?}", pcm[0]);
        }
    }
    assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
    drop(restored);

    let mut unavailable = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(unavailable.next().error.is_none());
    unavailable.applied(0);
    let old = unavailable
        .audio
        .as_ref()
        .unwrap()
        .control
        .output_identity()
        .unwrap();
    play_outgoing_before_open(&mut unavailable);
    let opened = unavailable.run(Request::Open(fixture.path.clone()));
    assert!(opened.project_replaced && opened.open_complete);
    assert_eq!(opened.state.project.as_ref(), &fixture.project);
    unavailable.release_relinquished_output();
    unavailable.wait_fresh_output(old);
    unavailable.applied(0);
    assert_eq!(
        unavailable
            .submitted_observation()
            .unwrap()
            .foreign_processor(device.value(), fixture.expected.type_id()),
        Some(false)
    );
    assert_eq!(unavailable.plugin_records.load(Acquire), 0);
    assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
}
