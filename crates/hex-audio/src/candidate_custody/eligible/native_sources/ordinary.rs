//! Native-source references attached to the existing ordinary publication.
//! The original graph/return carrier owns this move-only attachment; it never
//! owns a native worker, endpoint, sampler bank or constructor claim.
use super::*;

pub(in crate::candidate_custody::eligible) struct ReferenceCounts {
    capacity: usize,
    graphs: usize,
    sources: [usize; MAX_SOURCES],
}
impl Default for ReferenceCounts {
    fn default() -> Self {
        Self {
            capacity: 0,
            graphs: 0,
            sources: [0; MAX_SOURCES],
        }
    }
}
impl ReferenceCounts {
    pub(in crate::candidate_custody::eligible) fn contains(&self, index: usize) -> bool {
        self.sources.get(index).is_some_and(|count| *count != 0)
    }
}

/// Private, non-cloneable graph attachment. Created only after matching the
/// actual prepared nodes to Root's original loaned pairs under Work. Drop is
/// control-only, after the corresponding graph has been destroyed. Callback
/// handoff moves the whole publication through its existing bounded carriers.
pub(crate) struct OrdinaryNativeReferences {
    root: CandidateRoot,
    references: References,
}
impl Drop for OrdinaryNativeReferences {
    fn drop(&mut self) {
        let mut bank = lock(&self.root.0.eligible.bank);
        let counts = &mut bank.publication.ordinary_native;
        // A broken internal attachment remains fail-closed, never opening a
        // source for reuse after an underflow. No external counter is accepted.
        counts.graphs = counts.graphs.checked_sub(1).unwrap_or(usize::MAX);
        for (index, count) in counts.sources.iter_mut().enumerate() {
            if self.references.contains(index) {
                *count = count.checked_sub(1).unwrap_or(usize::MAX);
            }
        }
        drop(bank);
        crate::lifecycle::request_candidate_cleanup(&self.root);
    }
}
impl CandidateRoot {
    pub(crate) fn configure_ordinary_native_references(
        &self,
        capacity: usize,
    ) -> Result<(), &'static str> {
        let mut bank = lock(&self.0.eligible.bank);
        if capacity == 0 || bank.closing || bank.ordinary.is_none() {
            return Err("ordinary native references require an ordinary source root");
        }
        let counts = &mut bank.publication.ordinary_native;
        if counts.graphs != 0 || (counts.capacity != 0 && counts.capacity != capacity) {
            return Err("ordinary native reference reservation differs");
        }
        counts.capacity = capacity;
        Ok(())
    }
    pub(crate) fn retain_ordinary_native_sources(
        &self,
        playback: &hex_engine::playback::Playback,
    ) -> Result<Option<OrdinaryNativeReferences>, &'static str> {
        if playback.foreign_nodes_on_control().next().is_none() {
            return Ok(None);
        }
        self.require_native_source_control_open()?;
        let work = self
            .0
            .eligible
            .work
            .try_lock()
            .map_err(|_| "native graph preparation is busy")?;
        let mut bank = lock(&self.0.eligible.bank);
        if bank.closing || bank.ordinary.is_none() {
            return Err("ordinary native source root is closing or unavailable");
        }
        let references = bank
            .sources
            .references_for_nodes(playback.foreign_nodes_on_control(), &work.sources)?;
        if references.is_empty() {
            return Ok(None);
        }
        let counts = &mut bank.publication.ordinary_native;
        if counts.graphs >= counts.capacity || counts.graphs == usize::MAX {
            return Err("ordinary native graph reference capacity exceeded");
        }
        // The graph count bounds every per-source count. Validate every row
        // before changing any; synchronous refusal preserves the same graph.
        if counts
            .sources
            .iter()
            .enumerate()
            .any(|(index, count)| references.contains(index) && *count >= counts.capacity)
        {
            return Err("ordinary native source reference capacity exceeded");
        }
        counts.graphs += 1;
        for (index, count) in counts.sources.iter_mut().enumerate() {
            if references.contains(index) {
                *count += 1;
            }
        }
        Ok(Some(OrdinaryNativeReferences {
            root: self.clone(),
            references,
        }))
    }
}
