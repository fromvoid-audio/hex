//! Fast non-SDK unit tests for `candidate_custody::eligible::native_sources`.
use super::*;

fn root() -> Custody {
    super::super::tests::configured(64 * 1024 * 1024)
}

fn source_reservation() -> NativeSourceDataReservation {
    NativeSourceDataReservation {
        max_retained_data_bytes: 4 * 1024 * 1024,
        max_construction_peak_data_bytes: 24 * 1024 * 1024,
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
