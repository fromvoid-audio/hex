//! Control-owned, persistent copy destinations for validated canonical meter.
//! No new timing kernel, mutable exposure, or allocator/RSS claim.
use super::{MeterMap, MeterMapOwnedAllocationLayout};
use std::panic::{AssertUnwindSafe, catch_unwind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterMapCopyPhase {
    Vacant,
    Preparing,
    Validated,
    Transferred,
    Cleaning,
    Incomplete,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterMapCopyError {
    Occupied,
    Allocation,
    Overflow,
    Incomplete,
}
/// Actual backing capacities; inline custody/map/Vec headers are charged by the
/// containing owner once. Growth overlap is conservative old backing retained
/// during this attempt, not bytes still live after successful reservation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeterMapCopyCapacity {
    pub layout: MeterMapOwnedAllocationLayout,
    pub previous_growth_backing_bytes: usize,
}
/// No Clone/Deref/raw-vector access. The host must retain this owner across a
/// failed or abandoned operation. Only explicit normal cleanup makes it vacant.
/// Process-fatal OOM/abort is outside the recoverable contract.
pub struct MeterMapCopyCustody {
    map: MeterMap,
    phase: MeterMapCopyPhase,
    growth: usize,
    first_panic: Option<Box<dyn std::any::Any + Send>>,
    #[cfg(test)]
    fault: Option<usize>,
    #[cfg(test)]
    panic_after_points: bool,
}
impl Default for MeterMapCopyCustody {
    fn default() -> Self {
        Self::new()
    }
}
impl MeterMapCopyCustody {
    pub fn new() -> Self {
        Self {
            map: MeterMap {
                points: Vec::new(),
                first_bar_at_point: Vec::new(),
            },
            phase: MeterMapCopyPhase::Vacant,
            growth: 0,
            first_panic: None,
            #[cfg(test)]
            fault: None,
            #[cfg(test)]
            panic_after_points: false,
        }
    }
    pub fn phase(&self) -> MeterMapCopyPhase {
        self.phase
    }
    pub fn capacity(&self) -> MeterMapCopyCapacity {
        MeterMapCopyCapacity {
            layout: self.map.owned_allocation_layout(),
            previous_growth_backing_bytes: self.growth,
        }
    }
    /// Both reserves finish before either copy. All owning fields are already
    /// persistent; errors contain no owner. A caught panic is sticky/no retry.
    pub fn prepare_from(&mut self, source: &MeterMap) -> Result<(), MeterMapCopyError> {
        if self.phase != MeterMapCopyPhase::Vacant {
            return Err(MeterMapCopyError::Occupied);
        }
        self.phase = MeterMapCopyPhase::Preparing;
        self.growth = 0;
        match catch_unwind(AssertUnwindSafe(|| self.copy_inner(source))) {
            Ok(result) => {
                self.phase = if result.is_ok() {
                    MeterMapCopyPhase::Validated
                } else {
                    MeterMapCopyPhase::Cleaning
                };
                result
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = MeterMapCopyPhase::Incomplete;
                Err(MeterMapCopyError::Incomplete)
            }
        }
    }
    fn copy_inner(&mut self, source: &MeterMap) -> Result<(), MeterMapCopyError> {
        let before = self.map.owned_allocation_layout();
        // Compute all old-growth arithmetic before either allocation changes.
        self.growth = if source.points.len() > before.point_capacity() {
            before.point_bytes().ok_or(MeterMapCopyError::Overflow)?
        } else {
            0
        }
        .checked_add(
            if source.first_bar_at_point.len() > before.prefix_capacity() {
                before.prefix_bytes().ok_or(MeterMapCopyError::Overflow)?
            } else {
                0
            },
        )
        .ok_or(MeterMapCopyError::Overflow)?;
        #[cfg(test)]
        if self.fault == Some(0) {
            return Err(MeterMapCopyError::Allocation);
        }
        self.map
            .points
            .try_reserve_exact(source.points.len())
            .map_err(|_| MeterMapCopyError::Allocation)?;
        #[cfg(test)]
        if self.fault == Some(1) {
            return Err(MeterMapCopyError::Allocation);
        }
        self.map
            .first_bar_at_point
            .try_reserve_exact(source.first_bar_at_point.len())
            .map_err(|_| MeterMapCopyError::Allocation)?;
        self.map.points.extend_from_slice(&source.points);
        #[cfg(test)]
        {
            assert!(!self.panic_after_points, "persistent meter copy panic");
            if self.fault == Some(2) {
                return Err(MeterMapCopyError::Allocation);
            }
        }
        self.map
            .first_bar_at_point
            .extend_from_slice(&source.first_bar_at_point);
        self.map
            .owned_allocation_layout()
            .checked_backing_bytes()
            .ok_or(MeterMapCopyError::Overflow)?;
        Ok(())
    }
    /// Validated transfer into an already allocated, vacant persistent slot.
    /// Refusal mutates neither owner. After preflight only fixed moves remain.
    pub fn transfer_into(
        &mut self,
        destination: &mut Option<MeterMap>,
    ) -> Result<(), MeterMapCopyError> {
        if self.phase != MeterMapCopyPhase::Validated || destination.is_some() {
            return Err(MeterMapCopyError::Occupied);
        }
        *destination = Some(MeterMap {
            points: std::mem::take(&mut self.map.points),
            first_bar_at_point: std::mem::take(&mut self.map.first_bar_at_point),
        });
        self.phase = MeterMapCopyPhase::Transferred;
        Ok(())
    }
    /// Explicitly return a normally completed map to this same empty copy
    /// destination for retained-capacity reuse. No occupied owner is overwritten.
    /// The engine host keeps that destination private. This general owned-data
    /// helper does not authenticate allocation origin or mint a lifetime fact;
    /// its capacity report always describes the actual supplied owner.
    pub fn clean_transferred_on_control(
        &mut self,
        destination: &mut Option<MeterMap>,
    ) -> Result<(), MeterMapCopyError> {
        if self.phase != MeterMapCopyPhase::Transferred
            || destination.is_none()
            || self.map.points.capacity() != 0
            || self.map.first_bar_at_point.capacity() != 0
        {
            return Err(MeterMapCopyError::Occupied);
        }
        if let Some(map) = destination.take() {
            self.map = map;
        }
        self.phase = MeterMapCopyPhase::Cleaning;
        self.clean_on_control()
    }
    /// Copies contain only scalar elements. Clearing them retains every backing
    /// allocation, including spare capacity, and never destroys the panic record.
    pub fn clean_on_control(&mut self) -> Result<(), MeterMapCopyError> {
        if self.phase == MeterMapCopyPhase::Incomplete {
            return Err(MeterMapCopyError::Incomplete);
        }
        if self.phase == MeterMapCopyPhase::Transferred {
            return Err(MeterMapCopyError::Occupied);
        }
        self.phase = MeterMapCopyPhase::Cleaning;
        self.map.points.clear();
        self.map.first_bar_at_point.clear();
        self.phase = MeterMapCopyPhase::Vacant;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> MeterMap {
        let mut points = Vec::with_capacity(19);
        points.push(super::super::MeterPoint::new(
            crate::BeatTime::ZERO,
            crate::Meter::COMMON_TIME,
        ));
        points.push(super::super::MeterPoint::new(
            crate::BeatTime::from_quarters(7).unwrap(),
            crate::Meter::new(3, 4).unwrap(),
        ));
        MeterMap::new(points).unwrap()
    }
    #[test]
    fn copied_meter_retains_actual_allocations_through_transfer_and_normal_cleanup() {
        let original = source();
        let mut copy = MeterMapCopyCustody::new();
        copy.prepare_from(&original).unwrap();
        let pointer = copy.map.points.as_ptr();
        let prefix = copy.map.first_bar_at_point.as_ptr();
        let mut target = None;
        copy.transfer_into(&mut target).unwrap();
        assert_eq!(target.as_ref().unwrap(), &original);
        assert_eq!(target.as_ref().unwrap().points.as_ptr(), pointer);
        assert_eq!(target.as_ref().unwrap().first_bar_at_point.as_ptr(), prefix);
        assert_eq!(copy.capacity().layout.checked_backing_bytes(), Some(0));
        assert_eq!(copy.clean_on_control(), Err(MeterMapCopyError::Occupied));
        copy.clean_transferred_on_control(&mut target).unwrap();
        assert!(target.is_none());
        assert_eq!(copy.phase(), MeterMapCopyPhase::Vacant);
        assert!(copy.capacity().layout.checked_backing_bytes().unwrap() > 0);
        copy.prepare_from(&original).unwrap();
        assert_eq!(copy.map.points.as_ptr(), pointer);
        assert_eq!(copy.map.first_bar_at_point.as_ptr(), prefix);
    }
    #[test]
    fn reserve_fill_and_panic_failures_keep_partial_copy_custody() {
        for fault in 0..3 {
            let mut copy = MeterMapCopyCustody::new();
            copy.fault = Some(fault);
            assert_eq!(
                copy.prepare_from(&source()),
                Err(MeterMapCopyError::Allocation)
            );
            assert_eq!(copy.phase(), MeterMapCopyPhase::Cleaning);
            if fault > 0 {
                assert!(copy.capacity().layout.point_capacity() > 0);
            }
            copy.clean_on_control().unwrap();
        }
        let mut copy = MeterMapCopyCustody::new();
        copy.panic_after_points = true;
        assert_eq!(
            copy.prepare_from(&source()),
            Err(MeterMapCopyError::Incomplete)
        );
        assert!(copy.first_panic.is_some());
        assert!(copy.capacity().layout.checked_backing_bytes().unwrap() > 0);
        assert_eq!(copy.clean_on_control(), Err(MeterMapCopyError::Incomplete));
        assert_eq!(
            copy.prepare_from(&source()),
            Err(MeterMapCopyError::Occupied)
        );
    }
}
