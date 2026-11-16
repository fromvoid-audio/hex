//! Actual Root preparation/return, not endpoint admission or a native app run.
use super::*;
use hex_lattice::ProcessorChainOwner;
use hex_plugin_state::ClassId;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub(super) fn root() -> Custody {
    super::super::tests::configured(64 * 1024 * 1024)
}
pub(crate) fn source_reservation() -> NativeSourceDataReservation {
    NativeSourceDataReservation {
        max_retained_data_bytes: 4 * 1024 * 1024,
        max_construction_peak_data_bytes: 24 * 1024 * 1024,
    }
}
mod completion;
mod definition;
mod effect;
mod epoch;

pub(crate) fn input() -> (InstanceBuild, native::Source, native::Preparation) {
    input_for(
        "processing",
        [
            0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
        ],
    )
}
pub(crate) fn input_for(
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
                // This isolated held-worker driver observes the callback fault
                // before cleanup. Shorten only its eventual native I/O timeout;
                // it still requires the unresponsive child to be reaped.
                exchange: if std::env::var_os("HEX_AUDIO_HELD_WORKER_DRIVER").is_some() {
                    Duration::from_millis(400)
                } else {
                    Duration::from_secs(1)
                },
                retirement: Duration::from_secs(3),
            },
        },
    )
}
pub(crate) fn identity() -> ProcessorInstanceIdentity {
    ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap()
}
pub(super) fn start(root: &Custody) -> Binding {
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
pub(super) fn dispose(root: &Custody, binding: Binding) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while root.native_source_progress(binding) != Some(NativeSourcePreparationProgress::Complete) {
        root.advance_on_control(false);
        assert!(Instant::now() < deadline, "actual Root native disposal");
        std::thread::yield_now();
    }
}

#[test]
fn native_preparation_cells_require_finite_pool_reservations() {
    let small = super::super::tests::configured(1024);
    assert!(
        small
            .configure_native_sources(1, source_reservation())
            .is_err()
    );
    assert!(!lock(&small.bank).sources.occupied());
    let root = root();
    assert!(
        root.configure_native_sources(0, source_reservation())
            .is_err()
    );
    assert!(
        root.configure_native_sources(MAX_SOURCES + 1, source_reservation())
            .is_err()
    );
    let before = root.actual_control_backing_bytes().unwrap();
    root.configure_native_sources(2, source_reservation())
        .unwrap();
    let expected = 2 * (std::mem::size_of::<Slot>() + std::mem::size_of::<Execution>());
    assert_eq!(root.actual_control_backing_bytes(), Some(before + expected));
    let usage = root.usage().unwrap();
    assert_eq!(
        usage.retained_data_reservation,
        before + expected + 2 * source_reservation().max_retained_data_bytes
    );
    assert_eq!(
        usage.construction_peak_data_reservation,
        before + expected + 2 * source_reservation().max_construction_peak_data_bytes
    );
    assert!(
        root.configure_native_sources(2, source_reservation())
            .is_err()
    );
    assert!(root.complete());
}

#[test]
fn native_pool_refuses_invalid_and_overflowing_allowances_without_installing_cells() {
    let root = root();
    let before = root.usage();
    for limits in [
        NativeSourceDataReservation {
            max_retained_data_bytes: 0,
            ..source_reservation()
        },
        NativeSourceDataReservation {
            max_construction_peak_data_bytes: 1,
            ..source_reservation()
        },
        NativeSourceDataReservation {
            max_retained_data_bytes: usize::MAX,
            max_construction_peak_data_bytes: usize::MAX,
        },
        NativeSourceDataReservation {
            max_construction_peak_data_bytes: usize::MAX,
            ..source_reservation()
        },
    ] {
        assert!(root.configure_native_sources(2, limits).is_err());
        assert!(lock(&root.bank).sources.slots.is_empty());
        assert_eq!(root.usage(), before);
    }
}

