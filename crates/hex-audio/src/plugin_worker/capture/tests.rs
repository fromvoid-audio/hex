//! Actual paired callback and original SDK capture; no native app is started.
use super::*;
use crate::plugin_worker::native_tests::original_instance;
use std::{fs, path::PathBuf, process::Command};
mod stopped;

fn fixture_child(fault: u32, name: &str) -> bool {
    if std::env::var_os("HEX_WORKER_CAPTURE_CHILD").is_some() {
        return false;
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "hex-worker-capture-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            name,
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env("HEX_WORKER_CAPTURE_CHILD", "1")
        .env("HEX_ORIGINAL_INSTANCE_VARIANT", fault.to_string())
        .env("HEX_ORIGINAL_INSTANCE_TRACE", root.join("trace"))
        .env("HEX_ORIGINAL_INSTANCE_RELEASE", root.join("release"))
        .output()
        .unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(
        output.status.success(),
        "child: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "fixture child must execute the exact requested test"
    );
    true
}
fn trace_path() -> PathBuf {
    PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_TRACE").unwrap())
}
fn wait_held() {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        if fs::read(trace_path())
            .ok()
            .is_some_and(|bytes| bytes.get(..4) == Some(&22_i32.to_ne_bytes()))
        {
            return;
        }
        assert!(Instant::now() < until, "SDK capture hold");
        thread::sleep(Duration::from_millis(1));
    }
}
fn callback_block(owner: &NativeWorker, callback: &mut Callback, sequence: u64) {
    let mut output = [[9.0; 2]; 8];
    let run = || {
        callback
            .stage(&[[0.5, -0.25]; 8], &[], &mut output)
            .unwrap();
        callback.finish(true).unwrap();
    };
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(run);
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    {
        let mut run = run;
        run();
    }
    let until = Instant::now() + Duration::from_secs(2);
    while owner.published_blocks_on_control() <= sequence {
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    let gain = 0.5 + sequence.saturating_sub(2) as f32 * 0.125;
    assert_eq!(
        output,
        if sequence < 2 {
            [[0.0; 2]; 8]
        } else {
            [[0.5 * gain, -0.25 * gain]; 8]
        }
    );
}
fn complete(owner: &mut NativeWorker, request: &NativeWorkerCapture) -> NativeDeviceState {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match owner.poll_state_capture_on_control(request).unwrap() {
            NativeWorkerCaptureProgress::Captured(state) => return state,
            NativeWorkerCaptureProgress::Failed => panic!("capture failed"),
            NativeWorkerCaptureProgress::Pending => {
                assert!(Instant::now() < until);
                thread::yield_now();
            }
        }
    }
}

