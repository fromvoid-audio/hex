use super::*;
use crate::sampler_host_fields::SpectrumFloatBoxRef;
struct Core {
    buffers: [Box<[f64; 4]>; 8],
}
struct ProjectCore {
    buffers: [Box<[f64; 4]>; 6],
    envelopes: Box<[Option<[f32; 96]>; 2]>,
}
struct Capture {
    core: Option<Core>,
    project: Option<ProjectCore>,
}
fn capture() -> Capture {
    Capture {
        core: Some(Core {
            buffers: std::array::from_fn(|_| Box::new([0.; 4])),
        }),
        project: Some(ProjectCore {
            buffers: std::array::from_fn(|_| Box::new([0.; 4])),
            envelopes: Box::new([None; 2]),
        }),
    }
}
fn fields(value: &Capture) -> NativeSpectrumCaptureFields<'_> {
    let core = value.core.as_ref().unwrap();
    let project = value.project.as_ref().unwrap();
    NativeSpectrumCaptureFields {
        body: BodyRef::of(value),
        core: NativeSpectrumCoreFields {
            body: BodyRef::of(core),
            buffers: core.buffers.each_ref().map(SpectrumFloatBoxRef::fft_values),
        },
        project: NativeSpectrumProjectCoreFields {
            body: BodyRef::of(project),
            buffers: project
                .buffers
                .each_ref()
                .map(SpectrumFloatBoxRef::fft_values),
            envelope_field: BodyRef::of(&project.envelopes),
            envelopes: BodyRef::of(project.envelopes.as_ref()),
        },
    }
}
fn ledger() -> AudioRootStorageLedger<'static> {
    AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        // Two actual records each retain metadata + frame + fifteen buffers.
        data_rows: 34,
        handle_rows: 8,
        root_rows: 2,
        max_retained_bytes: 1 << 20,
        max_construction_peak_bytes: 1 << 20,
    })
    .unwrap()
}
#[test]
fn normal_buffers_leave_frame_until_matching_join_and_metadata_alias_closure() {
    let mut value = capture();
    let (registration, mut capture, mut join) =
        NativeSpectrumCapturedRegistration::pair_actual_capture_on_control(fields(&value)).unwrap();
    let mut observed = ledger();
    registration
        .visit_storage_on_control(&mut observed)
        .unwrap();
    observed
        .validate_selected_shared_handles_on_control()
        .unwrap();
    let full = observed.summary().unwrap().data_bytes;
    drop(observed);
    capture
        .begin_actual_buffers_drop_on_control(fields(&value))
        .unwrap();
    drop(value.core.take());
    drop(value.project.take());
    capture.complete_actual_buffers_drop_on_control().unwrap();
    drop(capture);
    let mut observed = ledger();
    registration
        .visit_storage_on_control(&mut observed)
        .unwrap();
    observed
        .validate_selected_shared_handles_on_control()
        .unwrap();
    let frame = observed.summary().unwrap().data_bytes;
    assert_eq!(
        full - frame,
        14 * size_of::<[f64; 4]>() + size_of::<[Option<[f32; 96]>; 2]>()
    );
    assert!(!registration.is_normally_disposed_on_control());
    drop(observed);
    registration
        .finalize_actual_join_on_control(&mut join)
        .unwrap();
    drop(join);
    let mut observed = ledger();
    registration
        .visit_storage_on_control(&mut observed)
        .unwrap();
    observed
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert_eq!(
        frame - observed.summary().unwrap().data_bytes,
        size_of::<Capture>()
    );
}
#[test]
fn foreign_counterpart_refuses_without_changing_real_pair() {
    let a = capture();
    let b = capture();
    let (ra, ca, mut ja) =
        NativeSpectrumCapturedRegistration::pair_actual_capture_on_control(fields(&a)).unwrap();
    let (rb, cb, mut jb) =
        NativeSpectrumCapturedRegistration::pair_actual_capture_on_control(fields(&b)).unwrap();
    assert_eq!(
        ra.finalize_actual_join_on_control(&mut jb),
        Err(AudioStorageError::SourceMismatch)
    );
    assert_eq!(
        rb.finalize_failed_spawn_on_control(&mut ja),
        Err(AudioStorageError::SourceMismatch)
    );
    let mut observed = ledger();
    ra.visit_storage_on_control(&mut observed).unwrap();
    rb.visit_storage_on_control(&mut observed).unwrap();
    observed
        .validate_selected_shared_handles_on_control()
        .unwrap();
    drop(observed);
    drop((ca, cb, ja, jb));
    assert_eq!(
        ra.visit_storage_on_control(&mut ledger()),
        Err(AudioStorageError::UnknownStorage)
    );
}
#[test]
fn unexplained_metadata_alias_and_consumed_pending_are_sticky_failure() {
    let mut value = capture();
    let (record, mut capture, mut join) =
        NativeSpectrumCapturedRegistration::pair_actual_capture_on_control(fields(&value)).unwrap();
    capture
        .begin_actual_buffers_drop_on_control(fields(&value))
        .unwrap();
    drop(value.core.take());
    drop(value.project.take());
    capture.complete_actual_buffers_drop_on_control().unwrap();
    drop(capture);
    let extra = record.state.clone();
    assert_eq!(
        record.finalize_actual_join_on_control(&mut join),
        Err(AudioStorageError::UnknownStorage)
    );
    drop(extra);
    assert_eq!(
        record.finalize_actual_join_on_control(&mut join),
        Err(AudioStorageError::UnknownStorage)
    );
    assert_eq!(
        record.visit_storage_on_control(&mut ledger()),
        Err(AudioStorageError::UnknownStorage)
    );
    let value = super::tests::capture();
    let (record, capture, join) =
        NativeSpectrumCapturedRegistration::pair_actual_capture_on_control(fields(&value)).unwrap();
    capture
        .begin_actual_buffers_drop_on_control(fields(&value))
        .unwrap();
    drop(capture);
    assert_eq!(
        record.visit_storage_on_control(&mut ledger()),
        Err(AudioStorageError::UnknownStorage)
    );
    drop(join);
}

#[test]
fn two_actual_captured_records_refuse_33_rows_and_bind_34_without_parent_mutation() {
    let a = capture();
    let b = capture();
    let (ra, _ca, _ja) =
        NativeSpectrumCapturedRegistration::pair_actual_capture_on_control(fields(&a)).unwrap();
    let (rb, _cb, _jb) =
        NativeSpectrumCapturedRegistration::pair_actual_capture_on_control(fields(&b)).unwrap();
    for rows in [33, 34] {
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                data_rows: rows,
                handle_rows: 8,
                root_rows: 2,
                max_retained_bytes: 1 << 20,
                max_construction_peak_bytes: 1 << 20,
            })
            .unwrap();
        ra.visit_storage_on_control(&mut ledger).unwrap();
        let result = rb.visit_storage_on_control(&mut ledger);
        if rows == 33 {
            assert_eq!(result, Err(AudioStorageError::Capacity));
        } else {
            result.unwrap();
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
        }
    }
}
