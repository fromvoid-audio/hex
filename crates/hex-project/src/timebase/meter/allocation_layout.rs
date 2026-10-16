//! Read-only reserved DATA capacities; no allocator overhead or mutable keys.
use super::{MeterMap, MeterPoint};

/// Backing allocations only. The containing MeterMap already owns the Vec
/// headers; do not charge them again. No allocator/RSS measurement is claimed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeterMapOwnedAllocationLayout {
    point_capacity: usize,
    prefix_capacity: usize,
}
impl MeterMapOwnedAllocationLayout {
    pub fn point_capacity(self) -> usize {
        self.point_capacity
    }
    pub fn prefix_capacity(self) -> usize {
        self.prefix_capacity
    }
    pub fn point_element_bytes(self) -> usize {
        std::mem::size_of::<MeterPoint>()
    }
    pub fn prefix_element_bytes(self) -> usize {
        std::mem::size_of::<u64>()
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
impl MeterMap {
    /// Control-only allocation inspection, not timing validation/admission.
    /// O(1), immutable and allocation-free; reports actual capacities, not len.
    pub fn owned_allocation_layout(&self) -> MeterMapOwnedAllocationLayout {
        let Self {
            points,
            first_bar_at_point,
        } = self;
        // Exhaustive scalar leaf pattern; no point/key is exposed to callers.
        if let Some(MeterPoint { beat: _, meter: _ }) = points.first() {}
        MeterMapOwnedAllocationLayout {
            point_capacity: points.capacity(),
            prefix_capacity: first_bar_at_point.capacity(),
        }
    }
}
