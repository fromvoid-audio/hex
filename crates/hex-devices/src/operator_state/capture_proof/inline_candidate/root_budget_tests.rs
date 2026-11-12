use super::super::{adapter::Root, tests::key};
use super::{root_leases::*, root_refs::*, root_tests::catalog};
use std::{mem::size_of, sync::Arc};
fn r(l: Location) -> RootRef {
    RootRef::encode(l).unwrap()
}
#[test]
fn real_context_budget_and_new_host_table_charge_prevent_universal_fit_claims() {
    let (mut host, record, cache) = catalog();
    let root_bytes = host.entries[0].root.context.declared_context_bytes();
    let state = host.entries[0].root.operator().authored_state.clone();
    let count = 2 * 1024 * 1024 / root_bytes;
    // Unrelated retained owners with identical bytes are NOT deduplicated.
    for i in 0..count {
        host.entries.push(Entry {
            root: Arc::new(Root::new(
                key(1000 + i as u64, 1000 + i as u64),
                &state,
                48000,
            )),
            counts: [0, 0, 0, 1],
            location: r(Location::Retained((i + 1) as u32)),
        });
    }
    let p = host.preflight(record, cache, record.receipt, 1).unwrap();
    let before = host.fingerprint();
    let epoch = host.epoch;
    assert_eq!(host.acquire(p, 0), Err(Error::Budget));
    assert_eq!(host.fingerprint(), before);
    assert_eq!(host.epoch, epoch);
    // Actual extra table capacities are charged; no invented free host map.
    let (mut small, _, _) = catalog();
    small
        .entries
        .try_reserve_exact(256 - small.entries.len())
        .unwrap();
    assert_eq!(small.entries.capacity(), 256);
    assert_eq!((size_of::<Entry>(), size_of::<Catalog>()), (32, 32));
    let total = 30728168 + pool_bytes() + small.bookkeeping_bytes() + small.owner_header_bytes();
    assert_eq!(total, 33553736);
    assert_eq!(33554432 - total, 696);
    assert!(small.declared_closure_bytes() > small.bookkeeping_bytes());
    assert_eq!(small.owner_header_bytes(), 192);
    small
        .entries
        .try_reserve_exact(277 - small.entries.len())
        .unwrap();
    assert_eq!(small.entries.capacity(), 277);
    assert_eq!(
        33554432
            - (30728168 + pool_bytes() + small.bookkeeping_bytes() + small.owner_header_bytes()),
        24
    );
    small
        .entries
        .try_reserve_exact(278 - small.entries.len())
        .unwrap();
    assert_eq!(small.entries.capacity(), 278);
    assert_eq!(
        (30728168 + pool_bytes() + small.bookkeeping_bytes() + small.owner_header_bytes())
            - 33554432,
        8
    );
    // The proposed context alone makes 417 independent three-root recipes exceed 2 MiB.
    assert!(417 * 3 * super::super::layout::context_bytes() > 2 * 1024 * 1024);
    eprintln!(
        "cached metadata={} slot={} header={} pool={} EQ_margin=9112 real_v8_context={} owner_entry={} host_header={} new_host_capacity256_EQ_margin=696 complete_fixture_declared_closure={}",
        size_of::<CachedMetadata>(),
        size_of::<CachedSlot>(),
        size_of::<CachedPool>(),
        pool_bytes(),
        root_bytes,
        size_of::<Entry>(),
        size_of::<Catalog>(),
        small.declared_closure_bytes()
    );
}
