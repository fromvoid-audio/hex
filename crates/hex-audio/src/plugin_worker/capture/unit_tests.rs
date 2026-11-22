//! Fast non-SDK unit tests for `plugin_worker::capture`.
use super::*;

#[test]
fn helper_refusal_completes_only_its_operation_and_frees_the_mailbox() {
    let mut slot = Slot {
        ordinal: 4,
        phase: Phase::Requested,
        ..Slot::default()
    };
    assert!(slot.take_terminal().is_none(), "requested is still pending");
    // A stale ordinal never completes the current operation.
    assert!(
        slot.complete(3, Err(StateCaptureRefusal::StillChanging))
            .is_err()
    );
    slot.complete(4, Err(StateCaptureRefusal::StillChanging))
        .unwrap();
    assert!(matches!(
        slot.take_terminal(),
        Some(Err(StateCaptureRefusal::StillChanging))
    ));
    assert!(slot.phase == Phase::Idle);
    assert!(
        slot.take_terminal().is_none(),
        "a terminal is consumed once"
    );
    // The freed mailbox admits the next operation and carries real bytes.
    slot.ordinal = 5;
    slot.phase = Phase::Requested;
    let payload = NativeDeviceState::new(
        hex_plugin_state::TYPE_ID,
        hex_plugin_state::STATE_VERSION,
        vec![9; 150_545],
    )
    .unwrap();
    let original = payload.clone();
    slot.complete(5, Ok(payload)).unwrap();
    let Some(Ok(captured)) = slot.take_terminal() else {
        panic!("captured bytes")
    };
    assert_eq!(captured, original);
    assert!(captured.shares_payload_storage_with(&original));
    // Completing an idle slot is a protocol fault.
    assert!(slot.complete(5, Ok(original)).is_err());
}
