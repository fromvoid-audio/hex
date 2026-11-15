//! Paired original callback/worker/controller observations, not a mock mailbox.
use super::*;
use crate::plugin_worker::{NativeWorkerControllerProgress, NativeWorkerControllerQuery};
use hex_plugin_host::protocol::{
    ParameterConversion, ParameterPageQuery, ParameterPlainValue, ParameterText,
};

mod extension;

fn query(owner: &mut NativeWorker) -> NativeWorkerControllerQuery {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner.begin_controller_query_on_control(ParameterPageQuery::default()) {
            Ok(query) => return query,
            Err(
                "native controller mailbox is busy"
                | "native preparation is busy"
                | "native controller query is already pending",
            ) => {}
            Err(error) => panic!("controller admission: {error}"),
        }
        assert!(Instant::now() < until, "controller admission deadline");
        thread::yield_now();
    }
}
fn page(
    owner: &mut NativeWorker,
    request: &NativeWorkerControllerQuery,
) -> Box<hex_plugin_host::protocol::ParameterPage> {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner.poll_controller_query_on_control(request).unwrap() {
            NativeWorkerControllerProgress::Pending => {}
            NativeWorkerControllerProgress::Page(page) => {
                return page;
            }
            other => panic!("controller completion: {other:?}"),
        }
        assert!(Instant::now() < until, "controller completion deadline");
        thread::yield_now();
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_worker_controller_pages_share_capture_reader_and_preserve_stopped_source() {
    const NAME: &str = "plugin_worker::capture::tests::controller::actual_worker_controller_pages_share_capture_reader_and_preserve_stopped_source";
    if fixture_child(92, NAME) {
        return;
    }
    let instance = original_instance();
    let child = instance.child_id_on_control();
    let (mut owner, mut callback) = NativeWorker::start_on_control(instance).unwrap();
    let (mut other, other_callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    let storage = owner.constructor_storage_on_control(&callback).unwrap();
    let request = query(&mut owner);
    assert!(
        other.poll_controller_query_on_control(&request).is_err(),
        "equal bindings cannot substitute another completion owner"
    );
    other.revoke_on_control();
    let retired = other.retire_on_control().unwrap();
    assert!(retired.failure.is_none() && retired.native.status.success());
    assert!(other.owns_callback(&other_callback));
    assert!(
        owner
            .begin_controller_query_on_control(ParameterPageQuery::default())
            .is_err()
    );
    // Both observations must complete on the same serialized reader while
    // requested receipt reads also compete for its bounded service turns.
    let capture = begin(&mut owner);
    let until = Instant::now() + Duration::from_secs(3);
    let observed = loop {
        let _ = owner.poll_parameter_edits_on_control();
        match owner.poll_controller_query_on_control(&request).unwrap() {
            NativeWorkerControllerProgress::Pending => {}
            NativeWorkerControllerProgress::Page(page) => {
                break page;
            }
            other => panic!("concurrent controller query: {other:?}"),
        }
        assert!(Instant::now() < until, "capture/receipt/query fairness");
        thread::yield_now();
    };
    assert_eq!(observed.as_slice()[0].id, 0);
    assert_eq!(observed.as_slice()[0].normalized, 0.25);
    complete(&mut owner, &capture);
    assert!(owner.poll_controller_query_on_control(&request).is_err());
    let until = Instant::now() + Duration::from_secs(3);
    let conversion = loop {
        match owner.begin_controller_query_on_control(ParameterPageQuery {
            conversion: Some(ParameterConversion::Parse {
                id: 0,
                text: ParameterText::from_text("double").unwrap(),
            }),
            ..ParameterPageQuery::default()
        }) {
            Ok(request) => break request,
            Err("native controller mailbox is busy") => {}
            Err(error) => panic!("SDK parse admission: {error}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    };
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match owner.poll_controller_query_on_control(&conversion).unwrap() {
            NativeWorkerControllerProgress::Pending => {}
            NativeWorkerControllerProgress::Conversion(value) => {
                assert_eq!(
                    (value.id, value.normalized, value.plain),
                    (0, Some(0.5), ParameterPlainValue::Finite(2.0))
                );
                break;
            }
            other => panic!("SDK parse completion: {other:?}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    assert!(owner.poll_controller_query_on_control(&conversion).is_err());
    callback_block(&owner, &mut callback, 0);
    let late = query(&mut owner);
    owner.revoke_on_control();
    assert!(matches!(
        owner.poll_controller_query_on_control(&late).unwrap(),
        NativeWorkerControllerProgress::Failed
    ));
    let stopped = query(&mut owner);
    assert!(owner.poll_controller_query_on_control(&late).is_err());
    let stopped_page = page(&mut owner, &stopped);
    assert_eq!(stopped_page.as_slice(), observed.as_slice());
    assert!(owner.poll_controller_query_on_control(&stopped).is_err());
    assert!(!owner.transport_is_live_on_control());
    assert!(owner.owns_callback(&callback));
    assert_eq!(
        owner.constructor_storage_on_control(&callback),
        Some(storage)
    );
    let quiescent = owner.quiesce_on_control().unwrap();
    assert_eq!(quiescent.child_id_on_control(), child);
    assert_eq!(quiescent.completed_blocks(), 1);
    assert!(quiescent.retire_on_control().unwrap().status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_worker_quiescence_drains_held_controller_getter_before_source_return() {
    const NAME: &str = "plugin_worker::capture::tests::controller::actual_worker_quiescence_drains_held_controller_getter_before_source_return";
    if fixture_child(93, NAME) {
        return;
    }
    let instance = original_instance();
    let child = instance.child_id_on_control();
    let (mut owner, mut callback) = NativeWorker::start_on_control(instance).unwrap();
    let request = query(&mut owner);
    let until = Instant::now() + Duration::from_secs(3);
    while fs::read(trace_path()).ok().is_none_or(|bytes| {
        bytes.len() != 56 || i32::from_le_bytes(bytes[..4].try_into().unwrap()) != 26
    }) {
        assert!(Instant::now() < until, "actual held controller getter");
        thread::sleep(Duration::from_millis(1));
    }
    callback_block(&owner, &mut callback, 0);
    owner.revoke_on_control();
    assert!(matches!(
        owner.poll_controller_query_on_control(&request).unwrap(),
        NativeWorkerControllerProgress::Failed
    ));
    let until = Instant::now() + Duration::from_secs(3);
    while owner
        .reaper
        .join
        .as_ref()
        .is_some_and(|join| !join.is_finished())
    {
        assert!(
            Instant::now() < until,
            "original transport must return before SDK release"
        );
        thread::sleep(Duration::from_millis(1));
    }
    fs::write(
        PathBuf::from(std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap()),
        b"release formatter",
    )
    .unwrap();
    let quiescent = owner
        .into_quiescent_on_control(callback)
        .unwrap_or_else(|failure| panic!("original query drain: {}", failure.finish_on_control()));
    assert_eq!(quiescent.child_id_on_control(), child);
    assert_eq!(quiescent.completed_blocks(), 1);
    assert!(quiescent.retire_on_control().unwrap().status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn retirement_cancels_held_controller_getter_without_normal_stop() {
    const NAME: &str = "plugin_worker::capture::tests::controller::retirement_cancels_held_controller_getter_without_normal_stop";
    if fixture_child(93, NAME) {
        return;
    }
    let (mut owner, _callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    let _request = query(&mut owner);
    let until = Instant::now() + Duration::from_secs(3);
    while fs::read(trace_path()).ok().is_none_or(|bytes| {
        bytes.len() != 56 || i32::from_le_bytes(bytes[..4].try_into().unwrap()) != 26
    }) {
        assert!(Instant::now() < until, "actual held controller getter");
        thread::sleep(Duration::from_millis(1));
    }
    owner.revoke_on_control();
    let retired = owner.retire_on_control().unwrap();
    assert!(matches!(
        retired.failure,
        Some(hex_plugin_host::Error::Cancelled)
    ));
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn editor_seal_requires_stopped_original_worker_and_exact_completion() {
    use hex_plugin_host::protocol::{ControllerRequest, EditorSealState};
    const NAME: &str = "plugin_worker::capture::tests::controller::editor_seal_requires_stopped_original_worker_and_exact_completion";
    if fixture_child(92, NAME) {
        return;
    }
    let (mut owner, _callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    let (mut other, _other_callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    assert!(matches!(
        owner.begin_controller_query_on_control(ControllerRequest::SealEditorEdits),
        Err("native editor seal requires a stopped source")
    ));
    owner.revoke_on_control();
    let until = Instant::now() + Duration::from_millis(750);
    let request = loop {
        match owner.begin_controller_query_on_control(ControllerRequest::SealEditorEdits) {
            Ok(request) => break request,
            Err("native preparation is busy" | "native controller mailbox is busy") => {}
            Err(reason) => panic!("stopped seal admission: {reason}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    };
    assert!(other.poll_controller_query_on_control(&request).is_err());
    loop {
        match owner.poll_controller_query_on_control(&request).unwrap() {
            NativeWorkerControllerProgress::Pending => {}
            NativeWorkerControllerProgress::EditorSeal(EditorSealState::Sealed) => break,
            other => panic!("stopped seal terminal: {other:?}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    assert!(owner.poll_controller_query_on_control(&request).is_err());
    assert!(owner.retire_on_control().unwrap().native.status.success());
    other.revoke_on_control();
    assert!(other.retire_on_control().unwrap().native.status.success());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn sealed_stopped_worker_drains_original_editor_burst_in_order() {
    use hex_plugin_host::protocol::{ControllerRequest, EditorSealState, ParameterEditKind};
    const NAME: &str = "plugin_worker::capture::tests::controller::sealed_stopped_worker_drains_original_editor_burst_in_order";
    if fixture_child(116, NAME) {
        return;
    }
    let (mut owner, _callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    assert!(owner.poll_sealed_edits_on_control().is_err());
    let (mut other, _other_callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    other.revoke_on_control();
    assert!(other.poll_sealed_edits_on_control().is_err());
    fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"emit",
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while fs::read(trace_path())
        .ok()
        .is_none_or(|trace| trace.get(..4) != Some(&25_i32.to_le_bytes()))
    {
        assert!(Instant::now() < until, "SDK burst did not finish");
        thread::yield_now();
    }
    owner.revoke_on_control();
    assert!(owner.poll_sealed_edits_on_control().is_err());
    let request = loop {
        match owner.begin_controller_query_on_control(ControllerRequest::SealEditorEdits) {
            Ok(request) => break request,
            Err("native preparation is busy" | "native controller mailbox is busy") => {}
            Err(reason) => panic!("seal admission: {reason}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    };
    loop {
        match owner.poll_controller_query_on_control(&request).unwrap() {
            NativeWorkerControllerProgress::Pending => {}
            NativeWorkerControllerProgress::EditorSeal(EditorSealState::Sealed) => break,
            other => panic!("seal terminal: {other:?}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    let controller = query(&mut owner);
    assert!(matches!(
        owner.poll_sealed_edits_on_control(),
        Err("native stopped control operation is busy")
    ));
    let _ = page(&mut owner, &controller);
    let mut edits = Vec::new();
    while edits.len() < 14 {
        let batch = owner.poll_sealed_edits_on_control().unwrap();
        edits.extend_from_slice(batch.edits.as_slice());
        assert!(Instant::now() < until, "sealed burst did not drain");
    }
    assert_eq!(edits.len(), 14);
    for (index, edit) in edits.iter().enumerate() {
        assert_eq!(
            (edit.sequence, edit.gesture, edit.id),
            (index as u64 + 1, 1, 7)
        );
    }
    assert_eq!(edits.first().unwrap().kind, ParameterEditKind::Begin);
    assert_eq!(edits.last().unwrap().kind, ParameterEditKind::End);
    assert!(
        owner
            .poll_sealed_edits_on_control()
            .unwrap()
            .edits
            .as_slice()
            .is_empty()
    );
    assert_eq!(owner.published_blocks_on_control(), 0);
    assert!(owner.retire_on_control().unwrap().failure.is_none());
    assert!(other.retire_on_control().unwrap().failure.is_none());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn sealed_worker_delivers_admitted_read_before_fresh_batches() {
    use crate::plugin_worker::StoppedEditProgress;
    use hex_plugin_host::protocol::{ControllerRequest, EditorSealState};
    const NAME: &str = "plugin_worker::capture::tests::controller::sealed_worker_delivers_admitted_read_before_fresh_batches";
    if fixture_child(116, NAME) {
        return;
    }
    let (mut owner, _callback) = NativeWorker::start_on_control(original_instance()).unwrap();
    fs::write(
        std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
        b"emit",
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while fs::read(trace_path())
        .ok()
        .is_none_or(|trace| trace.get(..4) != Some(&25_i32.to_le_bytes()))
    {
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    assert!(
        owner
            .poll_recovery_edits_on_control(true)
            .unwrap()
            .is_none()
    );
    owner.revoke_on_control();
    let request = loop {
        match owner.begin_controller_query_on_control(ControllerRequest::SealEditorEdits) {
            Ok(request) => break request,
            Err("native preparation is busy" | "native controller mailbox is busy") => {}
            Err(reason) => panic!("seal admission: {reason}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    };
    loop {
        match owner.poll_controller_query_on_control(&request).unwrap() {
            NativeWorkerControllerProgress::Pending => {}
            NativeWorkerControllerProgress::EditorSeal(EditorSealState::Sealed) => break,
            other => panic!("seal terminal: {other:?}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    }
    assert!(matches!(
        owner.poll_sealed_edits_on_control(),
        Err("native prior edit read remains")
    ));
    let first = loop {
        match owner.poll_stopped_edits_on_control(true) {
            Ok(StoppedEditProgress::Completed(batch)) => break batch,
            Ok(StoppedEditProgress::Pending) | Err("native edit observation is busy") => {}
            Err(reason) => panic!("prior read: {reason}"),
        }
        assert!(Instant::now() < until);
        thread::yield_now();
    };
    let mut edits = first.edits.as_slice().to_vec();
    while edits.len() < 14 {
        edits.extend_from_slice(
            owner
                .poll_sealed_edits_on_control()
                .unwrap()
                .edits
                .as_slice(),
        );
        assert!(Instant::now() < until);
    }
    assert_eq!(edits.len(), 14);
    assert!(
        edits
            .iter()
            .enumerate()
            .all(|(index, edit)| edit.sequence == index as u64 + 1)
    );
    owner.reaper.worker.as_mut().unwrap().native_error = Some(hex_plugin_host::Error::Protocol(
        "injected failure before sealed drain",
    ));
    assert!(matches!(
        owner.poll_sealed_edits_on_control(),
        Err("native stopped edit source failed")
    ));
    assert!(owner.retire_on_control().unwrap().failure.is_some());
}
