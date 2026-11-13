//! Actual SDK input operations on the original callback/worker pair.
use super::*;
use crate::plugin_worker::{NativeWorkerParameterChange, NativeWorkerParameterProgress};
use hex_plugin_host::transport::ParameterPoint;

mod unprogrammed;

const HELD_PROCESS_FIXTURE: u32 = 140;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn captured_state_follows_only_the_latest_processed_input_on_the_original_worker() {
    use crate::plugin_worker::NativeWorkerSequencedCaptureProgress as Progress;
    let name = std::thread::current().name().unwrap().to_owned();
    if fixture_child(92, &name) {
        return;
    }
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let (mut other, _other_endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let input = begin_change(&mut owner, 0, 0.5);
    assert!(
        owner
            .begin_state_capture_after_parameter_on_control(&input)
            .is_err()
    );
    assert!(
        other
            .begin_state_capture_after_parameter_on_control(&input)
            .is_err()
    );
    for sequence in 0..3 {
        callback(&owner, &mut endpoint, sequence, 2.0, &[]);
    }
    assert!(matches!(
        terminal(&mut owner, &input),
        NativeWorkerParameterProgress::Processed(_)
    ));
    let capture = owner
        .begin_state_capture_after_parameter_on_control(&input)
        .unwrap();
    assert!(owner.poll_state_capture_on_control(&capture).is_err());
    let until = Instant::now() + Duration::from_secs(3);
    let fact = loop {
        match owner.poll_state_capture_after_parameter_on_control(&capture) {
            Ok(Progress::Pending) | Err("native capture mailbox is busy") => {}
            Ok(Progress::Captured(fact)) => break fact,
            _ => panic!("the sequenced capture must complete"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    };
    let decoded = hex_plugin_state::decode(fact.state()).unwrap();
    assert_eq!(fact.parameter(), 0);
    assert_eq!(fact.normalized_input(), 0.5);
    assert_eq!(
        f32::from_le_bytes(decoded.component.unwrap()[8..12].try_into().unwrap()),
        2.0
    );
    assert!(
        owner
            .sequenced_capture_is_current_on_control(&fact)
            .unwrap()
    );
    assert!(
        !other
            .sequenced_capture_is_current_on_control(&fact)
            .unwrap()
    );
    let stale_capture = owner
        .begin_state_capture_after_parameter_on_control(&input)
        .unwrap();
    let next = begin_change(&mut owner, 0, 0.75);
    assert!(
        !owner
            .sequenced_capture_is_current_on_control(&fact)
            .unwrap()
    );
    assert!(
        owner
            .begin_state_capture_after_parameter_on_control(&input)
            .is_err()
    );
    loop {
        match owner.poll_state_capture_after_parameter_on_control(&stale_capture) {
            Ok(Progress::Pending) | Err("native capture mailbox is busy") => {}
            Err("native sequenced capture was superseded") => break,
            _ => panic!("superseded capture must drain and refuse"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    let plain = owner.begin_state_capture_on_control().unwrap();
    let _ = complete(&mut owner, &plain);
    assert!(owner.withdraw_parameter_change_on_control(&next).unwrap());
    assert_eq!(
        terminal(&mut owner, &next),
        NativeWorkerParameterProgress::Withdrawn
    );
    assert!(
        owner
            .begin_state_capture_after_parameter_on_control(&next)
            .is_err()
    );
    owner.revoke_on_control();
    assert!(
        !owner
            .sequenced_capture_is_current_on_control(&fact)
            .unwrap()
    );
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
    other.revoke_on_control();
    let retired = other.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn editor_can_change_worker_after_live_capture() {
    let name = std::thread::current().name().unwrap().to_owned();
    if fixture_child(89, &name) {
        return;
    }
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let capture = begin(&mut owner);
    let before = complete(&mut owner, &capture);
    fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"editor",
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner.poll_parameter_edits_on_control() {
            Ok(Some(batch)) if batch.ready > 0 => break,
            Ok(_) | Err("native edit observation is busy") => {}
            other => panic!("editor observation: {other:?}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    for sequence in 0..3 {
        callback(&owner, &mut endpoint, sequence, 3.0, &[]);
    }
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner.poll_parameter_edits_on_control() {
            Ok(Some(batch))
                if batch.as_slice().iter().any(|edit| {
                    edit.kind == hex_plugin_host::protocol::ParameterEditKind::Perform
                }) =>
            {
                break;
            }
            Ok(_) | Err("native edit observation is busy") => {}
            other => panic!("editor receipt: {other:?}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    let capture = begin(&mut owner);
    let after = complete(&mut owner, &capture);
    let gain = |state: &NativeDeviceState| {
        let decoded = hex_plugin_state::decode(state).unwrap();
        f32::from_le_bytes(decoded.component.unwrap()[8..12].try_into().unwrap())
    };
    assert_eq!(gain(&before), 1.0);
    assert_eq!(gain(&after), 3.0);
    owner.revoke_on_control();
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn editor_can_change_worker_during_manual_hold() {
    use crate::plugin_worker::NativeParameterHold;
    let name = std::thread::current().name().unwrap().to_owned();
    if fixture_child(147, &name) {
        return;
    }
    let mut instance = original_instance();
    let class = instance.configuration().identity.class;
    let metadata = instance
        .read_authored_parameter_metadata_on_control(class, &[7])
        .unwrap();
    instance
        .arm_authored_parameter_metadata_on_control(metadata)
        .unwrap();
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(instance).unwrap();
    let hold = owner
        .parameter_hold_on_control(NativeParameterHold::Begin {
            id: 7,
            normalized: 0.5,
        })
        .unwrap();
    for sequence in 0..3 {
        callback(&owner, &mut endpoint, sequence, 2.0, &[]);
    }
    assert!(matches!(
        terminal(&mut owner, &hold),
        NativeWorkerParameterProgress::Processed(_)
    ));
    fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"editor",
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner.poll_parameter_edits_on_control() {
            Ok(Some(batch)) if batch.ready > 0 => break,
            Ok(_) | Err("native edit observation is busy") => {}
            other => panic!("editor observation: {other:?}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    for sequence in 3..6 {
        callback(
            &owner,
            &mut endpoint,
            sequence,
            if sequence < 5 { 2.0 } else { 3.0 },
            &[],
        );
    }
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner.poll_parameter_edits_on_control() {
            Ok(Some(batch))
                if batch.as_slice().iter().any(|edit| {
                    edit.kind == hex_plugin_host::protocol::ParameterEditKind::Perform
                }) =>
            {
                break;
            }
            Ok(_) | Err("native edit observation is busy") => {}
            other => panic!("editor receipt: {other:?}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    let capture = begin(&mut owner);
    let after = complete(&mut owner, &capture);
    let decoded = hex_plugin_state::decode(&after).unwrap();
    assert_eq!(
        f32::from_le_bytes(decoded.component.unwrap()[8..12].try_into().unwrap()),
        3.0
    );
    let update = owner
        .parameter_hold_on_control(NativeParameterHold::Change {
            hold: &hold,
            normalized: 0.5,
        })
        .unwrap();
    callback(&owner, &mut endpoint, 6, 3.0, &[]);
    assert!(matches!(
        terminal(&mut owner, &update),
        NativeWorkerParameterProgress::Processed(_)
    ));
    owner.revoke_on_control();
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

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
fn actual_parameter_reservation_precedes_publication_and_refusal_keeps_the_ordinal() {
    let name = std::thread::current().name().unwrap().to_owned();
    if fixture_child(92, &name) {
        return;
    }
    let instance = original_instance();
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(instance).unwrap();
    let signal = Arc::clone(&owner.reaper.completed);
    let mut refused = None;
    let result: Result<(NativeWorkerParameterChange, ()), _> =
        owner.parameter_change_with_before(0, 0.75, false, |request| {
            assert!(!signal.parameter_change_requested.load(Acquire));
            refused = request.delivery_identity();
            Err("witness reservation refused")
        });
    assert!(matches!(result, Err("witness reservation refused")));
    assert!(!signal.parameter_change_requested.load(Acquire));
    let (request, reserved) = owner
        .parameter_change_with_before(0, 0.5, false, |request| {
            assert!(!signal.parameter_change_requested.load(Acquire));
            Ok(request.delivery_identity().unwrap())
        })
        .unwrap();
    assert_eq!(Some(reserved), refused);
    assert_eq!(reserved.2, 0);
    assert_eq!(Some(reserved), request.delivery_identity());
    for sequence in 0..3 {
        callback(&owner, &mut endpoint, sequence, 2.0, &[]);
    }
    let NativeWorkerParameterProgress::Processed(actual) = terminal(&mut owner, &request) else {
        panic!("actual SDK point must process")
    };
    assert_eq!((actual.point.id, actual.point.normalized), (0, 0.5));
    owner.revoke_on_control();
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn observed_preroll_input_remains_withdrawable_and_stop_drains_the_original_slot() {
    let name = std::thread::current().name().unwrap().to_owned();
    if fixture_child(92, &name) {
        return;
    }
    let instance = original_instance();
    let child = instance.child_id_on_control();
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(instance).unwrap();
    let (request, ()) = owner
        .parameter_change_with_before(0, 0.75, true, |_| Ok(()))
        .unwrap();
    for sequence in 0..3 {
        callback(&owner, &mut endpoint, sequence, 1.0, &[]);
    }
    assert_eq!(
        terminal_pending(&mut owner, &request),
        NativeWorkerParameterProgress::Pending
    );
    assert!(
        owner
            .withdraw_parameter_change_on_control(&request)
            .unwrap()
    );
    assert_eq!(
        terminal(&mut owner, &request),
        NativeWorkerParameterProgress::Withdrawn
    );
    let (stopped, ()) = owner
        .parameter_change_with_before(0, 0.5, true, |_| Ok(()))
        .unwrap();
    for sequence in 3..5 {
        callback(&owner, &mut endpoint, sequence, 1.0, &[]);
    }
    assert_eq!(
        terminal_pending(&mut owner, &stopped),
        NativeWorkerParameterProgress::Pending
    );
    owner.revoke_on_control();
    assert_eq!(
        terminal(&mut owner, &stopped),
        NativeWorkerParameterProgress::Failed
    );
    assert_eq!(
        drain(&mut owner, &stopped),
        NativeWorkerParameterProgress::Withdrawn
    );
    let capture = begin(&mut owner);
    let actual = complete(&mut owner, &capture);
    assert_eq!(
        &hex_plugin_state::decode(&actual)
            .unwrap()
            .component
            .unwrap()[8..12],
        &1.0_f32.to_le_bytes()
    );
    let source = owner.quiesce_on_control().unwrap();
    assert_eq!(source.child_id_on_control(), child);
    assert_eq!(source.completed_blocks(), 5);
    assert!(source.retire_on_control().unwrap().status.success());
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
        HELD_PROCESS_FIXTURE,
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
    if variant == HELD_PROCESS_FIXTURE {
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
    assert_eq!(
        drain(&mut owner, &abandoned),
        NativeWorkerParameterProgress::Withdrawn
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
fn stopped_parameter_drain_distinguishes_queued_and_processed_on_original_source() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::stopped_parameter_drain_distinguishes_queued_and_processed_on_original_source";
    if fixture_child(92, NAME) {
        return;
    }
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let processed = begin_change(&mut owner, 0, 0.75);
    assert!(
        owner
            .drain_stopped_parameter_change_on_control(&processed)
            .is_err()
    );
    let (mut other, _other_endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    other.revoke_on_control();
    assert!(
        other
            .drain_stopped_parameter_change_on_control(&processed)
            .is_err(),
        "equal binding numbers do not transfer a token"
    );
    assert!(other.retire_on_control().unwrap().native.status.success());
    callback(&owner, &mut endpoint, 0, 3.0, &[]);
    owner.revoke_on_control();
    assert_eq!(
        terminal(&mut owner, &processed),
        NativeWorkerParameterProgress::Failed
    );
    assert!(
        owner.quiesce_on_control().is_err(),
        "the pending token retains its source"
    );
    let result = drain(&mut owner, &processed);
    assert!(
        matches!(result, NativeWorkerParameterProgress::Processed(point)
        if point.point.id == 0 && point.point.normalized == 0.75 && point.sequence == 0)
    );
    assert!(
        owner
            .drain_stopped_parameter_change_on_control(&processed)
            .is_err()
    );
    let capture = begin(&mut owner);
    let snapshot = complete(&mut owner, &capture);
    assert_eq!(
        &hex_plugin_state::decode(&snapshot)
            .unwrap()
            .component
            .unwrap()[8..12],
        &3.0_f32.to_le_bytes()
    );
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());

    let (mut owner, _endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let queued = begin_change(&mut owner, 0, 0.5);
    owner.revoke_on_control();
    assert!(
        owner.quiesce_on_control().is_err(),
        "queued input must reach a terminal first"
    );
    assert_eq!(
        drain(&mut owner, &queued),
        NativeWorkerParameterProgress::Withdrawn
    );
    assert!(
        owner
            .drain_stopped_parameter_change_on_control(&queued)
            .is_err()
    );
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn stopped_parameter_drain_observes_the_actual_held_sdk_reply() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::stopped_parameter_drain_observes_the_actual_held_sdk_reply";
    if fixture_child(HELD_PROCESS_FIXTURE, NAME) {
        return;
    }
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    let request = begin_change(&mut owner, 7, 0.75);
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
    owner.revoke_on_control();
    assert_eq!(
        owner
            .drain_stopped_parameter_change_on_control(&request)
            .unwrap(),
        NativeWorkerParameterProgress::Pending
    );
    assert!(
        owner.quiesce_on_control().is_err(),
        "held SDK input retains the original source"
    );
    fs::write(
        PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap()),
        b"release original SDK process",
    )
    .unwrap();
    let result = drain(&mut owner, &request);
    assert!(
        matches!(result, NativeWorkerParameterProgress::Processed(point)
        if point.point.id == 7 && point.point.normalized == 0.75 && point.sequence == 0)
    );
    assert!(
        owner
            .drain_stopped_parameter_change_on_control(&request)
            .is_err()
    );
    let stopped = owner.quiesce_on_control().unwrap();
    assert_eq!(stopped.completed_blocks(), 1);
    assert!(stopped.retire_on_control().unwrap().status.success());
}

fn drain(
    owner: &mut NativeWorker,
    request: &NativeWorkerParameterChange,
) -> NativeWorkerParameterProgress {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let result = owner
            .drain_stopped_parameter_change_on_control(request)
            .unwrap();
        if result != NativeWorkerParameterProgress::Pending {
            return result;
        }
        assert!(Instant::now() < until, "stopped parameter terminal");
        thread::yield_now();
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn stopped_parameter_drain_keeps_an_authored_collision_refused() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::stopped_parameter_drain_keeps_an_authored_collision_refused";
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
    owner.revoke_on_control();
    assert_eq!(
        drain(&mut owner, &request),
        NativeWorkerParameterProgress::Refused
    );
    assert!(
        owner
            .drain_stopped_parameter_change_on_control(&request)
            .is_err()
    );
    let capture = begin(&mut owner);
    let actual = complete(&mut owner, &capture);
    assert_eq!(
        &hex_plugin_state::decode(&actual)
            .unwrap()
            .component
            .unwrap()[8..12],
        &2.0_f32.to_le_bytes()
    );
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_authored_parameter_input_refuses_competing_host_point_without_replacing_audio() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::actual_authored_parameter_input_refuses_competing_host_point_without_replacing_audio";
    if fixture_child(91, NAME) {
        return;
    }
    use hex_plugin_host::transport::{
        AuthoredParameterProgram, AuthoredSegment, SegmentTiming, TimingChanges,
    };
    use std::num::NonZeroU64;

    struct FixtureEnvelope;
    impl AuthoredParameterProgram for FixtureEnvelope {
        fn quantum_program_key(&self) -> NonZeroU64 {
            NonZeroU64::MIN
        }
        fn render_quantum(
            &self,
            _: &[Option<u64>],
            _: &TimingChanges,
            output: &mut Parameters,
        ) -> Result<(), Fault> {
            output.push(ParameterPoint {
                frame: 0,
                id: 7,
                normalized: 0.5,
            })
        }
    }

    let mut instance = original_instance();
    let class = instance.configuration().identity.class;
    let facts = instance
        .read_authored_parameter_metadata_on_control(class, &[7])
        .unwrap();
    instance
        .arm_authored_parameter_metadata_on_control(facts)
        .unwrap();
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(instance).unwrap();
    assert_eq!(owner.received_program_on_control(), None);
    assert_eq!(owner.processed_program_on_control(), None);
    let request = begin_change(&mut owner, 7, 0.75);
    // Force the first quantum to encounter the control mailbox. The native
    // worker must defer the request, while authored audio still progresses.
    let signal = Arc::clone(&owner.reaper.completed);
    let held = signal.capture.lock().unwrap();
    let mut contention = Some(held);
    let until = Instant::now() + Duration::from_secs(3);
    let mut sequence = 0;
    loop {
        let mut output = [[9.0; 2]; 8];
        let stage = || {
            endpoint.begin_physical_callback(8).unwrap();
            endpoint
                .stage_segment_with_authored_parameters(
                    0,
                    &[[0.5, -0.25]; 8],
                    &[],
                    &mut output,
                    SegmentTiming {
                        playing: None,
                        at: |_| None,
                    },
                    AuthoredSegment {
                        program: &FixtureEnvelope,
                        origin: &|frame| Some(frame as u64),
                    },
                )
                .unwrap();
            endpoint.finish(true).unwrap();
        };
        #[cfg(feature = "allocation-probe")]
        {
            let counts = allocation_counter::measure(stage);
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            let mut stage = stage;
            stage();
        }
        while owner.published_blocks_on_control() <= sequence {
            assert!(Instant::now() < until, "authored collision quantum");
            thread::yield_now();
        }
        let expected = ReceivedProgram {
            binding: owner.binding(),
            sequence,
            program: Some(NonZeroU64::MIN),
        };
        assert_eq!(owner.received_program_on_control(), Some(expected));
        assert_eq!(owner.processed_program_on_control(), Some(expected));
        if sequence == 0 {
            assert_eq!(output, [[0.0; 2]; 8]);
            drop(contention.take());
            assert_eq!(
                owner.poll_parameter_change_on_control(&request).unwrap(),
                NativeWorkerParameterProgress::Pending
            );
        } else {
            match owner.poll_parameter_change_on_control(&request).unwrap() {
                NativeWorkerParameterProgress::Refused => break,
                NativeWorkerParameterProgress::Pending => {}
                other => panic!("authored collision terminal: {other:?}"),
            }
        }
        assert!(Instant::now() < until, "authored collision terminal");
        sequence += 1;
    }
    assert!(
        owner
            .begin_state_capture_after_parameter_on_control(&request)
            .is_err()
    );
    for n in sequence + 1..sequence + 4 {
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
    assert_eq!(owner.processed_program_on_control().unwrap().program, None);
    owner.revoke_on_control();
    assert_eq!(owner.received_program_on_control(), None);
    assert_eq!(owner.processed_program_on_control(), None);
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_manual_input_coexists_with_another_authored_parameter() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::actual_manual_input_coexists_with_another_authored_parameter";
    if fixture_child(143, NAME) {
        return;
    }
    use hex_plugin_host::transport::{
        AuthoredParameterProgram, AuthoredSegment, ProcessTiming, SegmentTiming, TimingChanges,
    };
    use std::num::NonZeroU64;

    struct FixtureEnvelope;
    impl AuthoredParameterProgram for FixtureEnvelope {
        fn quantum_program_key(&self) -> NonZeroU64 {
            NonZeroU64::MIN
        }

        fn render_quantum(
            &self,
            origins: &[Option<u64>],
            timing: &TimingChanges,
            output: &mut Parameters,
        ) -> Result<(), Fault> {
            assert_eq!(
                origins,
                &[
                    Some(0),
                    Some(1),
                    Some(2),
                    None,
                    Some(100),
                    Some(101),
                    Some(102),
                    Some(103)
                ]
            );
            let [change] = timing.as_slice() else {
                return Err(Fault::InvalidParameters);
            };
            assert_eq!(change.frame, 4);
            for (frame, normalized) in [(0, 0.25), (2, 0.5), (4, 0.75)] {
                output.push(ParameterPoint {
                    frame,
                    id: 7,
                    normalized,
                })?;
            }
            Ok(())
        }
    }

    let mut instance = original_instance();
    let class = instance.configuration().identity.class;
    assert!(
        instance
            .read_authored_parameter_metadata_on_control(class, &[11])
            .is_err()
    );
    let metadata = instance
        .read_authored_parameter_metadata_on_control(class, &[7])
        .unwrap();
    instance
        .arm_authored_parameter_metadata_on_control(metadata)
        .unwrap();
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(instance).unwrap();
    let request = begin_change(&mut owner, 11, 0.5);
    let input = [[0.5, -0.25]; 8];
    let mut output = [[9.0; 2]; 8];
    let origin = |frame| match frame {
        3 => None,
        0..=2 => Some(frame as u64),
        _ => Some(frame as u64 + 96),
    };
    let stage = || {
        endpoint.begin_physical_callback(8).unwrap();
        endpoint
            .stage_segment_with_authored_parameters(
                0,
                &input,
                &[],
                &mut output,
                SegmentTiming {
                    playing: None,
                    at: |frame| {
                        Some(ProcessTiming {
                            arrangement_frame: origin(frame)? as i64,
                            playing: true,
                            arrangement_beat_raw: None,
                            tempo_micro_bpm: None,
                            meter: None,
                            cycle_beats_raw: None,
                        })
                    },
                },
                AuthoredSegment {
                    program: &FixtureEnvelope,
                    origin: &origin,
                },
            )
            .unwrap();
        endpoint.finish(true).unwrap();
    };
    #[cfg(feature = "allocation-probe")]
    {
        let counts = allocation_counter::measure(stage);
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    {
        let mut stage = stage;
        stage();
    }
    assert_eq!(output, [[0.0; 2]; 8]);
    // Polling the control mailbox before the first worker quantum can itself
    // defer its try-lock input admission. This fixture proves offset zero in
    // that first quantum, so let it publish before consuming the terminal.
    let until = Instant::now() + Duration::from_secs(3);
    while owner.published_blocks_on_control() == 0 {
        assert!(Instant::now() < until, "first mixed parameter quantum");
        thread::yield_now();
    }
    let progress = terminal(&mut owner, &request);
    let NativeWorkerParameterProgress::Processed(receipt) = progress else {
        panic!(
            "manual input did not reach the original PCM worker: {progress:?}, {:?}",
            owner.take_failure_diagnostic_on_control()
        );
    };
    assert_eq!(
        receipt.point,
        ParameterPoint {
            frame: 0,
            id: 11,
            normalized: 0.5
        }
    );
    assert_eq!((receipt.sequence, receipt.first_frame), (0, 0));

    for sequence in 1..=2 {
        endpoint.begin_physical_callback(input.len()).unwrap();
        endpoint
            .stage_segment_with_parameters(0, &input, &[], &[], &mut output)
            .unwrap();
        endpoint.finish(true).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while owner.published_blocks_on_control() <= sequence {
            assert!(Instant::now() < until);
            thread::yield_now();
        }
    }
    let expected_left = [0.5, 0.75, 1.0, 1.0, 1.875, 1.875, 1.875, 1.875];
    for (actual, expected) in output.into_iter().zip(expected_left) {
        assert!(
            (actual[0] - expected).abs() < 1.0e-6,
            "{actual:?} != {expected}"
        );
        assert!(
            (actual[1] + expected / 2.0).abs() < 1.0e-6,
            "{actual:?} != {expected}"
        );
    }
    owner.revoke_on_control();
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_parameter_hold_replaces_one_lane_until_matching_release() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::actual_parameter_hold_replaces_one_lane_until_matching_release";
    if fixture_child(144, NAME) {
        return;
    }
    use hex_plugin_host::transport::{
        AuthoredParameterProgram, AuthoredSegment, SegmentTiming, TimingChanges,
    };
    use std::num::NonZeroU64;

    struct TwoLanes;
    impl AuthoredParameterProgram for TwoLanes {
        fn quantum_program_key(&self) -> NonZeroU64 {
            NonZeroU64::MIN
        }

        fn render_quantum(
            &self,
            _: &[Option<u64>],
            _: &TimingChanges,
            output: &mut Parameters,
        ) -> Result<(), Fault> {
            for (id, normalized) in [(7, 0.25), (11, 0.5)] {
                output.push(ParameterPoint {
                    frame: 0,
                    id,
                    normalized,
                })?;
            }
            Ok(())
        }
    }

    let admit_hold = |owner: &mut NativeWorker, command| {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match owner.parameter_hold_on_control(command) {
                Ok(token) => break token,
                Err("native parameter mailbox is busy") => thread::yield_now(),
                Err(error) => panic!("hold admission: {error}"),
            }
            assert!(Instant::now() < until);
        }
    };
    let mut instance = original_instance();
    let class = instance.configuration().identity.class;
    let metadata = instance
        .read_authored_parameter_metadata_on_control(class, &[7, 11])
        .unwrap();
    instance
        .arm_authored_parameter_metadata_on_control(metadata)
        .unwrap();
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(instance).unwrap();
    let mut render = |owner: &NativeWorker, sequence: u64, expected: f32| {
        let mut output = [[9.0; 2]; 8];
        let mut accepted = None;
        let stage = || {
            endpoint.begin_physical_callback(8).unwrap();
            endpoint
                .stage_segment_with_authored_parameters(
                    0,
                    &[[0.5, -0.25]; 8],
                    &[],
                    &mut output,
                    SegmentTiming {
                        playing: None,
                        at: |_| None,
                    },
                    AuthoredSegment {
                        program: &TwoLanes,
                        origin: &|frame| Some(sequence * 8 + frame as u64),
                    },
                )
                .unwrap();
            assert_eq!(endpoint.take_accepted_manual_parameter(), None);
            endpoint.finish(true).unwrap();
            accepted = endpoint.take_accepted_manual_parameter();
        };
        #[cfg(feature = "allocation-probe")]
        {
            let counts = allocation_counter::measure(stage);
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            let mut stage = stage;
            stage();
        }
        let until = Instant::now() + Duration::from_secs(3);
        while owner.published_blocks_on_control() <= sequence {
            assert!(
                Instant::now() < until,
                "{:?}",
                owner.take_failure_diagnostic_on_control()
            );
            thread::yield_now();
        }
        assert_eq!(output, [[expected, -expected / 2.0]; 8]);
        let expected_marker = match sequence {
            3 => Some((1, Some(0.75))),
            5 => Some((2, Some(0.5))),
            7 => Some((4, None)),
            _ => None,
        };
        assert_eq!(
            accepted.map(|receipt| {
                assert_eq!(receipt.binding, endpoint.binding());
                assert_eq!(receipt.sequence, sequence - 2);
                assert_eq!(receipt.first_frame, (sequence - 2) * 8);
                assert_eq!(receipt.callback_offset, 0);
                assert_eq!(receipt.origin, None);
                assert_eq!(receipt.input.parameter(), 7);
                let value = receipt.input.point().map(|point| {
                    assert_eq!(point.frame, 0);
                    point.normalized
                });
                (receipt.input.operation.get(), value)
            }),
            expected_marker
        );
    };
    let processed =
        |owner: &mut NativeWorker, request: &NativeWorkerParameterChange, sequence, normalized| {
            assert_eq!(
                terminal(owner, request),
                NativeWorkerParameterProgress::Processed(
                    crate::plugin_worker::ProcessedParameterChange {
                        point: ParameterPoint {
                            frame: 0,
                            id: 7,
                            normalized
                        },
                        sequence,
                        first_frame: sequence * 8,
                    }
                ),
            );
        };

    render(&owner, 0, 0.0);
    let hold = admit_hold(
        &mut owner,
        crate::plugin_worker::NativeParameterHold::Begin {
            id: 7,
            normalized: 0.75,
        },
    );
    assert!(
        owner.release_held_parameter_on_control(&hold).is_err(),
        "one mailbox operation"
    );
    render(&owner, 1, 0.0);
    processed(&mut owner, &hold, 1, 0.75);
    render(&owner, 2, 1.0);
    let change = admit_hold(
        &mut owner,
        crate::plugin_worker::NativeParameterHold::Change {
            hold: &hold,
            normalized: 0.5,
        },
    );
    render(&owner, 3, 3.0);
    processed(&mut owner, &change, 3, 0.5);
    let unscoped = begin_change(&mut owner, 7, 0.0);
    render(&owner, 4, 3.0);
    assert_eq!(
        terminal(&mut owner, &unscoped),
        NativeWorkerParameterProgress::Refused
    );
    assert!(owner.release_held_parameter_on_control(&unscoped).is_err());
    let release = admit_hold(
        &mut owner,
        crate::plugin_worker::NativeParameterHold::Release { hold: &hold },
    );
    render(&owner, 5, 2.0);
    assert_eq!(
        terminal(&mut owner, &release),
        NativeWorkerParameterProgress::Released {
            sequence: 5,
            first_frame: 40
        }
    );
    let stale = admit_hold(
        &mut owner,
        crate::plugin_worker::NativeParameterHold::Change {
            hold: &hold,
            normalized: 0.75,
        },
    );
    render(&owner, 6, 2.0);
    assert_eq!(
        terminal(&mut owner, &stale),
        NativeWorkerParameterProgress::Refused
    );
    let stale_release = admit_hold(
        &mut owner,
        crate::plugin_worker::NativeParameterHold::Release { hold: &hold },
    );
    render(&owner, 7, 1.0);
    assert_eq!(
        terminal(&mut owner, &stale_release),
        NativeWorkerParameterProgress::Refused
    );
    render(&owner, 8, 1.0);

    let stopped = admit_hold(
        &mut owner,
        crate::plugin_worker::NativeParameterHold::Begin {
            id: 7,
            normalized: 0.75,
        },
    );
    owner.revoke_on_control();
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner
            .drain_stopped_parameter_change_on_control(&stopped)
            .unwrap()
        {
            NativeWorkerParameterProgress::Pending => assert!(Instant::now() < until),
            NativeWorkerParameterProgress::Withdrawn => break,
            terminal => panic!("stopped hold unexpectedly processed: {terminal:?}"),
        }
        thread::yield_now();
    }
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

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn compiled_saved_vst3_lane_reaches_the_signed_worker_without_callback_allocations() {
    use crate::plugin_effects::{NativeEffect, NativeEffectCallbacks, NativeEffectDefinition};
    use hex_engine::foreign_automation::BoundProgram;
    use hex_lattice::{
        DevicePreparationContext, ForeignProcessorNode, ForeignProcessors, NativeDeviceDefinition,
        NativeInstrumentClock, ProcessorChainOwner, ProcessorInstanceIdentity,
    };
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, Command, Created,
        DeviceAutomationTarget, Editor, Project,
    };

    const NAME: &str = "plugin_worker::capture::tests::parameter_change::compiled_saved_vst3_lane_reaches_the_signed_worker_without_callback_allocations";
    if fixture_child(91, NAME) {
        return;
    }

    struct Clock<'a>(&'a [u64]);
    impl NativeInstrumentClock for Clock<'_> {
        fn beat_at_offset(&self, _: usize) -> Option<u64> {
            None
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            self.0.get(offset).copied()
        }
        fn authored_arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            self.arrangement_frame_at_offset(offset)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(true)
        }
    }

    let instance = original_instance();
    let class = instance.configuration().identity.class;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: instance.ready().initial_state.clone(),
        })
        .unwrap()
    else {
        panic!("expected a vst3 device");
    };
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target: DeviceAutomationTarget::Vst3 { parameter: 7 },
            points: [
                (BeatTime::ZERO, 0.25, AutomationInterpolation::Step),
                (
                    BeatTime::from_ratio(1, 500).unwrap(),
                    0.5,
                    AutomationInterpolation::Linear,
                ),
                (
                    BeatTime::from_ratio(13, 6000).unwrap(),
                    0.75,
                    AutomationInterpolation::Step,
                ),
            ]
            .map(|(beat, value, outgoing)| AutomationPoint {
                beat,
                value: AutomationValue::new(value).unwrap(),
                outgoing,
            })
            .to_vec(),
        })
        .unwrap();
    let path = trace_path().with_extension("hex");
    hex_storage::save_new(&path, editor.project()).unwrap();
    let reopened = hex_storage::load(&path).unwrap();
    let binding = instance.configuration().binding;
    let identity =
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, binding.instance).unwrap();
    let mut effect = NativeEffect::prepare_retained_with_authored_selection_on_control(
        identity,
        instance,
        usize::MAX,
        Some((class, &[7])),
    )
    .unwrap();
    let state = effect.configuration().clone();
    let definition = NativeEffectDefinition::new(std::slice::from_ref(&effect)).unwrap();
    let mut context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &hex_lattice::TuningTable::default(),
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let source = definition
        .foreign_automation_source(identity, &state, &context)
        .unwrap()
        .unwrap();
    assert_eq!(source.binding, binding);
    assert_eq!(
        source.parameters,
        [hex_lattice::ForeignParameterCapability {
            id: 7,
            steps: 0,
            normalized_base_bits: 0.25f64.to_bits()
        }]
    );
    assert!(
        definition
            .foreign_automation_source(
                ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(9), binding.instance)
                    .unwrap(),
                &state,
                &context,
            )
            .is_err()
    );
    context.sample_rate = 44_100;
    assert!(
        definition
            .foreign_automation_source(identity, &state, &context)
            .is_err()
    );
    let program = BoundProgram::prepare(
        reopened.device(device).unwrap().automation(),
        source,
        reopened.tempo_map(),
        2,
    )
    .unwrap()
    .unwrap();
    drop(definition);
    let baseline_revision = effect.reserve_program_revision_on_control().unwrap();
    let definition = NativeEffectDefinition::new(std::slice::from_ref(&effect)).unwrap();
    context.sample_rate = 48_000;
    let base_source = definition
        .foreign_automation_source(identity, &state, &context)
        .unwrap()
        .unwrap();
    assert_eq!(base_source.revision, baseline_revision);
    let baseline = BoundProgram::prepare(&[], base_source, reopened.tempo_map(), 2)
        .unwrap()
        .unwrap();
    let node = ForeignProcessorNode {
        owner: identity.owner(),
        device_id: identity.device_id(),
        device_type: state.type_id(),
        sample_rate: 48_000,
        configuration: &state,
        prepared: effect.prepared(),
        automation: Some(program.borrow()),
        bypassed: false,
        input_gain: 1.0,
        output_gain: 1.0,
    };
    let (mut workers, mut endpoint) =
        NativeEffectCallbacks::from_effects_on_control(vec![effect]).unwrap();
    let owner = &mut workers[0];
    let origins = [45, 46, 47, 48, 49, 0, 1, 2];
    for sequence in 0..8 {
        let node = ForeignProcessorNode {
            automation: Some(if sequence < 4 {
                program.borrow()
            } else {
                baseline.borrow()
            }),
            ..node
        };
        let mut pcm = [[0.5, -0.25]; 8];
        let render = || {
            // Split one actual PCM quantum across two physical callbacks.
            for (start, end) in [(0, 3), (3, 8)] {
                endpoint.begin_host_callback(end - start).unwrap();
                endpoint
                    .process_segment(
                        node,
                        0,
                        &mut pcm[start..end],
                        Some(&Clock(&origins[start..end])),
                    )
                    .unwrap();
                endpoint.finish_host_callback(true).unwrap();
            }
        };
        #[cfg(feature = "allocation-probe")]
        {
            let info = allocation_counter::measure(render);
            assert_eq!(
                (
                    info.count_total,
                    info.bytes_total,
                    info.count_current,
                    info.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            let mut render = render;
            render();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while owner.published_blocks_on_control() <= sequence {
            assert!(
                Instant::now() < deadline,
                "compiled lane did not reach the worker"
            );
            thread::yield_now();
        }
        for (index, &actual) in pcm.iter().enumerate() {
            // The first canonical-base request follows four authored quanta;
            // its actual PCM reaches this endpoint two quanta later.
            let gain = if sequence >= 6 {
                1.0
            } else {
                match index {
                    3 => 2.0,
                    4 => 2.25,
                    _ => 1.0,
                }
            };
            let scale = if sequence < 2 {
                0.0
            } else {
                0.5 + (sequence - 2) as f32 * 0.125
            };
            assert_eq!(
                actual,
                [0.5 * gain * scale, -0.25 * gain * scale],
                "frame {index}"
            );
        }
    }
    // Even the correct state and processor facts cannot lend this program
    // to a different source binding.
    let mut wrong = node.automation.unwrap();
    wrong.binding.epoch += 1;
    let mut refused = [[0.5, -0.25]; 8];
    endpoint.begin_host_callback(refused.len()).unwrap();
    assert_eq!(
        endpoint.process_segment(
            ForeignProcessorNode {
                automation: Some(wrong),
                ..node
            },
            0,
            &mut refused,
            Some(&Clock(&origins)),
        ),
        Err(hex_lattice_contract::DeviceContractError::ForeignProcessingUnavailable)
    );
    assert_eq!(refused, [[0.0; 2]; 8]);
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn unavailable_authored_parameter_refuses_worker_and_retains_original_disposal() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::unavailable_authored_parameter_refuses_worker_and_retains_original_disposal";
    if fixture_child(91, NAME) {
        return;
    }

    use crate::plugin_effects::NativeEffect;
    use hex_lattice::{ProcessorChainOwner, ProcessorInstanceIdentity};
    use hex_plugin_host::native::PreparationDisposal;

    let instance = original_instance();
    let binding = instance.configuration().binding;
    let class = instance.configuration().identity.class;
    let identity =
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, binding.instance).unwrap();
    let mut failure = NativeEffect::prepare_retained_with_authored_selection_on_control(
        identity,
        instance,
        usize::MAX,
        Some((class, &[7, 99])),
    )
    .err()
    .expect("missing parameter must refuse the entire selection");
    assert_eq!(failure.binding(), binding);
    assert!(
        failure
            .to_string()
            .contains("authored parameter is missing")
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(disposal) = failure.poll_disposal_on_control().unwrap() {
            assert!(matches!(disposal, PreparationDisposal::Reaped(_)));
            break;
        }
        assert!(Instant::now() < deadline, "original source disposal");
        thread::yield_now();
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_canonical_parameter_base_survives_the_same_native_lease_epoch() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::actual_canonical_parameter_base_survives_the_same_native_lease_epoch";
    if fixture_child(91, NAME) {
        return;
    }
    let mut instance = original_instance();
    let class = instance.configuration().identity.class;
    let original = instance.configuration().binding;
    let metadata = instance
        .read_authored_parameter_metadata_on_control(class, &[7])
        .unwrap();
    assert_eq!(metadata.initial_normalized(7), Some(0.25));
    instance
        .arm_authored_parameter_metadata_on_control(metadata)
        .unwrap();
    let mut parameters = Parameters::default();
    parameters
        .push(ParameterPoint {
            id: 7,
            frame: 0,
            normalized: 0.75,
        })
        .unwrap();
    let quiescent = instance.quiesce_on_control().unwrap();
    let (quiescent, flushed) = quiescent
        .flush_parameters_retained_on_control(&parameters)
        .unwrap();
    assert_eq!(flushed.as_slice()[0].normalized, 0.75);
    let renewed = Binding {
        graph: original.graph + 1,
        epoch: original.epoch + 1,
        ..original
    };
    let mut instance = quiescent
        .resume_on_control(
            renewed,
            Some(hex_plugin_host::protocol::PresentationLatency {
                input_frames: 77,
                output_frames: 91,
            }),
        )
        .unwrap();
    assert!(!instance.has_armed_authored_parameter(7));
    let metadata = instance
        .read_authored_parameter_metadata_on_control(class, &[7])
        .unwrap();
    assert_eq!(metadata.binding(), renewed);
    assert_eq!(metadata.initial_normalized(7), Some(0.25));
    instance
        .arm_authored_parameter_metadata_on_control(metadata)
        .unwrap();
    assert!(instance.has_armed_authored_parameter(7));
    assert!(
        instance
            .quiesce_on_control()
            .unwrap()
            .retire_on_control()
            .unwrap()
            .status
            .success()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn stopped_idle_parameter_uses_original_raw_zero_source_without_pcm_or_revival() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::stopped_idle_parameter_uses_original_raw_zero_source_without_pcm_or_revival";
    if fixture_child(92, NAME) {
        return;
    }
    let instance = original_instance();
    let child = instance.child_id_on_control();
    let initial = instance.ready().initial_state.clone();
    let (mut owner, mut endpoint) = NativeWorker::start_on_control(instance).unwrap();
    callback(&owner, &mut endpoint, 0, 1.0, &[]);
    owner.revoke_on_control();
    let admit = |owner: &mut NativeWorker, value| {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match owner.begin_parameter_change(0, value, true) {
                Err(
                    "native preparation is busy"
                    | "native capture mailbox is busy"
                    | "native parameter mailbox is busy",
                ) if Instant::now() < until => thread::yield_now(),
                terminal => break terminal,
            }
        }
    };
    let queued = admit(&mut owner, 0.5).unwrap();
    let (mut other, other_endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    other.revoke_on_control();
    assert!(
        other
            .drain_stopped_parameter_change_on_control(&queued)
            .is_err()
    );
    assert!(other.withdraw_parameter_change_on_control(&queued).is_err());
    assert!(other.retire_on_control().unwrap().native.status.success());
    assert!(other.owns_callback(&other_endpoint));
    assert!(queued.delivery_identity().is_none());
    assert!(owner.quiesce_on_control().is_err());
    assert!(!owner.transport_is_live_on_control());
    assert_eq!(
        owner.poll_parameter_change_on_control(&queued).unwrap(),
        NativeWorkerParameterProgress::Failed
    );
    assert_eq!(
        drain(&mut owner, &queued),
        NativeWorkerParameterProgress::Flushed
    );
    assert!(
        owner
            .drain_stopped_parameter_change_on_control(&queued)
            .is_err()
    );
    assert_eq!(owner.published_blocks_on_control(), 1);
    let capture = begin(&mut owner);
    let captured = complete(&mut owner, &capture);
    assert_eq!(
        &hex_plugin_state::decode(&captured)
            .unwrap()
            .component
            .unwrap()[8..12],
        &2.0_f32.to_le_bytes()
    );
    assert!(owner.begin_parameter_change_on_control(0, 0.25).is_err());
    let restoring = admit(&mut owner, 0.25).unwrap();
    assert_eq!(
        drain(&mut owner, &restoring),
        NativeWorkerParameterProgress::Flushed
    );
    let cancelled = admit(&mut owner, 0.75).unwrap();
    assert!(
        owner
            .withdraw_parameter_change_on_control(&cancelled)
            .unwrap()
    );
    assert_eq!(
        drain(&mut owner, &cancelled),
        NativeWorkerParameterProgress::Withdrawn
    );
    let capture = begin(&mut owner);
    let captured = complete(&mut owner, &capture);
    assert_eq!(
        &hex_plugin_state::decode(&captured)
            .unwrap()
            .component
            .unwrap()[8..12],
        &1.0_f32.to_le_bytes()
    );
    let Some(Processor::Native(instance)) = owner
        .reaper
        .worker
        .as_ref()
        .and_then(|state| state.processor.as_ref())
    else {
        panic!("original native source");
    };
    assert_eq!(instance.ready().initial_state, initial);
    assert_eq!(instance.child_id_on_control(), child);
    let quiescent = owner.quiesce_on_control().unwrap();
    assert_eq!(quiescent.completed_blocks(), 1);
    assert!(quiescent.retire_on_control().unwrap().status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn stopped_idle_parameter_refusal_cannot_be_flushed_or_reused_as_success() {
    const NAME: &str = "plugin_worker::capture::tests::parameter_change::stopped_idle_parameter_refusal_cannot_be_flushed_or_reused_as_success";
    if fixture_child(11, NAME) {
        return;
    }
    let (mut owner, endpoint) = NativeWorker::start_on_control(original_instance()).unwrap();
    owner.revoke_on_control();
    let until = Instant::now() + Duration::from_secs(3);
    let request = loop {
        match owner.begin_parameter_change(0x1234, 0.5, true) {
            Err(
                "native preparation is busy"
                | "native capture mailbox is busy"
                | "native parameter mailbox is busy",
            ) if Instant::now() < until => thread::yield_now(),
            terminal => break terminal.unwrap(),
        }
    };
    assert_eq!(
        drain(&mut owner, &request),
        NativeWorkerParameterProgress::Failed
    );
    assert!(owner.begin_parameter_change(0x1234, 0.25, true).is_err());
    assert!(
        owner
            .drain_stopped_parameter_change_on_control(&request)
            .is_err()
    );
    assert!(owner.owns_callback(&endpoint));
    let retired = owner.retire_on_control().unwrap();
    assert!(retired.failure.is_some());
}
