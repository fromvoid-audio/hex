//! Persistent prepared/periodic tempo assembly, not a prepared inventory or admission.
//! The canonical destination stays private until full inventory assembly exists.
use crate::prepared_tempo::{PreparedPeriodicSessionTempo, PreparedTempoError};
use crate::{AudioStorageError, MidiTiming, PreparedTempoMap};
use hex_project::{Project, TempoMap, TempoMapCopyCustody, TempoMapCopyError};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionTempoCapacity {
    pub copy_point_capacity: usize,
    pub copy_prefix_capacity: usize,
    pub copy_backing_bytes: usize,
    /// Validated canonical intermediary, not a completed graph.
    pub completed_point_capacity: usize,
    pub completed_prefix_capacity: usize,
    pub completed_backing_bytes: usize,
    pub prepared_point_capacity: usize,
    pub prepared_prefix_capacity: usize,
    pub prepared_backing_bytes: usize,
    pub previous_growth_backing_bytes: usize,
    /// Independent unresolved exclusive allocations; transfer changes their
    /// owning location, not their lifetime status or data charge.
    pub unresolved_owned_allocations: usize,
}
impl SessionTempoCapacity {
    pub(super) fn bytes(self) -> Result<usize, AudioStorageError> {
        self.copy_backing_bytes
            .checked_add(self.completed_backing_bytes)
            .and_then(|n| n.checked_add(self.prepared_backing_bytes))
            .ok_or(AudioStorageError::Overflow)
    }
}
#[derive(Default)]
pub(super) struct SessionTempoConstruction {
    copy: TempoMapCopyCustody,
    // This already allocated inline destination is private persistent custody.
    // Do not call legacy PreparedTempoMap::prepare with its consumed owner.
    pub(super) canonical: Option<TempoMap>,
    inventory_owns_map: bool,
    max_micro_bpm: u32,
    expected_rate: u32,
    loop_region: Option<hex_project::LoopRegion>,
    pub(super) prepared: Option<PreparedTempoMap>,
    pub(super) periodic: Option<PreparedPeriodicSessionTempo>,
    pub(super) periodic_error: Option<PreparedTempoError>,
    pub(super) source_admission_max_micro_bpm: u64,
    #[cfg(test)]
    pub(super) fail_before_install: bool,
    #[cfg(test)]
    pub(super) panic_after_install: bool,
    #[cfg(test)]
    pub(super) fail_after_copy: bool,
    #[cfg(test)]
    pub(super) panic_after_copy: bool,
}
impl SessionTempoConstruction {
    /// Cleanup destination state only, never a fresh map's origin proof.
    pub(crate) fn inventory_map_is_outstanding(&self) -> bool {
        self.inventory_owns_map
    }
    pub(super) fn reserve(&mut self, project: &Project) -> Result<(), AudioStorageError> {
        if self.canonical.is_some() || self.prepared.is_some() {
            return Err(AudioStorageError::Capacity);
        }
        let maximum =
            PreparedTempoMap::validate_canonical(project.tempo_map(), project.sample_rate())
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
        self.copy.prepare_from(project.tempo_map()).map_err(error)?;
        // Both private destinations were vacant before copying. No canonical
        // owner has moved yet; all byte/peak checks still precede bank movement.
        self.max_micro_bpm = maximum;
        self.expected_rate = project.sample_rate();
        self.loop_region = project.loop_region();
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
        if self.prepared.is_some()
            || self.canonical.is_some()
            || self.copy.phase() != hex_project::TempoMapCopyPhase::Validated
        {
            return Err(AudioStorageError::Capacity);
        }
        self.copy
            .transfer_into(&mut self.canonical)
            .map_err(error)?;
        #[cfg(test)]
        if self.fail_before_install {
            return Err(AudioStorageError::Allocation);
        }
        // Validation errors retain the canonical owner in its persistent slot.
        // Never pass it to the legacy consuming fallible constructor.
        PreparedTempoMap::install_owned_on_control(
            &mut self.canonical,
            &mut self.prepared,
            self.expected_rate,
        )?;
        #[cfg(test)]
        assert!(!self.panic_after_install, "persistent prepared tempo panic");
        let map = self
            .prepared
            .as_ref()
            .ok_or(AudioStorageError::UnknownStorage)?;
        let periodic = self
            .loop_region
            .filter(|region| region.enabled())
            .map(|region| PreparedPeriodicSessionTempo::prepare(map, region));
        // Match legacy semantics: invalid periodic conversion remains explicit,
        // while source admission uses canonical tempo rather than fake success.
        self.periodic_error = periodic.and_then(Result::err);
        self.periodic = periodic.and_then(Result::ok);
        self.source_admission_max_micro_bpm = self
            .periodic
            .map_or(u64::from(map.max_micro_bpm()), |cycle| {
                cycle.view(map).max_micro_bpm()
            });
        Ok(())
    }
    pub(crate) fn preflight_inventory_transfer(&self) -> Result<(), AudioStorageError> {
        if self.inventory_owns_map
            || self.prepared.is_none()
            || self.canonical.is_some()
            || self.copy.phase() != hex_project::TempoMapCopyPhase::Transferred
        {
            return Err(AudioStorageError::Capacity);
        }
        Ok(())
    }
    pub(crate) fn commit_inventory_transfer(
        &mut self,
        destination: &mut Option<PreparedTempoMap>,
    ) -> bool {
        if destination.is_some() || self.prepared.is_none() || self.inventory_owns_map {
            return false;
        }
        *destination = self.prepared.take();
        self.inventory_owns_map = true;
        true
    }
    pub(crate) fn restore_inventory_map(
        &mut self,
        source: &mut Option<PreparedTempoMap>,
    ) -> Result<(), AudioStorageError> {
        if !self.inventory_owns_map
            || self.prepared.is_some()
            || self.canonical.is_some()
            || source.is_none()
            || self.copy.phase() != hex_project::TempoMapCopyPhase::Transferred
        {
            return Err(AudioStorageError::Capacity);
        }
        self.prepared = source.take();
        self.inventory_owns_map = false;
        Ok(())
    }
    pub(super) fn incomplete(&self) -> bool {
        self.copy.phase() == hex_project::TempoMapCopyPhase::Incomplete
    }
    pub(super) fn cleanup(&mut self) -> Result<(), AudioStorageError> {
        if self.inventory_owns_map {
            return Err(AudioStorageError::Capacity);
        }
        if self.prepared.is_some() {
            if self.canonical.is_some()
                || self.copy.phase() != hex_project::TempoMapCopyPhase::Transferred
            {
                return Err(AudioStorageError::Capacity);
            }
            PreparedTempoMap::return_canonical_on_control(&mut self.prepared, &mut self.canonical)?;
        }
        if self.canonical.is_some() {
            self.copy
                .clean_transferred_on_control(&mut self.canonical)
                .map_err(error)?;
        } else {
            self.copy.clean_on_control().map_err(error)?;
        }
        self.max_micro_bpm = 0;
        self.expected_rate = 0;
        self.loop_region = None;
        self.periodic = None;
        self.periodic_error = None;
        self.source_admission_max_micro_bpm = 0;
        Ok(())
    }
    pub(super) fn observe(
        &self,
        current_growth: bool,
    ) -> Result<SessionTempoCapacity, AudioStorageError> {
        // Exhaustive field inventory: all three possible map owners are visited
        // below. Remaining production fields are inline Copy metadata. Fault
        // controls are test-only booleans; no added owner may silently vanish.
        let Self {
            copy: _,
            canonical: _,
            prepared: _,
            inventory_owns_map: _,
            max_micro_bpm: _,
            expected_rate: _,
            loop_region: _,
            periodic: _,
            periodic_error: _,
            source_admission_max_micro_bpm: _,
            #[cfg(test)]
                fail_before_install: _,
            #[cfg(test)]
                panic_after_install: _,
            #[cfg(test)]
                fail_after_copy: _,
            #[cfg(test)]
                panic_after_copy: _,
        } = self;
        // Scalar metadata never authenticates ownership. Impossible combinations
        // remain Unknown; a partially installed owner may still be observed.
        if (self.periodic.is_some() && self.periodic_error.is_some())
            || (self.source_admission_max_micro_bpm != 0
                && self.prepared.is_none()
                && !self.inventory_owns_map)
        {
            return Err(AudioStorageError::UnknownStorage);
        }
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
        if let Some(map) = &self.prepared {
            let layout = map.canonical_allocation_layout();
            report.prepared_point_capacity = layout.point_capacity();
            report.prepared_prefix_capacity = layout.prefix_capacity();
            report.prepared_backing_bytes = layout
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
            TempoLocation::PreparedPoints,
            TempoLocation::PreparedPrefixes,
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
    PreparedPoints,
    PreparedPrefixes,
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
        let prepared = self
            .anchor
            .prepared
            .as_ref()
            .map(PreparedTempoMap::canonical_allocation_layout);
        let bytes = match self.location {
            TempoLocation::CopyPoints => copy.point_bytes(),
            TempoLocation::CopyPrefixes => copy.prefix_bytes(),
            TempoLocation::CompletedPoints => {
                completed.map_or(Some(0), |layout| layout.point_bytes())
            }
            TempoLocation::CompletedPrefixes => {
                completed.map_or(Some(0), |layout| layout.prefix_bytes())
            }
            TempoLocation::PreparedPoints => {
                prepared.map_or(Some(0), |layout| layout.point_bytes())
            }
            TempoLocation::PreparedPrefixes => {
                prepared.map_or(Some(0), |layout| layout.prefix_bytes())
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

impl SessionTempoConstruction {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        use crate::audio_storage::{Location, Owner};
        let Self {
            copy,
            canonical,
            inventory_owns_map: _,
            max_micro_bpm: _,
            expected_rate: _,
            loop_region: _,
            prepared,
            periodic: _,
            periodic_error: _,
            source_admission_max_micro_bpm: _,
            #[cfg(test)]
                fail_before_install: _,
            #[cfg(test)]
                panic_after_install: _,
            #[cfg(test)]
                fail_after_copy: _,
            #[cfg(test)]
                panic_after_copy: _,
        } = self;
        let c = copy.capacity();
        l.allocation(
            Owner::exclusive(copy),
            Location("tempo-copy-points", 0),
            c.layout.point_capacity(),
            c.layout.point_element_bytes(),
        )?;
        l.allocation(
            Owner::exclusive(copy),
            Location("tempo-copy-prefix", 0),
            c.layout.prefix_capacity(),
            c.layout.prefix_element_bytes(),
        )?;
        l.growth(c.previous_growth_backing_bytes)?;
        if copy.phase() == hex_project::TempoMapCopyPhase::Incomplete {
            l.unknown()?;
        }
        if let Some(map) = canonical {
            l.map(map)?;
        }
        if let Some(map) = prepared {
            map.visit_audio_storage(l)?;
        }
        Ok(())
    }
}
