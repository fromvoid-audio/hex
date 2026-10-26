use super::*;
use crate::device_gesture::{
    DeviceParameterSnapshot, GestureKey, GestureRequest, ParameterId, ParameterValue,
};

#[test]
fn snapshot_full_and_closed_queue_return_exact_inline_terminal_for_retry() {
    let (tx, rx) = mpsc::sync_channel(REQUEST_CAPACITY);
    let tx = request_cancellation::Sender::new(tx, Arc::new(std::sync::atomic::AtomicU64::new(0)));
    let snapshot =
        DeviceParameterSnapshot::new(&[(ParameterId::from_static(1), ParameterValue::Float(0.5))])
            .unwrap();
    let terminal = GestureRequest::CommitSnapshot {
        touched_parameters: crate::device_gesture::TouchedParameters::default(),
        key: GestureKey {
            generation: 3,
            token: 9,
        },
        sequence: 17,
        snapshot,
    };
    for _ in 0..REQUEST_CAPACITY {
        assert!(try_submit(&tx, Request::DeviceGesture(terminal)).is_ok());
    }
    let Err(SubmitError::Full(Request::DeviceGesture(retry))) =
        try_submit(&tx, Request::DeviceGesture(terminal))
    else {
        panic!("full queue must return terminal");
    };
    assert_eq!(retry, terminal);
    assert!(
        matches!(rx.recv().unwrap().request, Request::DeviceGesture(request) if request == terminal)
    );
    assert!(try_submit(&tx, Request::DeviceGesture(retry)).is_ok());
    drop(rx);
    let Err(SubmitError::Closed(Request::DeviceGesture(retry))) =
        try_submit(&tx, Request::DeviceGesture(terminal))
    else {
        panic!("closed queue must return terminal");
    };
    assert_eq!(retry, terminal);
}
