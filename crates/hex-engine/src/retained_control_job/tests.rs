use super::*;
use crate::{AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation};
#[path = "../../build_support/retained_job_box_gate.rs"]
mod job_gate;
use crate::pcm_arc::build_gate_tests as pcm_gate;
fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 16,
        handle_rows: 16,
        root_rows: 4,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}
fn pending<T>(_: &mut T) -> Result<RetainedControlJobProgress, ()> {
    Ok(RetainedControlJobProgress::Pending)
}
#[repr(align(64))]
struct Aligned([u8; 3]);
#[test]
fn retained_job_fresh_actual_overaligned_body_and_independent_jobs_have_exact_limits() {
    let expected = Layout::new::<TypedJob<Aligned, ()>>();
    let quote = RetainedControlJob::body_reservation_quote::<Aligned, ()>();
    assert_eq!(quote, expected.size());
    let mut source = Some(Aligned([1, 2, 3]));
    assert!(matches!(
        RetainedControlJob::try_from_owned_on_control(&mut source, quote - 1, pending),
        Err(RetainedControlJobBuildError::Reservation)
    ));
    assert_eq!(source.as_ref().unwrap().0, [1, 2, 3]);
    let first = RetainedControlJob::try_from_owned_on_control(&mut source, quote, pending).unwrap();
    assert!(source.is_none());
    let mut source = Some(Aligned([1, 2, 3]));
    let second =
        RetainedControlJob::try_from_owned_on_control(&mut source, quote, pending).unwrap();
    assert_eq!(first.actual_body_layout_on_control(), expected);
    assert_eq!(first.actual_body_layout_on_control().align(), 64);
    assert_eq!(second.actual_body_layout_on_control(), expected);
    let mut unknown = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    assert_eq!(
        first.visit_outer_with_gate(&mut unknown, false),
        Err(AudioStorageError::UnknownStorage)
    );
    assert!(unknown.summary().is_err());
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    if !retained_control_job_box_layout_supported() {
        assert_eq!(
            first.visit_outer_storage_on_control(&mut ledger),
            Err(AudioStorageError::UnknownStorage)
        );
        eprintln!(
            "unsupported retained-job native constructor; actual outer-body qualification refused"
        );
        return;
    }
    eprintln!("SUPPORTED retained-job fresh actual over-aligned Box layout and independent bodies");
    first.visit_outer_storage_on_control(&mut ledger).unwrap();
    assert_eq!(ledger.summary().unwrap().data_bytes, expected.size());
    first.visit_outer_storage_on_control(&mut ledger).unwrap();
    assert_eq!(ledger.summary().unwrap().data_allocations, 1);
    second.visit_outer_storage_on_control(&mut ledger).unwrap();
    assert_eq!(ledger.summary().unwrap().data_allocations, 2);
    assert_eq!(ledger.summary().unwrap().data_bytes, 2 * expected.size());
    let observed = ledger.summary().unwrap();
    let mut exact = reservation();
    exact.max_retained_bytes = observed.retained_bytes;
    exact.max_construction_peak_bytes = observed.construction_peak_bytes;
    let mut exact_ledger = AudioRootStorageLedger::try_new_on_control(exact).unwrap();
    first
        .visit_outer_storage_on_control(&mut exact_ledger)
        .unwrap();
    second
        .visit_outer_storage_on_control(&mut exact_ledger)
        .unwrap();
    drop(exact_ledger);
    exact.max_retained_bytes -= 1;
    let mut below = AudioRootStorageLedger::try_new_on_control(exact).unwrap();
    first.visit_outer_storage_on_control(&mut below).unwrap();
    assert!(second.visit_outer_storage_on_control(&mut below).is_err());
}
#[test]
fn retained_job_outer_body_keeps_vector_backing_separate_and_raw_arc_child_unknown() {
    let mut bytes = Vec::with_capacity(97);
    bytes.extend_from_slice(&[3_u8, 4, 5]);
    let mut source = Some(bytes);
    let job = RetainedControlJob::try_from_owned_on_control(&mut source, 4096, pending).unwrap();
    if !retained_control_job_box_layout_supported() {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        assert!(job.visit_outer_storage_on_control(&mut ledger).is_err());
        eprintln!(
            "unsupported retained-job native constructor; child oracle qualification refused"
        );
        return;
    }
    eprintln!("SUPPORTED retained-job separate actual child backing and raw Arc refusal");
    job.inspect_payload::<Vec<u8>, _>(|child| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        job.visit_outer_storage_on_control(&mut ledger).unwrap();
        assert_eq!(
            ledger.summary().unwrap().data_bytes,
            job.actual_body_layout_on_control().size()
        );
        assert!(ledger.summary().unwrap().require_complete().is_err());
        ledger
            .borrow_metadata_vec_backing_on_control(child)
            .unwrap();
        assert_eq!(
            ledger.summary().unwrap().data_bytes,
            job.actual_body_layout_on_control().size() + child.capacity()
        );
        assert_eq!(ledger.summary().unwrap().data_allocations, 2);
        assert!(
            ledger.summary().unwrap().require_complete().is_err(),
            "child storage is not retirement/admission proof"
        );
    })
    .unwrap();
    let mut raw = Some(std::sync::Arc::new(vec![1_u8, 2, 3]));
    let job = RetainedControlJob::try_from_owned_on_control(&mut raw, 4096, pending).unwrap();
    job.inspect_payload::<std::sync::Arc<Vec<u8>>, _>(|child| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        job.visit_outer_storage_on_control(&mut ledger).unwrap();
        assert_eq!(
            ledger.mutate(|ledger| ledger.shared(
                child,
                None,
                crate::audio_storage::Owner::exclusive(child),
                crate::audio_only_inputs::handle()?,
                None
            )),
            Err(AudioStorageError::UnknownStorage)
        );
        assert!(ledger.summary().is_err());
    })
    .unwrap();
}
#[test]
fn retained_job_selector_has_separate_family_and_refuses_every_unsupported_observation() {
    assert_eq!(
        job_gate::FAMILY,
        "fresh-retained-control-job-sized-box-new-v1"
    );
    for target in ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"] {
        let metadata = format!(
            "release: 1.94.0\ncommit-hash: {}\nhost: {target}\n",
            pcm_gate::COMMIT
        );
        let observed = pcm_gate::Observation {
            verbose: &metadata,
            host: target,
            target,
            ordinary_install: true,
            unsupported_inputs: false,
        };
        assert!(job_gate::decide(observed).is_ok());
        assert!(
            job_gate::decide(pcm_gate::Observation {
                unsupported_inputs: true,
                ..observed
            })
            .is_err()
        );
        assert!(
            job_gate::decide(pcm_gate::Observation {
                ordinary_install: false,
                ..observed
            })
            .is_err()
        );
        assert!(
            job_gate::decide(pcm_gate::Observation {
                host: "other",
                ..observed
            })
            .is_err()
        );
        assert!(
            job_gate::decide(pcm_gate::Observation {
                target: "other",
                ..observed
            })
            .is_err()
        );
        assert!(
            job_gate::decide(pcm_gate::Observation {
                verbose: "unqualified",
                ..observed
            })
            .is_err()
        );
    }
}

