//! Stopped hosted Main power keeps canonical state behind the exact audio ACK.
use super::*;

#[test]
#[ignore = "explicit original SDK helper/source receipts required"]
fn stopped_main_power_waits_for_its_exact_callback_receipt() {
    let _native = native_fixture();
    let (mut h, _files, original) = fixture(Start::Saved);
    let (before_audio, _) = h.applied(0);
    let device = original.project.main_devices()[0].id();
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .paused_input_interval()
            .is_some()
    );
    h.service
        .submit(Request::Edit(Command::SetDeviceBypass {
            device,
            bypassed: true,
        }))
        .unwrap();
    // No callback runs while this first event is received.
    let pending = h.next();
    assert!(pending.error.is_none(), "{:?}", pending.error);
    assert_eq!(pending.state.project, original.project);
    assert_eq!(pending.state.revision, original.revision);
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
    h.applied(before_audio);
    let installed = edits::ordinary_terminal(&mut h);
    assert!(installed.error.is_none(), "{:?}", installed.error);
    assert!(installed.request_complete);
    assert!(installed.state.project.device(device).unwrap().bypassed());
    assert_eq!(installed.state.revision, original.revision + 1);
    assert_eq!(
        installed.state.project.device(device).unwrap().state(),
        original.project.device(device).unwrap().state()
    );
}