fn begin(owner: &mut NativeWorker) -> NativeWorkerCapture {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match owner.begin_state_capture_on_control() {
            Ok(request) => return request,
            Err("native capture mailbox is busy" | "native preparation is busy") => {
                assert!(Instant::now() < until);
                thread::yield_now();
            }
            Err(reason) => panic!("capture admission: {reason}"),
        }
    }
}
fn refused(owner: &mut NativeWorker, request: &NativeWorkerCapture) {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match owner.poll_state_capture_on_control(request) {
            Err(_) => return,
            Ok(NativeWorkerCaptureProgress::Pending) => {
                assert!(Instant::now() < until);
                thread::yield_now();
            }
            other => panic!("stale capture token accepted: {other:?}"),
        }
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_worker_capture_preserves_pair_storage_and_exact_operation() {
    let instance = original_instance();
    let initial = instance.ready().initial_state.clone();
    let quote = NativeWorker::preconstruction_data_bytes_on_control(&instance).unwrap();
    let (mut owner, mut callback) = NativeWorker::start_on_control(instance).unwrap();
    let storage = owner.constructor_storage_on_control(&callback).unwrap();
    assert_eq!(storage.data_bytes, quote);
    callback_block(&owner, &mut callback, 0);
    let first = begin(&mut owner);
    assert!(owner.begin_state_capture_on_control().is_err());
    let other_instance = original_instance(); // same scalar binding, different original shared cell
    let (mut other, other_callback) = NativeWorker::start_on_control(other_instance).unwrap();
    assert!(other.poll_state_capture_on_control(&first).is_err());
    let captured = complete(&mut owner, &first);
    assert_ne!(captured, initial);
    refused(&mut owner, &first);
    let second = begin(&mut owner);
    refused(&mut owner, &first);
    complete(&mut owner, &second);
    assert_eq!(
        owner.constructor_storage_on_control(&callback),
        Some(storage)
    );
    assert!(owner.owns_callback(&callback));
    assert!(!owner.owns_callback(&other_callback));
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
    assert!(other.retire_on_control().unwrap().native.status.success());
    assert!(owner.begin_state_capture_on_control().is_err());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_worker_capture_projection_failure_releases_exact_operation() {
    let (mut owner, mut callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    let first = begin(&mut owner);
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match owner.poll_state_capture_using(&first, |_| Err("fixture allocation refusal")) {
            Ok(NativeWorkerCaptureProgress::Pending) => {
                assert!(Instant::now() < until);
                thread::yield_now();
            }
            Err("fixture allocation refusal") => break,
            result => panic!("unexpected capture result: {result:?}"),
        }
    }
    refused(&mut owner, &first);
    let next = begin(&mut owner);
    complete(&mut owner, &next);
    callback_block(&owner, &mut callback, 0);
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_worker_capture_held_sdk_keeps_callback_pcm_and_revocation_wins() {
    if fixture_child(
        71,
        "plugin_worker::capture::tests::actual_worker_capture_held_sdk_keeps_callback_pcm_and_revocation_wins",
    ) {
        return;
    }
    let (mut owner, mut callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    let storage = owner.constructor_storage_on_control(&callback).unwrap();
    let request = begin(&mut owner);
    wait_held();
    for sequence in 0..4 {
        assert!(matches!(
            owner.poll_state_capture_on_control(&request).unwrap(),
            NativeWorkerCaptureProgress::Pending
        ));
        callback_block(&owner, &mut callback, sequence);
    }
    fs::write(
        PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap()),
        b"release",
    )
    .unwrap();
    let captured = complete(&mut owner, &request);
    assert!(
        hex_plugin_state::decode(&captured)
            .unwrap()
            .component
            .is_some()
    );
    callback_block(&owner, &mut callback, 4);
    assert_eq!(
        owner.constructor_storage_on_control(&callback),
        Some(storage)
    );
    let stale = begin(&mut owner);
    owner.revoke_on_control();
    assert!(matches!(
        owner.poll_state_capture_on_control(&stale).unwrap(),
        NativeWorkerCaptureProgress::Failed
    ));
    let retired = owner.retire_on_control().unwrap();
    assert!(owner.owns_callback(&callback));
    assert!(retired.native.status.success() || retired.native.terminal.is_none());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_worker_retirement_cancels_held_capture_through_original_lease() {
    if fixture_child(
        71,
        "plugin_worker::capture::tests::actual_worker_retirement_cancels_held_capture_through_original_lease",
    ) {
        return;
    }
    let (mut owner, mut callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    let request = begin(&mut owner);
    wait_held();
    callback_block(&owner, &mut callback, 0);
    owner.revoke_on_control();
    assert!(matches!(
        owner.poll_state_capture_on_control(&request).unwrap(),
        NativeWorkerCaptureProgress::Failed
    ));
    let retired = owner.retire_on_control().unwrap();
    assert!(
        matches!(retired.failure, Some(Error::Cancelled)),
        "{:?}",
        retired.failure
    );
    assert_eq!(retired.native.status.code(), Some(9));
    assert!(retired.native.terminal.is_none());
    assert!(owner.owns_callback(&callback));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_worker_capture_failure_retains_original_error_and_native_reap() {
    if fixture_child(
        69,
        "plugin_worker::capture::tests::actual_worker_capture_failure_retains_original_error_and_native_reap",
    ) {
        return;
    }
    let (mut owner, callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    let request = begin(&mut owner);
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner.poll_state_capture_on_control(&request).unwrap() {
            NativeWorkerCaptureProgress::Failed => break,
            NativeWorkerCaptureProgress::Pending => {
                assert!(Instant::now() < until);
                thread::yield_now();
            }
            NativeWorkerCaptureProgress::Captured(_) => panic!("SDK refusal cannot produce state"),
        }
    }
    let retired = owner.retire_on_control().unwrap();
    assert!(matches!(retired.failure, Some(Error::Worker(6))));
    assert!(!retired.native.status.success() && retired.native.terminal.is_none());
    assert!(owner.owns_callback(&callback));
}
