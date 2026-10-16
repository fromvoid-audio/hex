use super::*;
use crate::{BeatTime, Tempo, TempoPoint};
fn source() -> TempoMap {
    TempoMap::new(
        48_000,
        vec![
            TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
            TempoPoint::new(
                BeatTime::from_quarters(3).unwrap(),
                Tempo::new(87_000_000).unwrap(),
            ),
        ],
    )
    .unwrap()
}
#[test]
fn both_reserve_failures_and_late_copy_failure_retain_actual_destinations() {
    let source = source();
    for fault in 0..3 {
        let mut copy = TempoMapCopyCustody::new();
        copy.fault = Some(fault);
        assert_eq!(
            copy.prepare_from(&source),
            Err(TempoMapCopyError::Allocation)
        );
        assert_eq!(copy.phase(), TempoMapCopyPhase::Cleaning);
        assert_eq!(copy.map.points.len(), if fault == 2 { 2 } else { 0 });
        let observed = copy.capacity().layout;
        assert_eq!(observed.point_capacity() > 0, fault > 0);
        assert_eq!(observed.prefix_capacity() > 0, fault > 1);
        assert_eq!(copy.prepare_from(&source), Err(TempoMapCopyError::Occupied));
        copy.clean_on_control().unwrap();
        assert_eq!(copy.capacity().layout, observed);
        copy.fault = None;
        copy.prepare_from(&source).unwrap();
        assert_eq!(copy.map.points, source.points);
        assert_eq!(copy.map.frame_prefix_q32, source.frame_prefix_q32);
    }
}
#[test]
fn transferred_timing_prefixes_and_spare_capacity_are_exact_and_not_double_charged() {
    let source = source();
    let mut copy = TempoMapCopyCustody::new();
    copy.map.points.reserve_exact(31);
    copy.map.frame_prefix_q32.reserve_exact(47);
    let backing = copy.capacity().layout;
    let points = copy.map.points.as_ptr();
    let prefixes = copy.map.frame_prefix_q32.as_ptr();
    copy.prepare_from(&source).unwrap();
    assert_eq!(copy.capacity().previous_growth_backing_bytes, 0);
    let mut occupied = Some(source.clone());
    assert_eq!(
        copy.transfer_into(&mut occupied),
        Err(TempoMapCopyError::Occupied)
    );
    assert_eq!(copy.phase(), TempoMapCopyPhase::Validated);
    let mut destination = None;
    copy.transfer_into(&mut destination).unwrap();
    assert_eq!(copy.capacity().layout.checked_backing_bytes(), Some(0));
    let map = destination.as_ref().unwrap();
    assert_eq!(map.owned_allocation_layout(), backing);
    assert_eq!(map.points.as_ptr(), points);
    assert_eq!(map.frame_prefix_q32.as_ptr(), prefixes);
    assert_eq!(map.frame_prefix_q32, source.frame_prefix_q32);
    for quarter in 0..20 {
        let beat = BeatTime::from_quarters(quarter).unwrap();
        let frame = source.beat_to_frame(beat).unwrap();
        assert_eq!(map.beat_to_frame(beat), Ok(frame));
        assert_eq!(map.frame_to_beat(frame), source.frame_to_beat(frame));
    }
    assert_eq!(copy.prepare_from(&source), Err(TempoMapCopyError::Occupied));
    copy.clean_transferred_on_control(&mut destination).unwrap();
    assert!(destination.is_none());
    assert_eq!(copy.capacity().layout, backing);
    copy.prepare_from(&source).unwrap();
    assert_eq!(copy.map.points.as_ptr(), points);
    assert_eq!(copy.capacity().previous_growth_backing_bytes, 0);
}
#[test]
fn panic_keeps_partial_copy_and_payload_without_retry_or_cleanup() {
    let mut copy = TempoMapCopyCustody::new();
    copy.panic_after_points = true;
    assert_eq!(
        copy.prepare_from(&source()),
        Err(TempoMapCopyError::Incomplete)
    );
    assert_eq!(copy.map.points.len(), 2);
    assert!(copy.first_panic.is_some());
    assert_eq!(copy.phase(), TempoMapCopyPhase::Incomplete);
    let capacity = copy.capacity();
    assert_eq!(copy.clean_on_control(), Err(TempoMapCopyError::Incomplete));
    assert_eq!(
        copy.prepare_from(&source()),
        Err(TempoMapCopyError::Occupied)
    );
    assert_eq!(copy.capacity(), capacity);
}
#[test]
fn checked_growth_charges_old_allocations_only_when_replaced() {
    let mut copy = TempoMapCopyCustody::new();
    copy.prepare_from(&TempoMap::constant(48_000, Tempo::new(100_000_000).unwrap()).unwrap())
        .unwrap();
    copy.clean_on_control().unwrap();
    let old = copy.capacity().layout.checked_backing_bytes().unwrap();
    copy.prepare_from(&source()).unwrap();
    assert_eq!(copy.capacity().previous_growth_backing_bytes, old);
    assert!(copy.capacity().layout.checked_backing_bytes().unwrap() > old);
}
