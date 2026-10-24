//! Audited fixed inventory owners for the new audio-only compiler. No provider
//! acceptance, raw Arc adoption, completed inventory, or lifetime authority.
use crate::{
    AudioStorageError, SineTable, owned_shared::OwnedShared,
    shared_analytic::SharedAnalyticContextTable,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionSharedCapacity {
    pub sine_payload_bytes: usize,
    pub sine_declared_overhead_bytes: usize,
    pub context_payload_bytes: usize,
    pub context_declared_overhead_bytes: usize,
    pub unresolved_allocations: usize,
    pub reserved_handle_destinations: usize,
    pub installed_handle_obligations: usize,
}
impl SessionSharedCapacity {
    pub(super) fn bytes(self) -> Result<usize, AudioStorageError> {
        [
            self.sine_payload_bytes,
            self.sine_declared_overhead_bytes,
            self.context_payload_bytes,
            self.context_declared_overhead_bytes,
        ]
        .into_iter()
        .try_fold(0_usize, |n, bytes| {
            n.checked_add(bytes).ok_or(AudioStorageError::Overflow)
        })
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Disposal {
    #[default]
    Unstarted,
    Pending,
    Complete,
    Transferred,
}
#[derive(Default)]
pub(super) struct SessionSharedConstruction {
    sine: Option<OwnedShared<SineTable>>,
    arrangement_contexts: Option<OwnedShared<SharedAnalyticContextTable>>,
    session_contexts: Option<OwnedShared<SharedAnalyticContextTable>>,
    // Fixed preallocated destinations and independent physical-handle IDs.
    handles: [u64; 3],
    disposal: [Disposal; 3],
    next: usize,
    #[cfg(test)]
    pub(super) fail_after: Option<usize>,
    #[cfg(test)]
    pub(super) panic_after: Option<usize>,
    #[cfg(test)]
    pub(super) probes: [Option<crate::audio_preparation::DropProbe>; 3],
}
pub(crate) struct InventorySharedView<'a> {
    pub(crate) sine: Option<(
        &'a Arc<SineTable>,
        &'a crate::owned_shared::SharedDeclaration,
    )>,
    pub(crate) contexts: Option<&'a OwnedShared<SharedAnalyticContextTable>>,
}
impl SessionSharedConstruction {
    pub(super) fn reserve(
        &mut self,
        current_handles: usize,
        limit: usize,
    ) -> Result<(), AudioStorageError> {
        self.reserve_with_contexts(
            current_handles,
            limit,
            SharedAnalyticContextTable::default(),
        )
    }
    pub(super) fn reserve_current_sampler(
        &mut self,
        current_handles: usize,
        limit: usize,
        tuning: hex_lattice::TuningTable,
    ) -> Result<(), AudioStorageError> {
        self.reserve_with_contexts(
            current_handles,
            limit,
            SharedAnalyticContextTable::current_sampler_table(tuning)
                .map_err(|_| AudioStorageError::Overflow)?,
        )
    }
    fn reserve_with_contexts(
        &mut self,
        current_handles: usize,
        limit: usize,
        contexts: SharedAnalyticContextTable,
    ) -> Result<(), AudioStorageError> {
        if self.sine.is_some()
            || self.arrangement_contexts.is_some()
            || self.session_contexts.is_some()
            || self.next != 0
            || self.handles != [0; 3]
        {
            return Err(AudioStorageError::Capacity);
        }
        if current_handles
            .checked_add(3)
            .ok_or(AudioStorageError::Overflow)?
            > limit
        {
            return Err(AudioStorageError::Capacity);
        }
        if !crate::pcm_arc::native_layout_known() {
            return Err(AudioStorageError::UnsupportedConstructor);
        }
        // All destinations/checked handle obligations precede either allocation.
        for handle in &mut self.handles {
            *handle = crate::audio_only_inputs::handle()?;
        }
        #[cfg(test)]
        self.inject(0)?;
        // Audited hex-devices::SineTable consists solely of inline
        // samples:[f32;TABLE_SIZE+1]. Its constructor creates no nested owner.
        // This is a version-bound requested-layout declaration, not RSS.
        self.sine = Some(OwnedShared::new_owned(SineTable::new()));
        #[cfg(test)]
        self.inject(1)?;
        // The owning module's exhaustive check refuses ANY nested/spare owner.
        self.arrangement_contexts = Some(OwnedShared::new_owned(contexts));
        #[cfg(test)]
        self.inject(2)?;
        self.observe()?;
        Ok(())
    }
    pub(crate) fn current_sampler_contexts(
        &self,
    ) -> Result<&OwnedShared<SharedAnalyticContextTable>, AudioStorageError> {
        self.arrangement_contexts
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)
    }
    pub(super) fn construct(&mut self) -> Result<(), AudioStorageError> {
        if self.session_contexts.is_some()
            || self.handles.contains(&0)
            || self.disposal != [Disposal::Unstarted; 3]
        {
            return Err(AudioStorageError::Capacity);
        }
        let contexts = self
            .arrangement_contexts
            .as_ref()
            .ok_or(AudioStorageError::UnknownStorage)?;
        // Source+destination+extra independent handle were reserved before clone.
        self.session_contexts = Some(contexts.clone_on_control());
        #[cfg(test)]
        self.inject(3)?;
        Ok(())
    }
    #[cfg(test)]
    fn inject(&self, phase: usize) -> Result<(), AudioStorageError> {
        assert_ne!(
            self.panic_after,
            Some(phase),
            "persistent fixed inventory construction panic"
        );
        if self.fail_after == Some(phase) {
            return Err(AudioStorageError::Allocation);
        }
        Ok(())
    }
    pub(super) fn observe(&self) -> Result<SessionSharedCapacity, AudioStorageError> {
        self.observe_at(
            None,
            InventorySharedView {
                sine: None,
                contexts: None,
            },
        )
    }
    pub(crate) fn observe_at(
        &self,
        arrangement: Option<&OwnedShared<SharedAnalyticContextTable>>,
        inventory: InventorySharedView<'_>,
    ) -> Result<SessionSharedCapacity, AudioStorageError> {
        if (self.arrangement_contexts.is_some() && arrangement.is_some())
            || (self.session_contexts.is_some() && inventory.contexts.is_some())
            || (self.sine.is_some() && inventory.sine.is_some())
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let mut report = SessionSharedCapacity {
            reserved_handle_destinations: self.handles.iter().filter(|id| **id != 0).count(),
            ..Default::default()
        };
        if (self.sine.is_some()
            || self.arrangement_contexts.is_some()
            || self.session_contexts.is_some()
            || arrangement.is_some()
            || inventory.contexts.is_some()
            || inventory.sine.is_some())
            && (self.handles.contains(&0)
                || self.handles[0] == self.handles[1]
                || self.handles[0] == self.handles[2]
                || self.handles[1] == self.handles[2])
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        if let Some(sine) = &self.sine {
            let (payload, overhead) = sine.parts().ok_or(AudioStorageError::UnknownStorage)?;
            if payload != std::mem::size_of::<SineTable>() {
                return Err(AudioStorageError::UnknownStorage);
            }
            report.sine_payload_bytes = payload;
            report.sine_declared_overhead_bytes = overhead;
            report.unresolved_allocations += 1;
            report.installed_handle_obligations += 1;
        }
        if let Some((sine, declaration)) = inventory.sine {
            let (payload, overhead) = declaration
                .parts(sine)
                .ok_or(AudioStorageError::UnknownStorage)?;
            if payload != std::mem::size_of::<SineTable>() {
                return Err(AudioStorageError::UnknownStorage);
            }
            report.sine_payload_bytes = payload;
            report.sine_declared_overhead_bytes = overhead;
            report.unresolved_allocations += 1;
            report.installed_handle_obligations += 1;
        }
        let mut previous: Option<&OwnedShared<SharedAnalyticContextTable>> = None;
        for contexts in [
            self.arrangement_contexts.as_ref().or(arrangement),
            self.session_contexts.as_ref().or(inventory.contexts),
        ]
        .into_iter()
        .flatten()
        {
            contexts.require_empty_owned_storage()?;
            let (payload, overhead) = contexts.parts().ok_or(AudioStorageError::UnknownStorage)?;
            if let Some(previous) = previous {
                if !Arc::ptr_eq(previous.arc(), contexts.arc())
                    || previous.parts() != contexts.parts()
                {
                    return Err(AudioStorageError::UnknownStorage);
                }
            } else {
                report.context_payload_bytes = payload;
                report.context_declared_overhead_bytes = overhead;
                report.unresolved_allocations += 1;
                previous = Some(contexts);
            }
            report.installed_handle_obligations += 1;
        }
        report.bytes()?;
        Ok(report)
    }
    pub(super) fn has_next_disposal(&self) -> bool {
        self.next < 3
    }
    /// Borrowed caller catch surrounds this operation. Pending BEFORE take;
    /// panic leaves all remaining fields and exact failure location persistent.
    pub(super) fn dispose_next(&mut self) -> Result<(), AudioStorageError> {
        if self.next >= 3
            || !matches!(
                self.disposal[self.next],
                Disposal::Unstarted | Disposal::Transferred
            )
        {
            return Err(AudioStorageError::Capacity);
        }
        self.disposal[self.next] = Disposal::Pending;
        match self.next {
            0 => drop(self.sine.take()),
            1 => drop(self.arrangement_contexts.take()),
            _ => drop(self.session_contexts.take()),
        }
        #[cfg(test)]
        drop(self.probes[self.next].take());
        self.disposal[self.next] = Disposal::Complete;
        self.next += 1;
        Ok(())
    }
    pub(super) fn clear_disposed(&mut self) -> Result<(), AudioStorageError> {
        if self.sine.is_some()
            || self.arrangement_contexts.is_some()
            || self.session_contexts.is_some()
            || self.disposal != [Disposal::Complete; 3]
            || self.next != 3
        {
            return Err(AudioStorageError::Capacity);
        }
        self.handles = [0; 3];
        self.disposal = [Disposal::Unstarted; 3];
        self.next = 0;
        Ok(())
    }
    pub(crate) fn preflight_arrangement_transfer(
        &self,
        target: &Option<OwnedShared<SharedAnalyticContextTable>>,
    ) -> Result<(), AudioStorageError> {
        if target.is_some()
            || self.arrangement_contexts.is_none()
            || self.disposal[1] != Disposal::Unstarted
            || self.handles[1] == 0
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.arrangement_contexts
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?
            .require_empty_owned_storage()?;
        Ok(())
    }
    pub(crate) fn commit_arrangement_transfer(
        &mut self,
        target: &mut Option<OwnedShared<SharedAnalyticContextTable>>,
    ) {
        *target = self.arrangement_contexts.take();
        self.disposal[1] = Disposal::Transferred;
    }
    pub(crate) fn preflight_arrangement_restore(&self) -> Result<(), AudioStorageError> {
        if self.arrangement_contexts.is_some()
            || self.disposal[1] != Disposal::Transferred
            || self.next != 0
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    pub(crate) fn restore_arrangement(
        &mut self,
        source: &mut Option<OwnedShared<SharedAnalyticContextTable>>,
    ) {
        if source.is_some() {
            self.arrangement_contexts = source.take();
            self.disposal[1] = Disposal::Unstarted;
        }
    }
    pub(crate) fn inventory_handles(&self) -> [u64; 3] {
        self.handles
    }
    pub(crate) fn preflight_inventory_transfer(
        &self,
        handles: [u64; 3],
    ) -> Result<(), AudioStorageError> {
        if handles != self.handles
            || self.observe()?.installed_handle_obligations != 3
            || self.disposal != [Disposal::Unstarted; 3]
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    pub(crate) fn commit_inventory_transfer(
        &mut self,
        sine: &mut Option<Arc<SineTable>>,
        declaration: &mut Option<crate::owned_shared::SharedDeclaration>,
        contexts: &mut Option<OwnedShared<SharedAnalyticContextTable>>,
    ) -> bool {
        if sine.is_some()
            || declaration.is_some()
            || contexts.is_some()
            || self.sine.is_none()
            || self.session_contexts.is_none()
        {
            return false;
        }
        if let Some(owner) = self.sine.take() {
            let (arc, receipt) = owner.into_paired_parts_on_control();
            *declaration = Some(receipt);
            *sine = Some(arc);
        }
        *contexts = self.session_contexts.take();
        self.disposal[0] = Disposal::Transferred;
        self.disposal[2] = Disposal::Transferred;
        true
    }
    #[cfg(test)]
    pub(super) fn handle_ids(&self) -> [u64; 3] {
        self.handles
    }
    #[cfg(test)]
    pub(super) fn context_strong_count(&self) -> usize {
        self.arrangement_contexts
            .as_ref()
            .or(self.session_contexts.as_ref())
            .map_or(0, |contexts| Arc::strong_count(contexts.arc()))
    }
    #[cfg(test)]
    pub(super) fn same_context_allocation(&self, other: &Self) -> bool {
        match (&self.arrangement_contexts, &other.arrangement_contexts) {
            (Some(left), Some(right)) => Arc::ptr_eq(left.arc(), right.arc()),
            _ => false,
        }
    }
}
impl SessionSharedConstruction {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
        arrangement: Option<&'a OwnedShared<SharedAnalyticContextTable>>,
        inventory: InventorySharedView<'a>,
    ) -> Result<(), AudioStorageError> {
        use crate::audio_storage::Owner;
        self.observe_at(
            arrangement,
            InventorySharedView {
                sine: inventory.sine,
                contexts: inventory.contexts,
            },
        )?;
        let Self {
            sine,
            arrangement_contexts,
            session_contexts,
            handles,
            disposal: _,
            next: _,
            #[cfg(test)]
                fail_after: _,
            #[cfg(test)]
                panic_after: _,
            #[cfg(test)]
                probes: _,
        } = self;
        if let Some(owner) = sine {
            if owner.control_storage_declaration()
                != (hex_lattice::PreparedStorageDeclaration::ExclusiveOnly {
                    bytes: std::mem::size_of::<SineTable>(),
                })
            {
                return Err(AudioStorageError::UnknownStorage);
            }
            l.shared(
                owner.arc(),
                owner.parts(),
                Owner::exclusive(owner),
                handles[0],
                None,
            )?;
        }
        if let Some((owner, declaration)) = inventory.sine {
            if owner.control_storage_declaration()
                != (hex_lattice::PreparedStorageDeclaration::ExclusiveOnly {
                    bytes: std::mem::size_of::<SineTable>(),
                })
            {
                return Err(AudioStorageError::UnknownStorage);
            }
            l.shared(
                owner,
                declaration.parts(owner),
                Owner::exclusive(owner),
                handles[0],
                None,
            )?;
        }
        for (owner, id) in [
            arrangement_contexts.as_ref().or(arrangement),
            session_contexts.as_ref().or(inventory.contexts),
        ]
        .into_iter()
        .zip([handles[1], handles[2]])
        {
            if let Some(owner) = owner {
                owner.require_empty_owned_storage()?;
                l.shared(
                    owner.arc(),
                    owner.parts(),
                    Owner::exclusive(owner),
                    id,
                    None,
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_same_allocation_ingress_still_has_unknown_declaration_and_no_complete_receipt() {
        if !crate::pcm_arc::native_layout_known() {
            let mut unsupported = SessionSharedConstruction::default();
            assert_eq!(
                unsupported.reserve(0, 3),
                Err(AudioStorageError::UnsupportedConstructor)
            );
            assert_eq!(unsupported.handles, [0; 3]);
            eprintln!(
                "hex Session preparation evidence: shared_raw_origin; branch=unsupported-refusal"
            );
            return;
        }
        let mut custody = SessionSharedConstruction::default();
        crate::owned_shared::reset_clone_count();
        custody.reserve(0, 3).unwrap();
        assert_eq!(crate::owned_shared::clone_count(), 0);
        custody.construct().unwrap();
        assert_eq!(crate::owned_shared::clone_count(), 1); // actual positive control

        assert_eq!(custody.observe().unwrap().unresolved_allocations, 2);
        let sine = custody.sine.as_ref().unwrap();
        let raw = OwnedShared::untracked(Arc::clone(sine.arc()));
        assert!(Arc::ptr_eq(raw.arc(), sine.arc()));
        custody.sine = Some(raw); // actual test-only raw ingress, no origin upgrade
        assert_eq!(custody.observe(), Err(AudioStorageError::UnknownStorage));
        eprintln!(
            "hex Session preparation evidence: shared_raw_origin; supported-controls=completed"
        );
    }
}