#[test]
fn job_route_is_fresh_move_stable_and_identity_exhaustion_preserves_source() {
    use std::sync::atomic::AtomicU64;
    let mut source = Some([1_u8; 7]);
    let exhausted = AtomicU64::new(u64::MAX);
    assert!(matches!(
        RetainedControlJob::try_from_counter_on_control(
            &mut source,
            usize::MAX,
            pending,
            &exhausted,
        ),
        Err(RetainedControlJobBuildError::IdentityExhausted)
    ));
    assert_eq!(source, Some([1_u8; 7]));
    let first =
        RetainedControlJob::try_from_owned_on_control(&mut source, usize::MAX, pending).unwrap();
    let route = first.route();
    let second =
        RetainedControlJob::try_from_owned_on_control(&mut Some([1_u8; 7]), usize::MAX, pending)
            .unwrap();
    assert_ne!(route, second.route());
    let moved = first;
    assert_eq!(route, moved.route());
    let hint = RetainedControlJobRouteHint::new();
    assert!(!hint.matches(route));
    hint.publish(route);
    assert!(hint.matches(route));
    assert!(!hint.matches(second.route()));
    hint.clear();
    assert!(!hint.matches(route));
    assert_eq!(
        moved.actual_body_layout_on_control(),
        Layout::new::<TypedJob<[u8; 7], ()>>()
    );
}