#[test]
fn vacant_native_pool_peak_reservation_blocks_unrelated_domain_admission() {
    let baseline = root();
    let admitted = baseline
        .register_domain(super::super::tests::pair())
        .ok()
        .expect("unreserved baseline must admit the same stream constructor");
    assert!(baseline.retire_domain(admitted));
    for _ in 0..4 {
        baseline.advance_on_control(false);
    }
    assert!(baseline.recover_domain(admitted).is_some());
    let root = root();
    let metadata = root.actual_control_backing_bytes().unwrap();
    let cells = std::mem::size_of::<Slot>() + std::mem::size_of::<Execution>();
    let limits = NativeSourceDataReservation {
        max_construction_peak_data_bytes: 64 * 1024 * 1024 - metadata - cells - 1,
        ..source_reservation()
    };
    root.configure_native_sources(1, limits).unwrap();
    let held = root.usage();
    let domain = super::super::tests::pair();
    let refusal = root.register_domain(domain).unwrap_err();
    let original = refusal.domain.0.catalog_lease().unwrap();
    assert!(refusal.domain.1.matches_catalog(&original));
    drop(original);
    assert!(
        refusal
            .domain
            .0
            .shutdown(refusal.domain.1)
            .unwrap()
            .accounting_complete
    );
    assert_eq!(root.usage(), held);
    assert_eq!(root.usage().unwrap().occupied_domains, 0);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_input_over_native_allowance_refuses_before_cancellation_or_child_construction() {
    let root = root();
    root.configure_native_sources(
        1,
        NativeSourceDataReservation {
            max_retained_data_bytes: 1,
            max_construction_peak_data_bytes: hex_plugin_scan::MAX_BINARY as usize + 1,
        },
    )
    .unwrap();
    let before = root.usage();
    let (build, source, preparation) = input();
    let result = root.begin_native_source_between(
        identity(),
        12,
        &build,
        source,
        preparation,
        None,
        native::TrustUserCode::Acknowledged,
        || panic!("over-budget input must not construct a cancellation pair"),
        || panic!("over-budget input must not reserve a source"),
    );
    assert_eq!(
        result.unwrap_err(),
        "native source input exceeds retained data reservation"
    );
    assert!(!lock(&root.bank).sources.occupied());
    assert_eq!(lock(&root.bank).sources.next_epoch, 1);
    assert_eq!(root.usage(), before);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_preparation_retained_bound_is_checked_before_reservation_or_child() {
    let (build, source, preparation) = input();
    let bound = source
        .preparation_storage_bound_on_control(&build, None)
        .unwrap();
    assert_eq!(
        bound.diagnostic_data_bytes,
        hex_plugin_host::NATIVE_DIAGNOSTIC_DATA_BYTES
    );
    assert!(bound.diagnostic_data_bytes > 0);
    // All former retained DATA fits; only the actual diagnostic allowance
    // distinguishes this candidate from an admissible pre-start request.
    let limit = bound.max_retained_data_bytes() - bound.diagnostic_data_bytes;
    assert!(bound.input_data_bytes <= limit, "request input alone fits");
    let root = root();
    root.configure_native_sources(
        1,
        NativeSourceDataReservation {
            max_retained_data_bytes: limit,
            max_construction_peak_data_bytes: bound.base_peak_data_bytes().unwrap(),
        },
    )
    .unwrap();
    let before = root.usage();
    let result = root.begin_native_source_between(
        identity(),
        12,
        &build,
        source,
        preparation,
        None,
        native::TrustUserCode::Acknowledged,
        || panic!("preparation over budget must not construct cancellation or child"),
        || panic!("preparation over budget must not reserve a slot"),
    );
    assert_eq!(
        result.unwrap_err(),
        "native preparation exceeds retained data reservation"
    );
    assert_eq!(root.usage(), before);
    let bank = lock(&root.bank);
    assert!(!bank.sources.occupied());
    assert_eq!(bank.sources.next_epoch, 1);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_protocol_peak_is_reserved_before_cancellation_or_child() {
    let (build, source, preparation) = input();
    let bound = source
        .preparation_storage_bound_on_control(&build, None)
        .unwrap();
    let root = root();
    root.configure_native_sources(
        1,
        NativeSourceDataReservation {
            max_retained_data_bytes: bound.max_retained_data_bytes(),
            max_construction_peak_data_bytes: bound.base_peak_data_bytes().unwrap() - 1,
        },
    )
    .unwrap();
    let before = root.usage();
    let result = root.begin_native_source_between(
        identity(),
        12,
        &build,
        source,
        preparation,
        None,
        native::TrustUserCode::Acknowledged,
        || panic!("protocol peak over budget must not construct cancellation or child"),
        || panic!("protocol peak over budget must not reserve a slot"),
    );
    assert_eq!(
        result.unwrap_err(),
        "native preparation exceeds known peak data reservation"
    );
    assert_eq!(root.usage(), before);
    let bank = lock(&root.bank);
    assert!(!bank.sources.occupied());
    assert_eq!(bank.sources.next_epoch, 1);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_revalidation_workspace_is_charged_before_cancellation_or_child() {
    let (build, source, preparation) = input();
    let bound = source
        .preparation_storage_bound_on_control(&build, None)
        .unwrap();
    assert!(bound.helper_working_data_bytes > 0);
    assert!(bound.source_working_data_bytes > 0);
    // This covers the previously admitted retained/file/protocol components,
    // but deliberately leaves no allowance for either revalidation workspace.
    let peak = bound.max_retained_data_bytes()
        + hex_plugin_scan::MAX_BINARY as usize
        + hex_plugin_host::protocol::MAX_WORKING_DATA_BYTES;
    let root = root();
    root.configure_native_sources(
        1,
        NativeSourceDataReservation {
            max_retained_data_bytes: bound.max_retained_data_bytes(),
            max_construction_peak_data_bytes: peak,
        },
    )
    .unwrap();
    let before = root.usage();
    let result = root.begin_native_source_between(
        identity(),
        12,
        &build,
        source,
        preparation,
        None,
        native::TrustUserCode::Acknowledged,
        || panic!("revalidation workspace over budget must not construct cancellation or child"),
        || panic!("revalidation workspace over budget must not reserve a slot"),
    );
    assert_eq!(
        result.unwrap_err(),
        "native preparation exceeds known peak data reservation"
    );
    assert_eq!(root.usage(), before);
    let bank = lock(&root.bank);
    assert!(!bank.sources.occupied());
    assert_eq!(bank.sources.next_epoch, 1);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_retains_pending_native_source_until_disposal_and_exact_report_recovery() {
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
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
        let storage = actual.retained_storage_on_control().unwrap();
        assert_eq!(storage.shared_groups, 2);
        assert_eq!(storage.ipc_handle_bound, 2);
        assert!(storage.inventory_node_lifetime_bound > 0);
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
        root.configure_native_sources(1, source_reservation())
            .unwrap();
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
                .is_some_and(|e| matches!(e, hex_plugin_host::Error::Cancelled))
        );
        assert!(returned.cleanup_error.is_none());
        assert!(root.complete());
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_refused_native_request_retains_original_failure_through_cleanup() {
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
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
    {
        let work = root.work.try_lock().unwrap();
        let cell = &work.sources.cells[0];
        assert!(
            cell.error.is_none(),
            "original owner holds the only diagnostic"
        );
        assert!(matches!(
            cell.cause(),
            Some(hex_plugin_host::Error::Protocol(_))
        ));
        assert_eq!(
            root.try_native_source_error(binding),
            Err("native preparation is busy"),
            "a contended diagnostic is not an observed absence of error"
        );
    }
    assert!(
        root.try_native_source_error(binding)
            .unwrap()
            .is_some_and(|error| error.contains("configuration"))
    );
    assert_eq!(
        root.try_native_source_error(Binding {
            epoch: binding.epoch + 1,
            ..binding
        }),
        Err("native source differs")
    );
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(matches!(returned.disposal, PreparationDisposal::NoInstance));
    assert!(matches!(
        returned.error,
        Some(hex_plugin_host::Error::Protocol(_))
    ));
    assert!(returned.cleanup_error.is_none());
    assert!(root.native_source_error(binding).is_none());
    assert!(root.complete());
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_root_preserves_host_asset_state_refusal_after_input_accounting() {
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};

    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let (build, source, preparation) = input();
    let state = NativeDeviceState::new(
        hex_plugin_state::TYPE_ID,
        hex_plugin_state::STATE_VERSION,
        vec![],
    )
    .unwrap()
    .with_asset_references(vec![DeviceAssetReference {
        slot: DeviceAssetSlot::new(1).unwrap(),
        asset: DeviceAssetKey::new(1).unwrap(),
    }])
    .unwrap();
    let binding = root
        .begin_native_source(
            identity(),
            12,
            &build,
            source,
            preparation,
            Some(state),
            native::TrustUserCode::Acknowledged,
        )
        .unwrap();
    assert_eq!(
        root.native_source_progress(binding),
        Some(NativeSourcePreparationProgress::Retiring)
    );
    assert!(root.native_source_error(binding).is_some_and(|error| {
        error
            .to_string()
            .contains("startup state must be asset free")
    }));
    dispose(&root, binding);
    let returned = root.recover_native_source(binding).unwrap();
    assert!(matches!(returned.disposal, PreparationDisposal::NoInstance));
    assert!(returned.error.is_some_and(|error| {
        error
            .to_string()
            .contains("startup state must be asset free")
    }));
    assert!(returned.cleanup_error.is_none());
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
                max_retained_data_bytes: 64 * 1024 * 1024,
                max_construction_peak_data_bytes: 64 * 1024 * 1024,
            },
            std::mem::size_of::<Inner>(),
        )
        .unwrap();
    original
        .configure_native_source_preparation(1, source_reservation())
        .unwrap();
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
    assert!(returned.error.unwrap().to_string().contains("presentation"));
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
    root.configure_native_sources(1, source_reservation())
        .unwrap();
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
    // Hold the actual first request while other close/retire paths run. A
    // second request may not manufacture another pair of descriptor handles.
    let first = lock(&root.bank).sources.slots[0]
        .request_cancellation()
        .unwrap();
    for _ in 0..4 {
        assert!(root.retire_native_source(binding));
        let mut requests = empty_cancellations();
        lock(&root.bank).sources.close(&mut requests);
        assert!(requests.into_iter().all(|request| request.is_none()));
    }
    first.cancel_on_control();
    drop(first);
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
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let (build, source, preparation) = input();
    #[derive(Debug)]
    struct OriginalRefusal(String);
    impl std::fmt::Display for OriginalRefusal {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl std::error::Error for OriginalRefusal {}
    let original = std::io::Error::other(OriginalRefusal(String::from(
        "original cancellation pair refusal",
    )));
    let original_bytes = original
        .get_ref()
        .unwrap()
        .downcast_ref::<OriginalRefusal>()
        .unwrap()
        .0
        .as_ptr();
    let binding = root
        .begin_native_source_between(
            identity(),
            12,
            &build,
            source,
            preparation,
            None,
            native::TrustUserCode::Acknowledged,
            || Err(hex_plugin_host::Error::Io(original)),
            || panic!("native preparation must not start after cancellation creation refusal"),
        )
        .unwrap();
    assert_eq!(
        root.native_source_progress(binding),
        Some(NativeSourcePreparationProgress::Complete)
    );
    #[cfg(feature = "allocation-probe")]
    let returned = {
        let mut returned = None;
        let measured = allocation_counter::measure(|| {
            returned = root.recover_native_source(binding);
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
        assert_eq!((measured.bytes_total, measured.bytes_current), (0, 0));
        returned.unwrap()
    };
    #[cfg(not(feature = "allocation-probe"))]
    let returned = root.recover_native_source(binding).unwrap();
    let Some(hex_plugin_host::Error::Io(error)) = returned.error.as_ref() else {
        panic!("original typed factory error");
    };
    assert_eq!(
        error
            .get_ref()
            .unwrap()
            .downcast_ref::<OriginalRefusal>()
            .unwrap()
            .0
            .as_ptr(),
        original_bytes
    );
    assert!(matches!(returned.disposal, PreparationDisposal::NoInstance));
    assert!(
        returned
            .error
            .unwrap()
            .to_string()
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
            max_retained_data_bytes: 64 * 1024 * 1024,
            max_construction_peak_data_bytes: 64 * 1024 * 1024,
        })
        .unwrap();
    outer.announce_retirement();
    assert!(
        outer
            .configure_native_source_preparation(1, source_reservation())
            .is_err(),
        "retiring Root must not create a new native source pool"
    );
    assert!(
        outer
            .with_native_effect_definition::<()>(&[], |_| panic!("retiring Root must not compile"))
            .is_err()
    );
}

#[test]
fn native_pool_reserves_handles_before_allocating_and_keeps_them_while_vacant() {
    let per_source = resources::PoolResources::new(1).unwrap();
    assert_eq!(per_source.per_source.shared_groups, 12);
    assert_eq!(per_source.per_source.shared_handle_bound, 33);
    assert_eq!(
        per_source.per_source.source.inventory_node_lifetime_bound,
        4
    );
    assert_eq!(per_source.per_source.source.ipc_handle_bound, 4);
    assert_eq!(per_source.per_source.source.native_child_lifetime_bound, 1);
    assert_eq!(per_source.per_source.joining_thread_lifetime_bound, 1);
    for fits in [false, true] {
        let outer = CandidateRoot::new(2).unwrap();
        let root = &outer.0.eligible;
        // Same closed publication bank, but no native output carriers: this
        // fixture tests capacity admission, never native publication authority.
        root.configure_publications(
            outer.root_id(),
            AudioCandidatePublicationReservation {
                runtime_slots: 2,
                archive_slots: 2,
                member_rows: 64,
                transfer_rows: 64,
                receipt_rows: 2,
                data_rows: 4096,
                handle_rows: 2 * 5 + 28 + 2 * per_source.shared_handles - usize::from(!fits),
                max_retained_data_bytes: 64 * 1024 * 1024,
                max_construction_peak_data_bytes: 64 * 1024 * 1024,
            },
            std::mem::size_of::<Inner>(),
        )
        .unwrap();
        let mut result = None;
        let configure = || {
            result = Some(root.configure_native_sources(2, source_reservation()));
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(configure);
            if !fits {
                assert_eq!((measured.count_total, measured.count_current), (0, 0));
                assert_eq!((measured.bytes_total, measured.bytes_current), (0, 0));
            }
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            let mut configure = configure;
            configure();
        }
        let result = result.unwrap();
        if fits {
            result.unwrap();
            let bank = lock(&root.bank);
            assert_eq!(
                bank.sources.reserved_shared_handles(),
                2 * per_source.shared_handles
            );
            assert_eq!(publication::remaining_handle_reservation(&bank), Some(0));
            assert!(!bank.sources.occupied());
        } else {
            assert_eq!(
                result,
                Err("native preparation resource reservation exceeded")
            );
            assert!(lock(&root.bank).sources.slots.is_empty());
            assert!(root.work.try_lock().unwrap().sources.cells.is_empty());
        }
        assert!(root.complete());
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn failed_effect_requests_original_cancellation_without_claiming_disposal() {
    use std::io::Write;
    let root = root();
    root.configure_native_sources(1, source_reservation())
        .unwrap();
    let binding = start(&root);
    root.complete_native_source(
        binding,
        Some(hex_plugin_host::protocol::PresentationLatency {
            input_frames: 77,
            output_frames: 91,
        }),
    )
    .unwrap();
    root.start_native_effect(binding).unwrap();
    let descriptor = lock(&root.bank).sources.slots[0]
        .cancellation
        .as_ref()
        .unwrap()
        .descriptor_lifetime_probe_on_control()
        .upgrade()
        .unwrap();
    // Use the actual production failure transition after real SDK/worker
    // construction. Failed custody cannot use the normal retirement path.
    root.fail_native_source(0);
    let mut writer = descriptor.as_ref();
    let error = writer
        .write(&[0])
        .expect_err("failed source must shut down its original cancellation socket");
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::NotConnected
    ));
    assert_eq!(
        root.native_source_progress(binding),
        Some(NativeSourcePreparationProgress::Failed)
    );
    assert!(root.recover_native_source(binding).is_none());
    assert!(!root.complete(), "cancellation is not a disposal receipt");
    assert!(matches!(
        root.work.try_lock().unwrap().sources.cells[0].owner,
        Some(Owner::Effect(_))
    ));
    // This isolated fixture finally destroys its original owners on control;
    // no graph/native-output disposal or reusable source was manufactured.
}

#[cfg(feature = "allocation-probe")]
pub(crate) fn original_worker_awaits_endpoint(root: &CandidateRoot, binding: Binding) -> bool {
    let work = lock(&root.0.eligible.work);
    let bank = lock(&root.0.eligible.bank);
    let Some(index) = bank.sources.index(binding) else {
        return false;
    };
    bank.sources.slots[index].endpoint_loaned
        && matches!(work.sources.cells[index].owner, Some(Owner::Loaned(_)))
}

#[cfg(feature = "allocation-probe")]
pub(crate) fn original_worker_published_blocks(
    root: &CandidateRoot,
    binding: Binding,
) -> Option<u64> {
    let work = lock(&root.0.eligible.work);
    let bank = lock(&root.0.eligible.bank);
    let index = bank.sources.index(binding)?;
    match work.sources.cells[index].owner.as_ref()? {
        Owner::Loaned(control) => Some(control.test_published_blocks()),
        _ => None,
    }
}

#[cfg(feature = "allocation-probe")]
pub(crate) fn original_worker_transport_live(
    root: &CandidateRoot,
    binding: Binding,
) -> Option<bool> {
    let work = lock(&root.0.eligible.work);
    let bank = lock(&root.0.eligible.bank);
    let index = bank.sources.index(binding)?;
    match work.sources.cells[index].owner.as_ref()? {
        Owner::Loaned(control) => Some(control.transport_is_live_on_control()),
        _ => None,
    }
}

#[test]
fn endpoint_return_reservation_orders_decisions_without_delaying_return_requests() {
    let signal = EndpointSignal::new();
    let mut fence = signal.reserve().unwrap();
    assert!(signal.reserve().is_none());
    std::thread::scope(|scope| {
        scope.spawn(|| signal.request_return()).join().unwrap();
    });
    assert!(!fence.is_live());
    assert!(!fence.try_order_before_return());
    drop(fence);
    assert!(signal.load(Ordering::Acquire));
    assert!(signal.reserve().is_none());

    let signal = EndpointSignal::new();
    let mut fence = signal.reserve().unwrap();
    assert!(fence.try_order_before_return());
    assert!(!fence.try_order_before_return());
    signal.request_return();
    assert!(!fence.is_live());
    drop(fence);
    assert!(signal.load(Ordering::Acquire));
    assert!(signal.reserve().is_none());
}
