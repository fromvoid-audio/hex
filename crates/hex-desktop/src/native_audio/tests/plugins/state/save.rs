//! Delayed original SDK capture through the real Service Save request.
use super::*;
mod authored_reopen;
mod batch;
mod reopen;

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn native_effect_save_keeps_pending_mix_publication_and_metadata_alias() {
    let _native = native_fixture();
    let (mut h, files, _) = fixture(Start::Saved);
    let (applied, _) = h.applied(0);
    let added = same_source_edit(
        &mut h,
        Request::Edit(Command::InsertMainDevice {
            before: None,
            state: hex_engine::eq::EqState::default().encode().unwrap(),
        }),
    );
    let Some(Created::Device(alias_device)) = added.created else {
        panic!("comparison fixture")
    };
    let (applied, _) = h.applied(applied);
    h.run(Request::SaveCurrent);
    let path = files.0.join("effect.hex");
    let changed = same_source_edit(&mut h, Request::Edit(Command::MasterGain { gain: 0.5 }));
    assert!(changed.state.unsaved);
    // This pending graph retains the exact accepted plugin. Save must capture
    // it and save the authored mix value without requiring a callback ACK.
    h.service.submit(Request::SaveCurrent).unwrap();
    let saved = h.next();
    assert!(
        saved.save_complete && saved.error.is_none(),
        "{:?}",
        saved.error
    );
    assert!(!saved.state.unsaved);
    assert_eq!(saved.state.project, changed.state.project);
    assert_eq!(
        &hex_storage::load(&path).unwrap(),
        saved.state.project.as_ref()
    );
    h.applied(applied);
    // Use the existing builtin A/B provider. Foreign A/B validation is not yet
    // admitted, but this sealed project-wide alias also retains the VST3 source.
    let device = saved.state.project.device(alias_device).unwrap();
    let prepared_context = saved
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context();
    let compared = h.run(Request::CompareDevice {
        owner: hex_session::service::preset::Owner {
            project_generation: prepared_context.project_generation,
            revision: saved.state.revision,
            device: device.id(),
            class: hex_engine::NativeDeviceClass::Effect,
            type_id: device.state().type_id(),
        },
        expected: hex_project::DeviceVariant::A,
        action: hex_project::DeviceComparisonAction::Switch,
    });
    assert!(compared.state.comparison_audio.is_some());
    assert_eq!(
        compared.state.audio_authoring_revision(prepared_context),
        Some(compared.state.revision)
    );
    h.service.submit(Request::SaveCurrent).unwrap();
    let aliased = h.next();
    assert!(
        aliased.error.is_none(),
        "save after metadata alias: {:?}",
        aliased.error
    );
    assert!(aliased.save_complete && !aliased.state.unsaved);
    assert_eq!(aliased.state.project, compared.state.project);
    assert_eq!(
        hex_storage::load(&path).unwrap().main_devices(),
        compared.state.project.main_devices()
    );
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn native_effect_save_after_stop_preserves_original_source_for_later_playback() {
    let _native = native_fixture();
    let (mut h, files, before) = fixture(Start::Saved);
    let (applied, _) = h.applied(0);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..4 {
        h.render_plugin_block();
        wait(|| h.plugin_completed.load(Acquire) > block);
    }
    h.audio.as_ref().unwrap().control.stop();
    h.render_plugin_block();
    let saved = h.run(Request::SaveCurrent);
    assert!(saved.save_complete);
    assert_eq!(saved.state.project, before.project);
    assert_eq!(saved.state.revision, before.revision);
    assert_eq!(saved.state.can_undo, before.can_undo);
    assert_eq!(saved.state.can_redo, before.can_redo);
    let loaded = hex_storage::load(files.0.join("effect.hex")).unwrap();
    let captured = hex_plugin_state::decode(loaded.main_devices()[0].state()).unwrap();
    assert_eq!(
        u32::from_le_bytes(captured.component.unwrap()[4..8].try_into().unwrap()),
        4
    );
    let mut restored = h.run(Request::RestoreStoppedPlayback);
    if !restored.stopped_playback_restore_complete {
        restored = restored_terminal(&mut h);
    }
    assert!(restored.error.is_none(), "{:?}", restored.error);
    assert_eq!(restored.state.project, before.project);
    h.applied(applied);
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
        let pcm = h.render_plugin_block();
        wait(|| h.plugin_completed.load(Acquire) > block);
        if block == 2 {
            assert!(
                pcm.iter().all(|frame| *frame == [0.25; 2]),
                "the original DSP counter must survive Save and epoch renewal"
            );
        }
    }
    assert_eq!(h.plugin_records.load(Acquire), 1);
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn native_effect_save_delayed_capture_keeps_pcm_and_drains_cancelled_request() {
    const NAME: &str = "native_audio::tests::plugins::state::save::native_effect_save_delayed_capture_keeps_pcm_and_drains_cancelled_request";
    let Some(cancel) = std::env::var_os("HEX_DESKTOP_SAVE_CAPTURE_CHILD") else {
        for cancel in ["0", "1"] {
            let files = Files::new();
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    NAME,
                    "--ignored",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env("HEX_DESKTOP_SAVE_CAPTURE_CHILD", cancel)
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", "71")
                .env("HEX_ORIGINAL_INSTANCE_TRACE", files.0.join("capture-trace"))
                .env("HEX_ORIGINAL_RESTORE_TRACE", files.0.join("state-trace"))
                .env(
                    "HEX_ORIGINAL_INSTANCE_RELEASE",
                    files.0.join("capture-release"),
                )
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&child.stdout),
                String::from_utf8_lossy(&child.stderr)
            );
            assert!(String::from_utf8_lossy(&child.stdout).contains("1 passed"));
        }
        return;
    };
    let _native = native_fixture();
    let cancel = cancel == "1";
    let (mut h, files, before) = fixture(Start::Unsaved);
    let path = files.0.join("captured.hex");
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for block in 0..4 {
        h.render_plugin_block();
        wait(|| h.plugin_completed.load(Acquire) > block);
    }
    h.service.submit(Request::SaveNew(path.clone())).unwrap();
    let trace = PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_TRACE").unwrap());
    wait(|| {
        h.pump();
        fs::read(&trace)
            .ok()
            .is_some_and(|bytes| bytes.get(..4) == Some(&22_i32.to_ne_bytes()))
    });
    for _ in 4..7 {
        let pcm = h.render_plugin_block();
        // Save owns the Session worker, so its adapter observation counter
        // cannot advance here. Observe actual callback request completion.
        wait(|| {
            h.callback
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .native_workers_idle_on_control()
                == Some(true)
        });
        assert!(pcm.iter().any(|frame| *frame != [0.0; 2]));
        assert!(!path.exists());
        if let Ok(event) = h.service.try_event() {
            panic!(
                "event during held capture: save={}, complete={}, status={:?}, error={:?}",
                event.save_complete, event.request_complete, event.status, event.error
            );
        }
    }
    if cancel {
        // This accepted intent cancels Save, but its own missing-file preflight
        // fails. The healthy original instance must remain usable afterwards.
        h.service
            .submit(Request::Open(files.0.join("missing.hex")))
            .unwrap();
    }
    fs::write(
        PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap()),
        b"release",
    )
    .unwrap();
    let saved = h.next();
    assert!(saved.save_complete);
    assert_eq!(saved.state.project, before.project);
    assert_eq!(saved.state.revision, before.revision);
    assert_eq!(saved.state.can_undo, before.can_undo);
    assert_eq!(saved.state.can_redo, before.can_redo);
    if cancel {
        assert!(saved.error.is_some());
        assert!(!path.exists());
        let refused_open = h.next();
        assert!(refused_open.error.is_some());
        assert_eq!(refused_open.state.project, before.project);
        let retry = h.run(Request::SaveNew(path.clone()));
        assert!(retry.save_complete);
    } else {
        assert!(saved.error.is_none(), "{:?}", saved.error);
    }
    let loaded = hex_storage::load(&path).unwrap();
    // This fault variant stores fixed latency rather than its DSP block count.
    // Equal bytes are legitimate; the original SDK trace proves fresh calls.
    assert_eq!(
        loaded.main_devices()[0].state(),
        before.project.main_devices()[0].state()
    );
    assert_eq!(loaded.tracks(), before.project.tracks());
    let trace = fs::read(PathBuf::from(
        std::env::var_os("HEX_ORIGINAL_RESTORE_TRACE").unwrap(),
    ))
    .unwrap();
    let observed: Vec<_> = trace
        .chunks_exact(4)
        .map(|bytes| i32::from_ne_bytes(bytes.try_into().unwrap()))
        .collect();
    assert_eq!(observed.len(), 15);
    let expected_calls = if cancel { 3 } else { 2 };
    assert_eq!(
        observed[10], expected_calls,
        "actual component getState calls"
    );
    assert_eq!(
        observed[11], expected_calls,
        "actual controller getState calls"
    );
    assert_eq!(observed[12], 1, "the original controller remains loaded");
}
