//! Allocation proof for the same private foundation, not a callback/input claim.
#![cfg(feature = "allocation-probe")]
#[path = "../../hex-engine/tests/typing_foundation.rs"]
mod foundation;

#[test]
fn private_typing_foundation_allocates_and_deallocates_nothing() {
    let counts = allocation_counter::measure(|| {
        for _ in 0..128 {
            foundation::exercise_without_allocations();
        }
    });
    assert_eq!(counts.count_total, 0);
    assert_eq!(counts.bytes_total, 0);
    assert_eq!(counts.count_current, 0);
    assert_eq!(counts.bytes_current, 0);
}
