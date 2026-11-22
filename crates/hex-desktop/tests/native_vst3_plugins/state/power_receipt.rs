//! Stopped hosted Main power keeps canonical state behind the exact audio ACK.
use super::*;

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn stopped_main_power_waits_for_its_exact_callback_receipt() {
    let _native = native_fixture();
    let (mut h, _files, mut original) = fixture(Start::Saved);
    let (mut before_audio, _) = h.applied(0);
    let device = original.project.main_devices()[0].id();
    h.enable_confirmed_device_inserts();
    let binding = h.plugin_progress.lock().unwrap().binding;
    for (command, bypassed) in [
        (
            Command::SetDeviceBypass {
                device,
                bypassed: true,
            },
            true,
        ),
        (Command::ToggleDeviceBypass { device }, false),
    ] {
        assert!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .paused_input_interval()
                .is_some()
        );
        h.service.submit(Request::Edit(command)).unwrap();
        // No callback runs while this first event is received.
        let pending = h.next();
        assert!(pending.error.is_none(), "{:?}", pending.error);
        assert_eq!(pending.state.project, original.project);
        assert_eq!(pending.state.revision, original.revision);
        assert_eq!(
            (pending.state.can_undo, pending.state.can_redo),
            (original.can_undo, original.can_redo)
        );
        assert!(!pending.request_complete);
        assert_eq!(
            h.audio
                .as_ref()
                .unwrap()
                .status()
                .unwrap()
                .applied_revision
                .load(Acquire),
            before_audio
        );
        (before_audio, _) = h.applied(before_audio);
        let installed = edits::ordinary_terminal(&mut h);
        assert!(installed.error.is_none(), "{:?}", installed.error);
        assert!(installed.request_complete);
        assert_eq!(
            installed.state.project.device(device).unwrap().bypassed(),
            bypassed
        );
        assert_eq!(installed.state.revision, original.revision + 1);
        assert_eq!(
            installed.state.project.device(device).unwrap().state(),
            original.project.device(device).unwrap().state()
        );
        assert_eq!(h.plugin_progress.lock().unwrap().binding, binding);
        assert_eq!(h.plugin_records.load(Acquire), 1);
        original = installed.state;
    }
}

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn play_after_stopped_power_staging_refuses_without_canonical_commit() {
    let _native = native_fixture();
    let (mut h, _files, original) = fixture(Start::Saved);
    h.applied(0);
    let device = original.project.main_devices()[0].id();
    h.enable_confirmed_device_inserts();
    h.service
        .submit(Request::Edit(Command::ToggleDeviceBypass { device }))
        .unwrap();
    let pending = h.next();
    assert!(!pending.request_complete);
    assert_eq!(pending.state.project, original.project);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let terminal = edits::ordinary_terminal(&mut h);
    assert!(terminal.error.is_some());
    assert_eq!(terminal.state.project, original.project);
    assert_eq!(terminal.state.revision, original.revision);
    assert_eq!(
        (terminal.state.can_undo, terminal.state.can_redo),
        (original.can_undo, original.can_redo)
    );
}
