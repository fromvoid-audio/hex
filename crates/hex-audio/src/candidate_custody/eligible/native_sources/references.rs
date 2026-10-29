//! Scalar references in the existing original runtime/anchor slots. Source
//! cells cannot be reused while these references survive; no owner is cloned.
use super::*;

#[derive(Clone, Copy, Default)]
pub(in crate::candidate_custody::eligible) struct References([u64; MAX_SOURCES.div_ceil(64)]);
impl References {
    pub(in crate::candidate_custody::eligible) fn is_empty(self) -> bool {
        self.0.iter().all(|word| *word == 0)
    }
    pub(in crate::candidate_custody::eligible) fn contains(self, index: usize) -> bool {
        self.0
            .get(index / 64)
            .is_some_and(|word| word & (1 << (index % 64)) != 0)
    }
    fn insert(&mut self, index: usize) -> Result<(), &'static str> {
        let word = self
            .0
            .get_mut(index / 64)
            .ok_or("native source reference overflow")?;
        let bit = 1 << (index % 64);
        if *word & bit != 0 {
            return Err("native source appears more than once in graph");
        }
        *word |= bit;
        Ok(())
    }
}
impl Bank {
    pub(in crate::candidate_custody::eligible) fn graph_references(
        &self,
        owner: &AudioCandidateOwner,
        work: &Work,
    ) -> Result<References, &'static str> {
        self.references_for_nodes(
            owner
                .foreign_nodes_on_control()
                .map_err(|_| "foreign constructor query refused")?,
            work,
        )
    }
    pub(super) fn references_for_nodes<'a>(
        &self,
        nodes: impl Iterator<Item = &'a hex_lattice::PreparedRuntimeInstance>,
        work: &Work,
    ) -> Result<References, &'static str> {
        let mut references = References::default();
        for node in nodes {
            let Some(hex_lattice::PreparedProcessorFactory::Foreign(prepared)) = &node.definition
            else {
                return Err("native graph node differs");
            };
            let binding = prepared
                .binding()
                .ok_or("native graph binding is missing")?;
            let index = self.index(binding).ok_or("native graph source differs")?;
            let slot = &self.slots[index];
            let cell = work
                .cells
                .get(index)
                .ok_or("native graph source owner is missing")?;
            if slot.phase != Phase::Accepted
                || slot.starting
                || slot.retiring
                || slot.cancellation_requested
                || !slot.endpoint_loaned
                || slot
                    .endpoint_return
                    .as_ref()
                    .is_none_or(|s| s.load(Ordering::Acquire))
                || cell.pending
                || cell.first_panic.is_some()
                || cell.cause().is_some()
                || cell.cleanup_error.is_some()
                || cell.disposal.is_some()
            {
                return Err("native graph source is unavailable");
            }
            let Some(Owner::Loaned(control)) = cell.owner.as_ref() else {
                return Err("native graph endpoint is not loaned");
            };
            if !control.transport_is_live_on_control() {
                return Err("native graph source is unavailable");
            }
            if control.constructor_storage_on_control().is_none()
                || slot.identity != Some(control.identity())
                || slot.processor != Some(control.prepared())
                || control.binding() != binding
                || !crate::plugin_effects::NativeEffectReference::Loaned(control)
                    .matches_prepared(node)
            {
                return Err("native graph configuration or original source differs");
            }
            references.insert(index)?;
        }
        Ok(references)
    }
}
