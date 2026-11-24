//! Borrowed physical storage views and preflight byte accounting for `NativeRuntimeChain`.
use super::*;

/// Borrowed physical storage observations, never a provider declaration or
/// ownership/retirement capability. Private chain fields remain inaccessible.
pub struct NativeRuntimeStorage<'a> {
    chain: &'a NativeRuntimeChain,
}
pub struct NativeRuntimeEntryStorage<'a> {
    entry: &'a RuntimeEntry,
}
impl<'a> NativeRuntimeStorage<'a> {
    pub fn entry_capacity_bytes(&self) -> Option<usize> {
        self.chain
            .entries
            .capacity()
            .checked_mul(std::mem::size_of::<RuntimeEntry>())
    }
    pub fn entries(&self) -> impl Iterator<Item = NativeRuntimeEntryStorage<'a>> + use<'a> {
        self.chain
            .entries
            .iter()
            .map(NativeRuntimeEntryStorage::new)
    }
}
impl<'a> NativeRuntimeEntryStorage<'a> {
    fn new(entry: &RuntimeEntry) -> NativeRuntimeEntryStorage<'_> {
        let RuntimeEntry {
            modulation: _,
            automation: _,
            automation_frozen: _,
            committed_host_gains: _,
            scratch_host_gains: _,
            committed_output_gain_ramp: _,
            scratch_output_gain_ramp: _,
            committed_transient_output_gain: _,
            scratch_transient_output_gain: _,
            committed_matrix_ramp: _,
            scratch_matrix_ramp: _,
            meter: _,
            metering: _,
            input_gain: _,
            output_gain: _,
            owner: _,
            device_id: _,
            device_type: _,
            matrix: _,
            bypassed: _,
            powered_off: _,
            removal_bypassing: _,
            runtime_version: _,
            sample_rate: _,
            configuration: _,
            output_tail: _,
            algorithmic_latency_frames: _,
            supports_smoothed_bypass: _,
            supports_power_off: _,
            committed_bypass_ramp: _,
            scratch_bypass_ramp: _,
            committed_aligned_dry: _,
            scratch_aligned_dry: _,
            runtime: _,
        } = entry;
        NativeRuntimeEntryStorage { entry }
    }
    /// Only scalar addresses leave this view. They are observations, not origin
    /// receipts; equal bytes or addresses alone never establish provenance.
    pub fn automation(&self) -> &'a PreparedParameterAutomation {
        &self.entry.automation
    }
    pub fn modulation_storage_bytes(&self) -> Option<usize> {
        self.entry
            .modulation
            .as_ref()
            .map_or(Some(0), ModulationRuntime::heap_storage_bytes)?
            .checked_add(self.entry.runtime.foreign_modulation_bytes()?)
    }
    pub fn runtime_box_identities(&self) -> (Option<usize>, Option<usize>) {
        if let Some((primary, scratch)) = self.entry.runtime.legacy() {
            (
                Some(primary as *const dyn NativeProcessorRuntime as *const () as usize),
                Some(scratch as *const dyn NativeProcessorRuntime as *const () as usize),
            )
        } else if let Some(owned) = self.entry.runtime.owned_identity() {
            (Some(owned), None)
        } else {
            (None, None)
        }
    }
    /// Explicit prepared Off metadata, not absence of a processing source.
    pub fn is_powered_off(&self) -> bool {
        self.entry.powered_off
    }

    /// Owned transactions have a different physical proof and never masquerade
    /// as a pair of legacy boxes. A declaration is not a native custody proof.
    pub fn is_owned_transaction(&self) -> bool {
        matches!(self.entry.runtime, RuntimeOwnership::Owned(_))
    }
    /// Read-only typed view of the actual owned transaction. The caller retains
    /// the closed constructor's independent factory and storage custody proofs.
    pub fn owned_runtime<T: OwnedProcessorRuntime>(&self) -> Option<&T> {
        let RuntimeOwnership::Owned(runtime) = &self.entry.runtime else {
            return None;
        };
        let concrete: &dyn std::any::Any = runtime.as_ref();
        concrete.downcast_ref()
    }

    /// Borrow only actual scalar foreign facts. This transfers no endpoint or
    /// source custody and cannot classify another runtime family as foreign.
    pub fn foreign_processor(&self) -> Option<PreparedForeignProcessor> {
        match &self.entry.runtime {
            RuntimeOwnership::Foreign(value) => Some(value.prepared),
            _ => None,
        }
    }
    /// Borrow actual reserved host histories. This grants no constructor,
    /// source, publication or retirement authority; the chain retains custody.
    pub fn reserved_dry_storage(&self) -> [Option<&'a Vec<StereoFrame>>; 2] {
        [
            self.entry
                .committed_aligned_dry
                .as_ref()
                .and_then(bypass_ramp::AlignedDry::reserved_storage),
            self.entry
                .scratch_aligned_dry
                .as_ref()
                .and_then(bypass_ramp::AlignedDry::reserved_storage),
        ]
    }
    pub fn primary_payload_bytes(&self) -> Option<usize> {
        self.entry
            .runtime
            .legacy()
            .map(|(v, _)| std::mem::size_of_val(v))
    }
    pub fn scratch_payload_bytes(&self) -> Option<usize> {
        self.entry
            .runtime
            .legacy()
            .map(|(_, v)| std::mem::size_of_val(v))
    }
    /// Read-only typed borrow of both actual legacy runtime owners. This never
    /// adopts a declaration/provider, clones history, or establishes native
    /// source custody; closed constructors retain that independent obligation.
    pub fn runtime_pair<T: NativeProcessorRuntime>(&self) -> Option<(&T, &T)> {
        let (primary, scratch) = self.entry.runtime.legacy()?;
        Some((
            primary.as_any().downcast_ref::<T>()?,
            scratch.as_any().downcast_ref::<T>()?,
        ))
    }
    pub fn runtime_types_are<T: NativeProcessorRuntime>(&self) -> bool {
        self.entry
            .runtime
            .legacy()
            .is_some_and(|(a, b)| a.as_any().is::<T>() && b.as_any().is::<T>())
    }
    pub fn matches_prepared_runtime_storage(&self, prepared: &PreparedRuntimeInstance) -> bool {
        (match (
            &self.entry.modulation,
            self.entry.runtime.foreign_modulation(),
            &prepared.modulation,
        ) {
            (Some(runtime), None, Some(declaration)) => {
                declaration.foreign_effect().is_none() && declaration.matches_runtime(runtime)
            }
            (None, Some(runtime), Some(declaration)) => {
                declaration.foreign_effect().is_some_and(|target| {
                    self.foreign_processor()
                        .and_then(PreparedForeignProcessor::binding)
                        == Some(target.binding)
                }) && runtime.matches_prepared(declaration)
            }
            (None, None, None) => true,
            _ => false,
        }) && self.entry.powered_off
            == prepared
                .definition
                .as_ref()
                .is_some_and(PreparedProcessorFactory::is_powered_off)
            && self.entry.automation == prepared.automation
            && self.entry.owner == prepared.owner
            && self.entry.device_id == prepared.device_id
            && self.entry.device_type == prepared.device_type
            && self.entry.sample_rate == prepared.sample_rate
            && std::sync::Arc::ptr_eq(&self.entry.meter, &prepared.meter)
            && match (&self.entry.configuration, &prepared.configuration) {
                (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
    /// Compare against an already borrowed prepared owner. The view never
    /// returns or clones either shared owning field.
    pub fn matches_prepared_storage(&self, prepared: &PreparedProcessorInstance) -> bool {
        self.entry.modulation.is_none()
            && self.entry.runtime.foreign_modulation().is_none()
            && self.entry.owner == prepared.owner
            && self.entry.device_id == prepared.device_id
            && self.entry.device_type == prepared.device_type
            && self.entry.sample_rate == prepared.sample_rate
            && std::sync::Arc::ptr_eq(&self.entry.meter, &prepared.meter)
            && match (&self.entry.configuration, &prepared.configuration) {
                (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl NativeRuntimeChain {
    /// Control-side inventory or bounded scalar test observation only. This
    /// does not authorize concurrent access to an active runtime.
    pub fn storage_view(&self) -> NativeRuntimeStorage<'_> {
        let Self {
            entries: _,
            stateful_count: _,
            declared_pair_bytes: _,
        } = self;
        NativeRuntimeStorage { chain: self }
    }

    /// Account the chain's owned control/audio runtime storage, excluding
    /// allocator bookkeeping and shared prepared metadata (processor
    /// definitions and immutable `NativeDeviceState` payloads). Stateful DSP
    /// payload bytes use each processor's declared fixed storage. The
    /// configuration Arc handle stored in each runtime entry is included in
    /// the entry's `size_of` charge; its separately shared allocation is not.
    pub fn accounted_storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(
                self.entries
                    .capacity()
                    .checked_mul(std::mem::size_of::<RuntimeEntry>())?,
            )?
            .checked_add(self.declared_pair_bytes)?
            .checked_add(self.entries.iter().try_fold(0usize, |sum, entry| {
                sum.checked_add(entry.automation.storage_bytes()?)?
                    .checked_add(
                        entry
                            .modulation
                            .as_ref()
                            .map_or(Some(0), ModulationRuntime::heap_storage_bytes)?,
                    )?
                    .checked_add(entry.runtime.foreign_modulation_bytes()?)
            })?)
    }

    /// Declared payload preflight estimate for building a chain from instances.
    /// Call before runtime factories to reject an aggregate owner over budget.
    /// This assumes the requested entry capacity; allocator bookkeeping and
    /// shared prepared definitions are excluded. Check actual vector capacities
    /// and fixed runtime payload declarations after construction.
    pub fn declared_storage_preflight_bytes_for_instances(
        instances: &[PreparedProcessorInstance],
    ) -> Option<usize> {
        let payloads = instances.iter().try_fold(0_usize, |total, instance| {
            total.checked_add(
                instance
                    .definition
                    .as_ref()
                    .map_or(Some(0), |v| v.declared_runtime_bytes().checked_mul(2))?,
            )
        })?;
        Self::preflight_bytes(instances.len(), payloads)
    }
    pub fn declared_storage_preflight_bytes_for_prepared(
        instances: &[PreparedRuntimeInstance],
    ) -> Option<usize> {
        let payloads = instances.iter().try_fold(0_usize, |total, instance| {
            total.checked_add(
                instance
                    .definition
                    .as_ref()
                    .map_or(Some(0), PreparedProcessorFactory::declared_pair_bytes)?,
            )
        })?;
        let programs = instances.iter().try_fold(0usize, |sum, instance| {
            sum.checked_add(instance.automation.storage_bytes()?)?
                .checked_add(
                    instance
                        .modulation
                        .as_ref()
                        .map_or(Some(0), PreparedModulationState::quoted_runtime_heap_bytes)?,
                )
        })?;
        Self::preflight_bytes(instances.len(), payloads)?.checked_add(programs)
    }
    fn preflight_bytes(count: usize, payloads: usize) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(count.checked_mul(std::mem::size_of::<RuntimeEntry>())?)?
            .checked_add(payloads)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_chain_storage_accounting_and_preflight_match() {
        let chain = NativeRuntimeChain::empty();
        assert_eq!(chain.storage_view().entry_capacity_bytes(), Some(0));
        assert_eq!(chain.storage_view().entries().count(), 0);
        assert_eq!(
            chain.accounted_storage_bytes(),
            NativeRuntimeChain::declared_storage_preflight_bytes_for_instances(&[])
        );
        assert_eq!(
            chain.accounted_storage_bytes(),
            NativeRuntimeChain::declared_storage_preflight_bytes_for_prepared(&[])
        );
    }
}
