//! Read-only reserved DATA capacities; no allocator overhead or mutable keys.
use super::{TempoMap, TempoPoint};

/// Backing allocations only. The containing TempoMap already owns the Vec
/// headers; do not charge them again. No allocator/RSS measurement is claimed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TempoMapOwnedAllocationLayout {
    point_capacity: usize,
    prefix_capacity: usize,
}
impl TempoMapOwnedAllocationLayout {
    pub fn point_capacity(self) -> usize {
        self.point_capacity
    }
    pub fn prefix_capacity(self) -> usize {
        self.prefix_capacity
    }
    pub fn point_element_bytes(self) -> usize {
        std::mem::size_of::<TempoPoint>()
    }
    pub fn prefix_element_bytes(self) -> usize {
        std::mem::size_of::<u128>()
    }
    pub fn point_bytes(self) -> Option<usize> {
        self.point_capacity.checked_mul(self.point_element_bytes())
    }
    pub fn prefix_bytes(self) -> Option<usize> {
        self.prefix_capacity
            .checked_mul(self.prefix_element_bytes())
    }
    pub fn checked_backing_bytes(self) -> Option<usize> {
        self.point_bytes()?.checked_add(self.prefix_bytes()?)
    }
}
impl TempoMap {
    /// Control-only allocation inspection, not timing validation/admission.
    /// O(1), immutable and allocation-free; reports actual capacities, not len.
    pub fn owned_allocation_layout(&self) -> TempoMapOwnedAllocationLayout {
        let Self {
            sample_rate: _,
            points,
            frame_prefix_q32,
        } = self;
        // Exhaustive scalar leaf pattern; no point/key is exposed to callers.
        if let Some(TempoPoint { beat: _, tempo: _ }) = points.first() {}
        TempoMapOwnedAllocationLayout {
            point_capacity: points.capacity(),
            prefix_capacity: frame_prefix_q32.capacity(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BeatTime, Tempo};
    #[test]
    fn actual_overreserved_capacity_headers_and_timing_are_preserved() {
        let mut points = Vec::with_capacity(31);
        points.push(TempoPoint::new(
            BeatTime::ZERO,
            Tempo::new(120_000_000).unwrap(),
        ));
        let pointer = points.as_ptr();
        let mut map = TempoMap::new(48_000, points).unwrap();
        map.frame_prefix_q32.reserve(23);
        let prefix_pointer = map.frame_prefix_q32.as_ptr();
        let before = map
            .beat_to_frame(BeatTime::from_quarters(7).unwrap())
            .unwrap();
        let layout = map.owned_allocation_layout();
        assert_eq!(layout.point_capacity(), 31);
        assert_eq!(layout.prefix_capacity(), map.frame_prefix_q32.capacity());
        let expected = map.points.capacity() * std::mem::size_of::<TempoPoint>()
            + map.frame_prefix_q32.capacity() * std::mem::size_of::<u128>();
        assert_eq!(layout.checked_backing_bytes(), Some(expected));
        assert!(
            expected
                > map.points.len() * std::mem::size_of::<TempoPoint>()
                    + map.frame_prefix_q32.len() * std::mem::size_of::<u128>()
        );
        assert_ne!(
            layout.checked_backing_bytes(),
            Some(expected + std::mem::size_of::<TempoMap>())
        );
        assert_eq!(map.points.as_ptr(), pointer);
        assert_eq!(map.frame_prefix_q32.as_ptr(), prefix_pointer);
        assert_eq!(
            map.beat_to_frame(BeatTime::from_quarters(7).unwrap())
                .unwrap(),
            before
        );
    }
    #[test]
    fn empty_spare_data_fixture_is_reported_without_relaxing_constructor_validation() {
        // Private allocation-only fixture, never a valid published timing map.
        let map = TempoMap {
            sample_rate: 48_000,
            points: Vec::with_capacity(11),
            frame_prefix_q32: Vec::with_capacity(13),
        };
        let layout = map.owned_allocation_layout();
        assert_eq!(layout.point_capacity(), 11);
        assert_eq!(layout.prefix_capacity(), 13);
        assert!(layout.checked_backing_bytes().unwrap() > 0);
        assert!(TempoMap::new(48_000, Vec::new()).is_err());
    }
    #[test]
    fn multiplication_and_combined_overflow_refuse() {
        let multiplication = TempoMapOwnedAllocationLayout {
            point_capacity: usize::MAX,
            prefix_capacity: 0,
        };
        assert!(multiplication.point_bytes().is_none());
        assert!(multiplication.checked_backing_bytes().is_none());
        let addition = TempoMapOwnedAllocationLayout {
            point_capacity: usize::MAX / std::mem::size_of::<TempoPoint>(),
            prefix_capacity: 1,
        };
        assert!(addition.point_bytes().is_some());
        assert!(addition.prefix_bytes().is_some());
        assert!(addition.checked_backing_bytes().is_none());
    }
}
