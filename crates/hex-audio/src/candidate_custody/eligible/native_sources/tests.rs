//! Actual Root preparation/return, not endpoint admission or a native app run.
use super::*;
use hex_lattice::ProcessorChainOwner;
use hex_plugin_state::ClassId;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn root() -> Custody {
    super::super::tests::configured(8 * 1024 * 1024)
}
mod completion;
mod definition;
mod effect;

fn input() -> (InstanceBuild, native::Source, native::Preparation) {
    input_for(
        "processing",
        [
            0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
        ],
    )
}
fn input_for(
    fixture: &str,
    class: [u8; 16],
) -> (InstanceBuild, native::Source, native::Preparation) {
    let build = InstanceBuild::inspect(&PathBuf::from(
        std::env::var_os("HEX_VST3_INSTANCE_RECEIPT").unwrap(),
    ))
    .unwrap();
    let fixtures = PathBuf::from(std::env::var_os("HEX_VST3_MACOS_RECEIPT").unwrap());
    let source = native::Source::inspect_on_control(
        &fixtures.parent().unwrap().join(format!(
            "hex-vst3-host.app/Contents/Resources/fixtures/{fixture}.vst3"
        )),
        "Contents/MacOS/fixture",
        ClassId::new(class).unwrap(),
    )
    .unwrap();
    (
        build,
        source,
        native::Preparation {
            // Initial request values are replaced by Root before any helper starts.
            binding: Binding {
                root: 99,
                instance: 99,
                graph: 99,
                epoch: 99,
            },
            sample_rate: 48_000.0,
            quantum: 8,
            input_channels: 2,
            output_channels: 2,
            limits: native::Limits {
                startup: Duration::from_secs(3),
                exchange: Duration::from_secs(1),
                retirement: Duration::from_secs(3),
            },
        },
    )
}
fn identity() -> ProcessorInstanceIdentity {
    ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap()
}
fn start(root: &Custody) -> Binding {
    let (build, source, preparation) = input();
    root.begin_native_source(
        identity(),
        12,
        &build,
        source,
        preparation,
        None,
        native::TrustUserCode::Acknowledged,
    )
    .unwrap()
}
fn dispose(root: &Custody, binding: Binding) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while root.native_source_progress(binding) != Some(NativeSourcePreparationProgress::Complete) {
        root.advance_on_control(false);
        assert!(Instant::now() < deadline, "actual Root native disposal");
        std::thread::yield_now();
    }
}

