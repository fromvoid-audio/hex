//! Actual SDK input operations on the original callback/worker pair.
use super::*;
use crate::plugin_worker::{NativeWorkerParameterChange, NativeWorkerParameterProgress};
use hex_plugin_host::transport::ParameterPoint;

fn begin_change(owner: &mut NativeWorker, id: u32, value: f64) -> NativeWorkerParameterChange {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner.begin_parameter_change_on_control(id, value) {
            Ok(request) => return request,
            Err("native parameter mailbox is busy") => {}
            Err(error) => panic!("parameter admission: {error}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}
fn terminal(
    owner: &mut NativeWorker,
    request: &NativeWorkerParameterChange,
) -> NativeWorkerParameterProgress {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let result = owner.poll_parameter_change_on_control(request).unwrap();
        if result != NativeWorkerParameterProgress::Pending {
            return result;
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}
fn callback(
    owner: &NativeWorker,
    callback: &mut Callback,
    sequence: u64,
    gain: f32,
    parameters: &[ParameterPoint],
) {
    let mut output = [[9.0; 2]; 8];
    let run = || {
        callback.begin_physical_callback(8).unwrap();
        callback
            .stage_segment_with_parameters(0, &[[0.5, -0.25]; 8], &[], parameters, &mut output)
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
    let until = Instant::now() + Duration::from_secs(3);
    while owner.published_blocks_on_control() <= sequence {
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    let scale = gain * (0.5 + sequence.saturating_sub(2) as f32 * 0.125);
    assert_eq!(
        output,
        if sequence < 2 {
            [[0.0; 2]; 8]
        } else {
            [[0.5 * scale, -0.25 * scale]; 8]
        }
    );
}
fn trace() -> Vec<i32> {
    fs::read(trace_path())
        .unwrap()
        .chunks_exact(4)
        .map(|bytes| i32::from_le_bytes(bytes.try_into().unwrap()))
        .collect()
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_host_parameter_input_changes_same_sdk_source_and_captures_real_state() {
    live_change(
        "plugin_worker::capture::tests::parameter_change::actual_host_parameter_input_changes_same_sdk_source_and_captures_real_state",
        92,
    );
}
#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_inflight_host_parameter_input_cannot_be_withdrawn_as_unprocessed() {
    live_change(
        "plugin_worker::capture::tests::parameter_change::actual_inflight_host_parameter_input_cannot_be_withdrawn_as_unprocessed",
        95,
    );
}
fn live_change(name: &str, variant: u32) {
    if fixture_child(variant, name) {
        return;
    }
    let instance = original_instance();
    let child = instance.child_id_on_control();
    let initial = instance.ready().initial_state.clone();
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(instance).unwrap();
    let storage = owner.constructor_storage_on_control(&endpoint).unwrap();
    let id = if variant == 92 { 0 } else { 7 };
    let queued = begin_change(&mut owner, id, 0.5);
    if variant == 92 {
        let (mut other, other_endpoint) =
            NativeWorker::start_on_control(original_instance()).unwrap();
        assert!(
            other.poll_parameter_change_on_control(&queued).is_err(),
            "equal numeric bindings cannot replace the original source"
        );
        assert!(other.withdraw_parameter_change_on_control(&queued).is_err());
        other.revoke_on_control();
        let retired = other.retire_on_control().unwrap();
        assert!(retired.failure.is_none() && retired.native.status.success());
        assert!(other.owns_callback(&other_endpoint));
    }
    assert_eq!(
        terminal_pending(&mut owner, &queued),
        NativeWorkerParameterProgress::Pending
    );
    assert!(owner.begin_parameter_change_on_control(id, 0.75).is_err());
    assert!(
        owner
            .begin_parameter_change_on_control(id, f64::NAN)
            .is_err()
    );
    assert!(owner.withdraw_parameter_change_on_control(&queued).unwrap());
    assert_eq!(
        terminal(&mut owner, &queued),
        NativeWorkerParameterProgress::Withdrawn
    );
    assert!(owner.poll_parameter_change_on_control(&queued).is_err());
    assert_eq!(
        trace()[10],
        0,
        "admission/withdrawal never manufactures DSP"
    );
    let request = begin_change(&mut owner, id, 0.75);
    if variant == 95 {
        let mut output = [[0.0; 2]; 8];
        endpoint
            .stage(&[[0.5, -0.25]; 8], &[], &mut output)
            .unwrap();
        endpoint.finish(true).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while trace()[0] != 27 {
            assert!(Instant::now() < until, "actual SDK process hold");
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            !owner
                .withdraw_parameter_change_on_control(&request)
                .unwrap()
        );
        assert_eq!(
            terminal_pending(&mut owner, &request),
            NativeWorkerParameterProgress::Pending
        );
        fs::write(
            PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap()),
            b"release original SDK process",
        )
        .unwrap();
    } else {
        callback(&owner, &mut endpoint, 0, 3.0, &[]);
    }
    let NativeWorkerParameterProgress::Processed(processed) = terminal(&mut owner, &request) else {
        panic!("actual successful SDK input");
    };
    assert_eq!(
        (
            processed.point.id,
            processed.point.normalized,
            processed.point.frame,
            processed.sequence,
            processed.first_frame
        ),
        (id, 0.75, 0, 0, 0)
    );
    assert!(owner.poll_parameter_change_on_control(&request).is_err());
    for n in 1..4 {
        callback(&owner, &mut endpoint, n, 3.0, &[]);
    }
    let capture = begin(&mut owner);
    let actual = complete(&mut owner, &capture);
    assert_eq!(
        &hex_plugin_state::decode(&actual)
            .unwrap()
            .component
            .unwrap()[8..12],
        &3.0_f32.to_le_bytes()
    );
    assert_eq!(
        &hex_plugin_state::decode(&initial)
            .unwrap()
            .component
            .unwrap()[8..12],
        &1.0_f32.to_le_bytes()
    );
    assert_eq!(
        (trace()[8], trace()[9]),
        (1, 0),
        "no processing restart/reset during input"
    );
    assert_eq!(
        owner.constructor_storage_on_control(&endpoint),
        Some(storage)
    );
    let abandoned = begin_change(&mut owner, id, 0.25);
    owner.revoke_on_control();
    assert_eq!(
        terminal(&mut owner, &abandoned),
        NativeWorkerParameterProgress::Failed
    );
    assert!(owner.begin_parameter_change_on_control(id, 0.5).is_err());
    let stopped = owner.quiesce_on_control().unwrap();
    assert_eq!(stopped.child_id_on_control(), child);
    assert_eq!(stopped.completed_blocks(), 4);
    assert!(stopped.retire_on_control().unwrap().status.success());
    assert_eq!(trace()[1], 0);
}
fn terminal_pending(
    owner: &mut NativeWorker,
    request: &NativeWorkerParameterChange,
) -> NativeWorkerParameterProgress {
    owner.poll_parameter_change_on_control(request).unwrap()
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_authored_parameter_input_refuses_competing_host_point_without_replacing_audio() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::actual_authored_parameter_input_refuses_competing_host_point_without_replacing_audio";
    if fixture_child(91, NAME) {
        return;
    }
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let request = begin_change(&mut owner, 7, 0.75);
    callback(
        &owner,
        &mut endpoint,
        0,
        2.0,
        &[ParameterPoint {
            frame: 0,
            id: 7,
            normalized: 0.5,
        }],
    );
    assert_eq!(
        terminal(&mut owner, &request),
        NativeWorkerParameterProgress::Refused
    );
    for n in 1..4 {
        callback(&owner, &mut endpoint, n, 2.0, &[]);
    }
    let capture = begin(&mut owner);
    let actual = complete(&mut owner, &capture);
    assert_eq!(
        &hex_plugin_state::decode(&actual)
            .unwrap()
            .component
            .unwrap()[8..12],
        &2.0_f32.to_le_bytes()
    );
    owner.revoke_on_control();
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn rejected_physical_callback_never_consumes_queued_host_parameter_input() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::rejected_physical_callback_never_consumes_queued_host_parameter_input";
    if fixture_child(92, NAME) {
        return;
    }
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let request = begin_change(&mut owner, 0, 0.75);
    let mut output = [[9.0; 2]; 8];
    endpoint
        .stage(&[[0.5, -0.25]; 8], &[], &mut output)
        .unwrap();
    assert!(endpoint.finish(false).is_err());
    assert_eq!(
        terminal(&mut owner, &request),
        NativeWorkerParameterProgress::Failed
    );
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
    assert_eq!(trace()[10], 0);
}
