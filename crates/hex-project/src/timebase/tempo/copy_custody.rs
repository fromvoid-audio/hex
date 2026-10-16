//! Control-owned, persistent copy destinations for validated canonical tempo.
//! No new timing kernel, mutable exposure, or allocator/RSS claim.
use super::{TempoMap, TempoMapOwnedAllocationLayout};
use std::panic::{AssertUnwindSafe, catch_unwind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TempoMapCopyPhase {
    Vacant,
    Preparing,
    Validated,
    Transferred,
    Cleaning,
    Incomplete,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TempoMapCopyError {
    Occupied,
    Allocation,
    Overflow,
    Incomplete,
}
/// Actual backing capacities; inline custody/map/Vec headers are charged by the
/// containing owner once. Growth overlap is conservative old backing retained
/// during this attempt, not bytes still live after successful reservation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TempoMapCopyCapacity {
    pub layout: TempoMapOwnedAllocationLayout,
    pub previous_growth_backing_bytes: usize,
}
/// No Clone/Deref/raw-vector access. The host must retain this owner across a
/// failed or abandoned operation. Only explicit normal cleanup makes it vacant.
/// Process-fatal OOM/abort is outside the recoverable contract.
pub struct TempoMapCopyCustody {
    map: TempoMap,
    phase: TempoMapCopyPhase,
    growth: usize,
    first_panic: Option<Box<dyn std::any::Any + Send>>,
    #[cfg(test)]
    fault: Option<usize>,
    #[cfg(test)]
    panic_after_points: bool,
}
impl Default for TempoMapCopyCustody {
    fn default() -> Self {
        Self::new()
    }
}
impl TempoMapCopyCustody {
    pub fn new() -> Self {
        Self {
            map: TempoMap {
                sample_rate: 0,
                points: Vec::new(),
                frame_prefix_q32: Vec::new(),
            },
            phase: TempoMapCopyPhase::Vacant,
            growth: 0,
            first_panic: None,
            #[cfg(test)]
            fault: None,
            #[cfg(test)]
            panic_after_points: false,
        }
    }
    pub fn phase(&self) -> TempoMapCopyPhase {
        self.phase
    }
    pub fn capacity(&self) -> TempoMapCopyCapacity {
        TempoMapCopyCapacity {
            layout: self.map.owned_allocation_layout(),
            previous_growth_backing_bytes: self.growth,
        }
    }
    /// Both reserves finish before either copy. All owning fields are already
    /// persistent; errors contain no owner. A caught panic is sticky/no retry.
    pub fn prepare_from(&mut self, source: &TempoMap) -> Result<(), TempoMapCopyError> {
        if self.phase != TempoMapCopyPhase::Vacant {
            return Err(TempoMapCopyError::Occupied);
        }
        self.phase = TempoMapCopyPhase::Preparing;
        self.growth = 0;
        match catch_unwind(AssertUnwindSafe(|| self.copy_inner(source))) {
            Ok(result) => {
                self.phase = if result.is_ok() {
                    TempoMapCopyPhase::Validated
                } else {
                    TempoMapCopyPhase::Cleaning
                };
                result
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = TempoMapCopyPhase::Incomplete;
                Err(TempoMapCopyError::Incomplete)
            }
        }
    }
    fn copy_inner(&mut self, source: &TempoMap) -> Result<(), TempoMapCopyError> {
        let before = self.map.owned_allocation_layout();
        // Compute all old-growth arithmetic before either allocation changes.
        self.growth = if source.points.len() > before.point_capacity() {
            before.point_bytes().ok_or(TempoMapCopyError::Overflow)?
        } else {
            0
        }
        .checked_add(
            if source.frame_prefix_q32.len() > before.prefix_capacity() {
                before.prefix_bytes().ok_or(TempoMapCopyError::Overflow)?
            } else {
                0
            },
        )
        .ok_or(TempoMapCopyError::Overflow)?;
        #[cfg(test)]
        if self.fault == Some(0) {
            return Err(TempoMapCopyError::Allocation);
        }
        self.map
            .points
            .try_reserve_exact(source.points.len())
            .map_err(|_| TempoMapCopyError::Allocation)?;
        #[cfg(test)]
        if self.fault == Some(1) {
            return Err(TempoMapCopyError::Allocation);
        }
        self.map
            .frame_prefix_q32
            .try_reserve_exact(source.frame_prefix_q32.len())
            .map_err(|_| TempoMapCopyError::Allocation)?;
        self.map.points.extend_from_slice(&source.points);
        #[cfg(test)]
        {
            assert!(!self.panic_after_points, "persistent tempo copy panic");
            if self.fault == Some(2) {
                return Err(TempoMapCopyError::Allocation);
            }
        }
        self.map
            .frame_prefix_q32
            .extend_from_slice(&source.frame_prefix_q32);
        self.map.sample_rate = source.sample_rate;
        self.map
            .owned_allocation_layout()
            .checked_backing_bytes()
            .ok_or(TempoMapCopyError::Overflow)?;
        Ok(())
    }
    /// Validated transfer into an already allocated, vacant persistent slot.
    /// Refusal mutates neither owner. After preflight only fixed moves remain.
    pub fn transfer_into(
        &mut self,
        destination: &mut Option<TempoMap>,
    ) -> Result<(), TempoMapCopyError> {
        if self.phase != TempoMapCopyPhase::Validated || destination.is_some() {
            return Err(TempoMapCopyError::Occupied);
        }
        *destination = Some(TempoMap {
            sample_rate: self.map.sample_rate,
            points: std::mem::take(&mut self.map.points),
            frame_prefix_q32: std::mem::take(&mut self.map.frame_prefix_q32),
        });
        self.phase = TempoMapCopyPhase::Transferred;
        Ok(())
    }
    /// Explicitly return a normally completed map to this same empty copy
    /// destination for retained-capacity reuse. No occupied owner is overwritten.
    /// The engine host keeps that destination private. This general owned-data
    /// helper does not authenticate allocation origin or mint a lifetime fact;
    /// its capacity report always describes the actual supplied owner.
    pub fn clean_transferred_on_control(
        &mut self,
        destination: &mut Option<TempoMap>,
    ) -> Result<(), TempoMapCopyError> {
        if self.phase != TempoMapCopyPhase::Transferred
            || destination.is_none()
            || self.map.points.capacity() != 0
            || self.map.frame_prefix_q32.capacity() != 0
        {
            return Err(TempoMapCopyError::Occupied);
        }
        if let Some(map) = destination.take() {
            self.map = map;
        }
        self.phase = TempoMapCopyPhase::Cleaning;
        self.clean_on_control()
    }
    /// Copies contain only scalar elements. Clearing them retains every backing
    /// allocation, including spare capacity, and never destroys the panic record.
    pub fn clean_on_control(&mut self) -> Result<(), TempoMapCopyError> {
        if self.phase == TempoMapCopyPhase::Incomplete {
            return Err(TempoMapCopyError::Incomplete);
        }
        if self.phase == TempoMapCopyPhase::Transferred {
            return Err(TempoMapCopyError::Occupied);
        }
        self.phase = TempoMapCopyPhase::Cleaning;
        self.map.points.clear();
        self.map.frame_prefix_q32.clear();
        self.map.sample_rate = 0;
        self.phase = TempoMapCopyPhase::Vacant;
        Ok(())
    }
}
#[cfg(test)]
#[path = "copy_custody_tests.rs"]
mod tests;