#[test]
fn native_preparation_cells_require_finite_metadata_reservation() {
    let small = super::super::tests::configured(1024);
    assert!(small.configure_native_sources(1).is_err());
    assert!(!lock(&small.bank).sources.occupied());
    let root = root();
    assert!(root.configure_native_sources(0).is_err());
    assert!(root.configure_native_sources(MAX_SOURCES + 1).is_err());
    let before = root.actual_control_backing_bytes().unwrap();
    root.configure_native_sources(2).unwrap();
    let expected = 2 * (std::mem::size_of::<Slot>() + std::mem::size_of::<Execution>());
    assert_eq!(root.actual_control_backing_bytes(), Some(before + expected));
    assert!(root.configure_native_sources(2).is_err());
    assert!(root.complete());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_retains_pending_native_source_until_disposal_and_exact_report_recovery() {
    let root = root();
    root.configure_native_sources(1).unwrap();
    let metadata = root.actual_control_backing_bytes().unwrap();
    let first = start(&root);
    assert_eq!(
        first,
        Binding {
            root: 31,
            instance: 2,
            graph: 12,
            epoch: 1
        }
    );
    let Some(NativeSourcePreparationProgress::Facts(facts)) = root.native_source_progress(first)
    else {
        panic!("actual Root source facts")
    };
    assert!(facts.presentation_available);
    assert_eq!(facts.algorithmic_latency_frames, 0);
    assert_eq!(
        root.actual_control_backing_bytes(),
        None,
        "unsealed native resources cannot report complete backing"
    );
    {
        let work = root.work.try_lock().unwrap();
        let Some(Owner::Pending(actual)) = work.sources.cells[0].owner.as_ref() else {
            panic!("actual retained source");
        };
        assert_eq!(actual.configuration().binding, first);
    }
    let (build, source, preparation) = input();
    assert!(
        root.begin_native_source_between(
            identity(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
            || panic!("unreserved cancellation pair must not be constructed"),
            || {},
        )
        .is_err()
    );
    assert!(!root.retire_native_source(Binding { root: 32, ..first }));
    assert!(root.retire_native_source(first));
    dispose(&root, first);
    assert!(
        !root.complete(),
        "physical cleanup is not report consumption"
    );
    let returned = root.recover_native_source(first).unwrap();
    assert_eq!(returned.binding, first);
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
    assert!(returned.error.is_none() && returned.cleanup_error.is_none());
    assert!(root.recover_native_source(first).is_none());
    assert_eq!(root.actual_control_backing_bytes(), Some(metadata));
    let second = start(&root);
    assert!(second.epoch > first.epoch);
    assert!(!root.retire_native_source(first));
    root.close();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !root.complete() {
        root.advance_on_control(true);
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(root.recover_native_source(second).is_none());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_close_during_reserved_construction_keeps_custody_without_starting_an_instance() {
    for close_before_pair in [false, true] {
        let root = root();
        root.configure_native_sources(1).unwrap();
        let (build, source, preparation) = input();
        let binding = root
            .begin_native_source_between(
                identity(),
                12,
                &build,
                source,
                preparation,
                None,
                native::TrustUserCode::Acknowledged,
                || {
                    if close_before_pair {
                        root.close();
                    }
                    native::PreparationCancellation::new_on_control()
                },
                || {
                    let binding = lock(&root.bank).sources.slots[0].binding.unwrap();
                    if !close_before_pair {
                        assert_eq!(
                            root.native_source_progress(binding),
                            Some(NativeSourcePreparationProgress::Preparing)
                        );
                    }
                    assert!(
                        root.work.try_lock().is_err(),
                        "Root retains construction execution"
                    );
                    root.close();
                },
            )
            .unwrap();
        dispose(&root, binding);
        let returned = root.recover_native_source(binding).unwrap();
        assert!(matches!(returned.disposal, PreparationDisposal::NoInstance));
        assert!(
            returned
                .error
                .as_ref()
                .is_some_and(|e| e.contains("cancelled"))
        );
        assert!(returned.cleanup_error.is_none());
        assert!(root.complete());
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_refused_native_request_retains_original_failure_through_cleanup() {
    let root = root();
    root.configure_native_sources(1).unwrap();
    let (build, source, mut preparation) = input();
    preparation.quantum = 0;
    let binding = root
        .begin_native_source(
            identity(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
        )
        .unwrap();
    assert_eq!(
        root.native_source_progress(binding),
        Some(NativeSourcePreparationProgress::Retiring)
    );
    assert!(
        root.native_source_error(binding)
            .is_some_and(|error| error.contains("configuration"))
    );
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(matches!(returned.disposal, PreparationDisposal::NoInstance));
    assert!(returned.error.is_some() && returned.cleanup_error.is_none());
    assert!(root.native_source_error(binding).is_none());
    assert!(root.complete());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_native_preparation_stays_in_the_original_outer_root_across_handle_return() {
    crate::lifecycle::prepare_candidate_cleanup().unwrap();
    let original = CandidateRoot::new(2).unwrap();
    original
        .0
        .eligible
        .configure(
            original.root_id(),
            AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            std::mem::size_of::<Inner>(),
        )
        .unwrap();
    original.configure_native_source_preparation(1).unwrap();
    let (build, source, preparation) = input();
    let binding = original
        .begin_native_source_preparation(
            identity(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
        )
        .unwrap();
    assert_eq!(binding.root, original.root_id());
    let retained = original.clone();
    drop(original); // no native owner or completed fact was handed back to the adapter
    assert!(matches!(
        retained.native_source_preparation_progress(binding),
        Some(NativeSourcePreparationProgress::Facts(_))
    ));
    assert!(retained.retire_native_source_preparation(binding));
    // No caller-side tick: accepted retirement must wake the existing lifecycle lane.
    let deadline = Instant::now() + Duration::from_secs(3);
    while retained.native_source_preparation_progress(binding)
        != Some(NativeSourcePreparationProgress::Complete)
    {
        assert!(
            Instant::now() < deadline,
            "lifecycle must observe native disposal"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let returned = retained.recover_native_source_preparation(binding).unwrap();
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
    assert!(retained.0.eligible.complete());

    // A construction refusal must also schedule its retained disposal without
    // relying on a later explicit retirement or a caller-side cleanup tick.
    let (build, source, mut preparation) = input();
    preparation.quantum = 0;
    let refused = retained
        .begin_native_source_preparation(
            identity(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while retained.native_source_preparation_progress(refused)
        != Some(NativeSourcePreparationProgress::Complete)
    {
        assert!(
            Instant::now() < deadline,
            "lifecycle must dispose refused construction"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let returned = retained.recover_native_source_preparation(refused).unwrap();
    assert!(matches!(returned.disposal, PreparationDisposal::NoInstance));
    assert!(returned.error.is_some());

    let (build, source, preparation) = input();
    let abandoned = retained
        .begin_native_source_preparation(
            identity(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
        )
        .unwrap();
    retained
        .complete_native_source_preparation(abandoned, None)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while retained.native_source_preparation_progress(abandoned)
        != Some(NativeSourcePreparationProgress::Complete)
    {
        assert!(
            Instant::now() < deadline,
            "lifecycle must dispose completion refusal"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let returned = retained
        .recover_native_source_preparation(abandoned)
        .unwrap();
    assert!(returned.error.unwrap().contains("presentation"));
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
    let (build, source, preparation) = input();
    let abandoned = retained
        .begin_native_source_preparation(
            identity(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
        )
        .unwrap();
    retained.abandon_control(None, None);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !retained.0.eligible.complete() {
        assert!(
            Instant::now() < deadline,
            "lifecycle must dispose abandoned construction"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        retained
            .recover_native_source_preparation(abandoned)
            .is_none()
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn root_cancellation_descriptor_does_not_outlive_exact_disposal() {
    let root = root();
    root.configure_native_sources(1).unwrap();
    let (build, source, preparation) = input();
    let binding = root
        .begin_native_source(
            identity(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
        )
        .unwrap();
    let descriptor = lock(&root.bank).sources.slots[0]
        .cancellation
        .as_ref()
        .unwrap()
        .descriptor_lifetime_probe_on_control();
    assert!(root.retire_native_source(binding));
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
    assert!(root.complete());
    assert_eq!(
        descriptor.strong_count(),
        0,
        "Root disposal must close its cancellation descriptor"
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn root_cancellation_creation_refusal_is_original_pre_spawn_failure() {
    let root = root();
    root.configure_native_sources(1).unwrap();
    let (build, source, preparation) = input();
    let binding = root
        .begin_native_source_between(
            identity(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
            || {
                Err(hex_plugin_host::Error::Io(std::io::Error::other(
                    "original cancellation pair refusal",
                )))
            },
            || panic!("native preparation must not start after cancellation creation refusal"),
        )
        .unwrap();
    assert_eq!(
        root.native_source_progress(binding),
        Some(NativeSourcePreparationProgress::Complete)
    );
    let returned = root.recover_native_source(binding).unwrap();
    assert!(matches!(returned.disposal, PreparationDisposal::NoInstance));
    assert!(
        returned
            .error
            .unwrap()
            .contains("original cancellation pair refusal")
    );
    assert!(returned.cleanup_error.is_none());
    assert!(root.complete());
}

#[test]
fn native_source_controls_refuse_the_retiring_original_root() {
    let outer = CandidateRoot::new(2).unwrap();
    outer
        .configure_domains(AudioCandidateDomainReservation {
            max_retained_data_bytes: 8 * 1024 * 1024,
            max_construction_peak_data_bytes: 8 * 1024 * 1024,
        })
        .unwrap();
    outer.announce_retirement();
    assert!(
        outer.configure_native_source_preparation(1).is_err(),
        "retiring Root must not create a new native source pool"
    );
    assert!(
        outer
            .with_native_effect_definition::<()>(&[], |_| panic!("retiring Root must not compile"))
            .is_err()
    );
}
