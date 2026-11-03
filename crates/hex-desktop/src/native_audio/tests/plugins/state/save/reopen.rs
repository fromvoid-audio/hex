//! Saved live component/controller bytes restored into an independent instance.
use super::*;

fn open_saved(fixture: &SavedEffectFixture) -> ResidentHarness {
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
