//! Persistent canonical-tempo assembly, not a prepared inventory or admission.
//! The canonical destination stays private until full inventory assembly exists.
use crate::{AudioStorageError, PreparedTempoMap};
use hex_project::{Project, TempoMap, TempoMapCopyCustody, TempoMapCopyError};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionTempoCapacity {
    pub copy_point_capacity: usize,
    pub copy_prefix_capacity: usize,
    pub copy_backing_bytes: usize,
    pub completed_point_capacity: usize,
    pub completed_prefix_capacity: usize,
    pub completed_backing_bytes: usize,
    pub previous_growth_backing_bytes: usize,
    /// Independent unresolved exclusive allocations; transfer changes their
    /// owning location, not their lifetime status or data charge.
    pub unresolved_owned_allocations: usize,
}
impl SessionTempoCapacity {
    pub(super) fn bytes(self) -> Result<usize, AudioStorageError> {
        self.copy_backing_bytes
            .checked_add(self.completed_backing_bytes)
            .ok_or(AudioStorageError::Overflow)
    }
}
#[derive(Default)]
pub(super) struct SessionTempoConstruction {
    copy: TempoMapCopyCustody,
    // This already allocated inline destination is private persistent custody.
    // Do not call legacy PreparedTempoMap::prepare with its consumed owner.
    pub(super) canonical: Option<TempoMap>,
    max_micro_bpm: u32,
    #[cfg(test)]
    pub(super) fail_after_copy: bool,
    #[cfg(test)]
    pub(super) panic_after_copy: bool,
}
impl SessionTempoConstruction {
    pub(super) fn reserve(&mut self, project: &Project) -> Result<(), AudioStorageError> {
        if self.canonical.is_some() {
            return Err(AudioStorageError::Capacity);
        }
        let maximum =
            PreparedTempoMap::validate_canonical(project.tempo_map(), project.sample_rate())
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
        self.copy.prepare_from(project.tempo_map()).map_err(error)?;
        // Both private destinations were vacant before copying. No canonical
        // owner has moved yet; all byte/peak checks still precede bank movement.
        self.max_micro_bpm = maximum;
        #[cfg(test)]
        {
            assert!(
                !self.panic_after_copy,
                "persistent engine tempo construction panic"
            );
            if self.fail_after_copy {
                return Err(AudioStorageError::Allocation);
            }
        }
        Ok(())
    }
    pub(super) fn construct(&mut self) -> Result<(), AudioStorageError> {
        // transfer_into preflights phase and destination before either move.
        // No fallible validation/conversion consumes the resulting TempoMap.
        if self.max_micro_bpm == 0 {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.copy.transfer_into(&mut self.canonical).map_err(error)
    }
    pub(super) fn incomplete(&self) -> bool {
        self.copy.phase() == hex_project::TempoMapCopyPhase::Incomplete
    }
    pub(super) fn cleanup(&mut self) -> Result<(), AudioStorageError> {
        if self.canonical.is_some() {
            self.copy
                .clean_transferred_on_control(&mut self.canonical)
                .map_err(error)?;
        } else {
            self.copy.clean_on_control().map_err(error)?;
        }
        self.max_micro_bpm = 0;
        Ok(())
    }
    pub(super) fn observe(
        &self,
        current_growth: bool,
    ) -> Result<SessionTempoCapacity, AudioStorageError> {
        let copy = self.copy.capacity();
        let mut report = SessionTempoCapacity {
            copy_point_capacity: copy.layout.point_capacity(),
            copy_prefix_capacity: copy.layout.prefix_capacity(),
            copy_backing_bytes: copy
                .layout
                .checked_backing_bytes()
                .ok_or(AudioStorageError::Overflow)?,
            previous_growth_backing_bytes: if current_growth {
                copy.previous_growth_backing_bytes
            } else {
                0
            },
            ..SessionTempoCapacity::default()
        };
        if let Some(map) = &self.canonical {
            let layout = map.owned_allocation_layout();
            report.completed_point_capacity = layout.point_capacity();
            report.completed_prefix_capacity = layout.prefix_capacity();
            report.completed_backing_bytes = layout
                .checked_backing_bytes()
                .ok_or(AudioStorageError::Overflow)?;
        }
        let mut observed_bytes = 0_usize;
        self.visit_owners(|owner| {
            observed_bytes = observed_bytes
                .checked_add(owner.bytes()?)
                .ok_or(AudioStorageError::Overflow)?;
            report.unresolved_owned_allocations = report
                .unresolved_owned_allocations
                .checked_add(1)
                .ok_or(AudioStorageError::Overflow)?;
            Ok(())
        })?;
        if observed_bytes != report.bytes()? {
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(report)
    }
    pub(super) fn visit_owners<'a>(
        &'a self,
        mut visitor: impl FnMut(TempoOwner<'a>) -> Result<(), AudioStorageError>,
    ) -> Result<(), AudioStorageError> {
        for location in [
            TempoLocation::CopyPoints,
            TempoLocation::CopyPrefixes,
            TempoLocation::CompletedPoints,
            TempoLocation::CompletedPrefixes,
        ] {
            let owner = TempoOwner {
                anchor: self,
                location,
            };
            if owner.bytes()? != 0 {
                visitor(owner)?;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum TempoLocation {
    CopyPoints,
    CopyPrefixes,
    CompletedPoints,
    CompletedPrefixes,
}
/// Lifetime-bound exact owning field/charge location, not a persistent address,
/// data-content identity, or disposal/retirement receipt. A partial visitor sink
/// must be discarded on error. No traversal of a mutable active runtime occurs.
pub(super) struct TempoOwner<'a> {
    anchor: &'a SessionTempoConstruction,
    location: TempoLocation,
}
impl TempoOwner<'_> {
    pub(super) fn bytes(&self) -> Result<usize, AudioStorageError> {
        let copy = self.anchor.copy.capacity().layout;
        let completed = self
            .anchor
            .canonical
            .as_ref()
            .map(TempoMap::owned_allocation_layout);
        let bytes = match self.location {
            TempoLocation::CopyPoints => copy.point_bytes(),
            TempoLocation::CopyPrefixes => copy.prefix_bytes(),
            TempoLocation::CompletedPoints => {
                completed.map_or(Some(0), |layout| layout.point_bytes())
            }
            TempoLocation::CompletedPrefixes => {
                completed.map_or(Some(0), |layout| layout.prefix_bytes())
            }
        };
        bytes.ok_or(AudioStorageError::Overflow)
    }
    #[cfg(test)]
    pub(super) fn same_owner(&self, other: &Self) -> bool {
        std::ptr::eq(self.anchor, other.anchor) && self.location == other.location
    }
}

fn error(error: TempoMapCopyError) -> AudioStorageError {
    match error {
        TempoMapCopyError::Occupied => AudioStorageError::Capacity,
        TempoMapCopyError::Allocation => AudioStorageError::Allocation,
        TempoMapCopyError::Overflow => AudioStorageError::Overflow,
        TempoMapCopyError::Incomplete => AudioStorageError::UnknownStorage,
    }
}
