//! Sensitivity checks for the opt-in per-thread allocator probe.
#![cfg(feature = "allocation-probe")]

use allocation_counter::measure;
use std::hint::black_box;

#[test]
fn probe_detects_allocation_even_when_freed_in_the_same_scope() {
    let empty = measure(|| {});
    assert_eq!(empty.count_total, 0);
    assert_eq!(empty.count_current, 0);
    assert_eq!(empty.bytes_total, 0);
    assert_eq!(empty.bytes_current, 0);

    let observed = measure(|| {
        let value = black_box(Box::new(black_box(42_u64)));
        black_box(&value);
        drop(value);
    });
    assert_eq!(observed.count_total, 1);
    assert_eq!(observed.count_current, 0);
    assert_eq!(observed.bytes_total, 8);
    assert_eq!(observed.bytes_current, 0);
}

#[test]
fn probe_detects_freeing_memory_allocated_before_the_scope() {
    let value = black_box(Box::new(black_box(42_u64)));
    let observed = measure(|| drop(value));
    assert_eq!(observed.count_total, 0);
    assert_eq!(observed.count_current, -1);
    assert_eq!(observed.bytes_total, 0);
    assert_eq!(observed.bytes_current, -8);
}
