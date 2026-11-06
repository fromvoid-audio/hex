//! Actual paired callback and original SDK capture; no native app is started.
use super::*;
use crate::plugin_worker::native_tests::original_instance;
use std::{fs, path::PathBuf, process::Command};
mod controller;
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

fn trace_path() -> PathBuf {
    PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_TRACE").unwrap())
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_worker_edit_receipts_preserve_gestures_and_revoke_with_source() {
    use hex_plugin_host::protocol::ParameterEditKind;
    const NAME: &str = "plugin_worker::capture::tests::actual_worker_edit_receipts_preserve_gestures_and_revoke_with_source";
    if fixture_child(90, NAME) {
        return;
    }
    let (mut owner, mut callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    let storage = owner.constructor_storage_on_control(&callback).unwrap();
    // This actual SDK controller admits its gesture from the state read.
    // The joining worker must service receipts while capture remains pending,
    // then finish capture only after the one real accepted audio block.
    let request = begin(&mut owner);
    let mut edits = Vec::new();
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let batch = match owner.poll_parameter_edits_on_control() {
            Ok(batch) => batch,
            Err("native edit observation is busy") => None,
            Err(error) => panic!("gesture observation failed: {error}"),
        };
        if let Some(batch) = batch {
            edits.extend_from_slice(batch.as_slice());
            if batch.ready > 0 && edits.len() == 1 {
                break;
            }
        }
        assert!(
            Instant::now() < until,
            "actual GUI perform must remain pending until PCM"
        );
        thread::yield_now();
    }
    assert_eq!(owner.published_blocks_on_control(), 0);
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].kind, ParameterEditKind::Begin);
    callback_block(&owner, &mut callback, 0);
    let until = Instant::now() + Duration::from_secs(3);
    while edits.len() < 3 {
        let batch = match owner.poll_parameter_edits_on_control() {
            Ok(batch) => batch,
            Err("native edit observation is busy") => None,
            Err(error) => panic!("gesture observation failed: {error}"),
        };
        if let Some(batch) = batch {
            edits.extend_from_slice(batch.as_slice());
        }
        assert!(
            Instant::now() < until,
            "actual applied gesture must reach control"
        );
        thread::yield_now();
    }
    assert_eq!(edits.len(), 3);
    for (index, kind) in [
        ParameterEditKind::Begin,
        ParameterEditKind::Perform,
        ParameterEditKind::End,
    ]
    .into_iter()
    .enumerate()
    {
        let edit = edits[index];
        assert_eq!(
            (edit.sequence, edit.gesture, edit.id, edit.kind),
            (index as u64 + 1, 1, 7, kind)
        );
        // The main dispatcher can enqueue End after this one real block.
        // Begin/Perform are known admitted before processing; End retains
        // its actual consumed prefix instead of borrowing Perform's offset.
        assert!(edit.first_frame <= 8);
        if kind != ParameterEditKind::End {
            assert_eq!(edit.first_frame, 0);
        }
        assert_eq!(
            edit.normalized,
            if kind == ParameterEditKind::Perform {
                0.75
            } else {
                0.0
            }
        );
    }
    let captured = complete(&mut owner, &request);
    assert_eq!(
        &hex_plugin_state::decode(&captured)
            .unwrap()
            .component
            .unwrap()[8..12],
        &3.0_f32.to_le_bytes()
    );
    assert_eq!(owner.published_blocks_on_control(), 1);
    assert_eq!(
        owner.constructor_storage_on_control(&callback),
        Some(storage)
    );
    callback.reject(Fault::Revoked);
    assert!(owner.poll_parameter_edits_on_control().is_err());
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_idle_editor_poll_applies_a_burst_without_advancing_audio() {
    const NAME: &str = "plugin_worker::capture::tests::actual_idle_editor_poll_applies_a_burst_without_advancing_audio";
    if fixture_child(116, NAME) {
        return;
    }
    let (mut owner, _callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"emit editor burst while transport is idle",
    )
    .unwrap();
    let mut edits = Vec::new();
    let until = Instant::now() + Duration::from_secs(2);
    while edits.len() < 14 {
        match owner.poll_recovery_edits_on_control(true) {
            Ok(Some(batch)) => edits.extend_from_slice(batch.edits.as_slice()),
            Ok(None) | Err("native edit observation is busy") => {}
            Err(error) => panic!("idle editor poll failed: {error}"),
        }
        assert!(Instant::now() < until, "idle editor burst must drain");
        thread::yield_now();
    }
    assert_eq!(edits.len(), 14);
    assert!(edits.iter().all(|edit| edit.first_frame == 0));
    assert_eq!(owner.published_blocks_on_control(), 0);
    let request = begin(&mut owner);
    let captured = complete(&mut owner, &request);
    assert_eq!(
        &hex_plugin_state::decode(&captured)
            .unwrap()
            .component
            .unwrap()[8..12],
        &3.0_f32.to_le_bytes()
    );
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
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
            NativeWorkerCaptureProgress::Refused(refusal) => panic!("capture refused: {refusal}"),
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
            NativeWorkerCaptureProgress::Refused(refusal) => {
                panic!("SDK refusal is not a helper refusal: {refusal}")
            }
        }
    }
    let retired = owner.retire_on_control().unwrap();
    assert!(matches!(retired.failure, Some(Error::Worker(6))));
    assert!(!retired.native.status.success() && retired.native.terminal.is_none());
    assert!(owner.owns_callback(&callback));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_parameter_feedback_survives_busy_capture_slot_and_callback_replies_without_allocations() {
    const NAME: &str = "plugin_worker::capture::tests::actual_parameter_feedback_survives_busy_capture_slot_and_callback_replies_without_allocations";
    if fixture_child(37, NAME) {
        return;
    }
    let (mut owner, mut callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    let signal = Arc::clone(&owner.reaper.completed);
    let slot = signal.capture.lock().unwrap();
    for sequence in 0..4 {
        let mut output = [[9.0; 2]; 8];
        let point = ParameterPoint {
            frame: 0,
            id: 7,
            normalized: (sequence + 1) as f64 / 8.0,
        };
        let run = || {
            callback.begin_physical_callback(8).unwrap();
            let reply = callback
                .stage_segment_with_parameters(0, &[[0.5, -0.25]; 8], &[], &[point], &mut output)
                .unwrap();
            if sequence >= 2 {
                assert_eq!(
                    reply.parameters.as_slice(),
                    &[ParameterPoint {
                        frame: 7,
                        id: 7,
                        normalized: (sequence - 1) as f64 / 8.0,
                    }]
                );
            }
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
        let expected = if sequence < 2 {
            [0.0; 2]
        } else {
            let gain = (sequence - 1) as f32 / 2.0 * (0.5 + (sequence - 2) as f32 * 0.125);
            [0.5 * gain, -0.25 * gain]
        };
        assert_eq!(output, [expected; 8]);
        let until = Instant::now() + Duration::from_secs(2);
        while owner.published_blocks_on_control() <= sequence {
            assert!(
                Instant::now() < until,
                "capture contention cannot block PCM"
            );
            thread::yield_now();
        }
        assert!(
            owner.parameter_values_on_control().is_err(),
            "busy is an explicit read refusal"
        );
    }
    drop(slot);
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(values) = owner.parameter_values_on_control()
            && values.completed_blocks() == 4
        {
            assert_eq!(
                values.as_slice(),
                &[super::super::NativeParameterValue {
                    id: 7,
                    normalized: 0.5
                }]
            );
            break;
        }
        assert!(
            Instant::now() < until,
            "retained worker observation must publish after contention"
        );
        thread::yield_now();
    }
    let request = begin(&mut owner);
    let captured = complete(&mut owner, &request);
    assert_eq!(
        &hex_plugin_state::decode(&captured)
            .unwrap()
            .component
            .unwrap()[8..12],
        &2.0_f32.to_le_bytes()
    );
    callback.reject(Fault::Revoked);
    assert!(
        owner.parameter_values_on_control().is_err(),
        "old epoch cannot publish observations"
    );
    let retirement = owner.retire_on_control().unwrap();
    assert!(retirement.failure.is_none());
}
