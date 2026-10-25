//! A normal transport terminal is data custody, never renewed playback authority.
use super::*;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_worker_quiescence_drains_held_live_capture_before_epoch_return() {
    const NAME: &str = "plugin_worker::capture::tests::stopped::actual_worker_quiescence_drains_held_live_capture_before_epoch_return";
    if fixture_child(71, NAME) {
        return;
    }
    let instance = original_instance();
    let child = instance.child_id_on_control();
    let (mut owner, callback) = NativeWorker::start_on_control(instance).unwrap();
    let old = begin(&mut owner);
    wait_held();
    owner.revoke_on_control();
    assert!(matches!(
        owner.poll_state_capture_on_control(&old).unwrap(),
        NativeWorkerCaptureProgress::Failed
    ));
    fs::write(
        PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap()),
        b"release",
    )
    .unwrap();
    let quiescent = owner.quiesce_on_control().unwrap();
    assert_eq!(quiescent.child_id_on_control(), child);
    assert!(owner.owns_callback(&callback));
    assert!(quiescent.retire_on_control().unwrap().status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_worker_stopped_capture_keeps_pair_and_retires_old_live_token() {
    let (mut owner, mut callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    let storage = owner.constructor_storage_on_control(&callback).unwrap();
    callback_block(&owner, &mut callback, 0);
    let old = begin(&mut owner);
    owner.revoke_on_control();
    assert!(matches!(
        owner.poll_state_capture_on_control(&old).unwrap(),
        NativeWorkerCaptureProgress::Failed
    ));
    let request = begin(&mut owner);
    assert!(owner.begin_state_capture_on_control().is_err());
    let state = complete(&mut owner, &request);
    assert_eq!(
        u32::from_le_bytes(
            hex_plugin_state::decode(&state).unwrap().component.unwrap()[4..8]
                .try_into()
                .unwrap()
        ),
        1
    );
    refused(&mut owner, &old);
    refused(&mut owner, &request);
    let next = begin(&mut owner);
    complete(&mut owner, &next);
    assert!(!owner.transport_is_live_on_control());
    assert_eq!(
        owner.constructor_storage_on_control(&callback),
        Some(storage)
    );
    assert!(owner.owns_callback(&callback));
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_worker_stopped_capture_drains_held_live_request_before_new_capture() {
    const NAME: &str = "plugin_worker::capture::tests::stopped::actual_worker_stopped_capture_drains_held_live_request_before_new_capture";
    if fixture_child(71, NAME) {
        return;
    }
    let (mut owner, callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    let old = begin(&mut owner);
    wait_held();
    owner.revoke_on_control();
    assert!(matches!(
        owner.poll_state_capture_on_control(&old).unwrap(),
        NativeWorkerCaptureProgress::Failed
    ));
    assert!(owner.begin_state_capture_on_control().is_err());
    fs::write(
        PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap()),
        b"release",
    )
    .unwrap();
    let next = begin(&mut owner);
    complete(&mut owner, &next);
    refused(&mut owner, &old);
    assert!(!owner.transport_is_live_on_control());
    assert!(owner.owns_callback(&callback));
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_worker_stopped_capture_sdk_refusal_retains_failure_and_native_lease() {
    const NAME: &str = "plugin_worker::capture::tests::stopped::actual_worker_stopped_capture_sdk_refusal_retains_failure_and_native_lease";
    if fixture_child(69, NAME) {
        return;
    }
    let (mut owner, callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    owner.revoke_on_control();
    let request = begin(&mut owner);
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner.poll_state_capture_on_control(&request).unwrap() {
            NativeWorkerCaptureProgress::Failed => break,
            NativeWorkerCaptureProgress::Pending => {
                assert!(Instant::now() < until);
                thread::yield_now();
            }
            NativeWorkerCaptureProgress::Captured(_) => panic!("SDK refusal cannot yield state"),
        }
    }
    let retired = owner.retire_on_control().unwrap();
    assert!(matches!(retired.failure, Some(Error::Worker(6))));
    assert!(!retired.native.status.success() && retired.native.terminal.is_none());
    assert!(owner.owns_callback(&callback));
}
