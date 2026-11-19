//! Sealed native witness distinct from the public scalar byte ledger. Known
//! DATA and constructor-bounded opaque RESOURCE lifetimes are never conflated.
use super::*;
#[derive(Clone, Copy)]
pub(super) struct RootResourceWitness {
    root: u64,
    root_data: usize,
    root_shared_groups: usize,
    shared_handle_bound: usize,
    lifecycle_workers: usize,
    analysis_workers: usize,
    output_contexts: usize,
    native_workers: usize,
    native_children: usize,
    native_ipc: usize,
    source_workers: usize,
    catalog_channels: usize,
    source_readers: usize,
    kernel_handles: usize,
}
impl RootResourceWitness {
    pub(super) fn covers(&self, root: u64) -> bool {
        self.root == root && self.root_data != 0 && self.root_shared_groups != 0
            && self.shared_handle_bound >= self.root_shared_groups
            && self.lifecycle_workers == 1 && self.output_contexts == 1 && self.analysis_workers <= 1
            && self.native_workers <= hex_lattice::MAX_STATEFUL_PROCESSORS
            && self.native_children <= hex_lattice::MAX_STATEFUL_PROCESSORS
            && self.native_ipc.checked_add(self.native_children).is_some()
            // Two N2 domains each retain one ordinary worker and, when
            // granular sources are present, one dedicated rendering worker.
            && self.catalog_channels <= 2
            && (self.catalog_channels..=2 * self.catalog_channels).contains(&self.source_workers)
            // Counts are retained until actual source-pair/report closure;
            // zero sources legitimately need no reader or kernel handles.
            && self.source_readers.checked_add(self.kernel_handles).is_some()
    }
}
fn root_handle_bound(bank: &NativeBank) -> Option<usize> {
    // Root handles: 2 per accepted State + at most8 control/callback/terminal,
    // queue/lifecycle/construction handles in this private factory. Life:3 per
    // reserved cell (Root,current,spare-or-covering). Rings:6; cells:4; Status:6; two domain signals:4.
    // Six auxiliary rings and private channel atomic payload handles come from
    // the actual callback constructor visitor, independently of queue occupancy.
    // Observer Status handles outside this publication Root own no graph or
    // source lease; no process-wide Arc/RSS bound is claimed.
    let insert_receipts = if bank.publication.stream_extents.is_some() {
        bank.publication.slots.len().checked_add(1)?
    } else {
        0
    };
    bank.publication
        .slots
        .len()
        .checked_mul(5)?
        .checked_add(28)?
        .checked_add(insert_receipts)?
        .checked_add(
            bank.native
                .as_ref()
                .map_or(0, |native| native.channel_resources().1),
        )
}
fn occupied_handles(bank: &NativeBank, excluded_preparation: Option<usize>) -> Option<usize> {
    let mut handles = root_handle_bound(bank)?;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        handles = handles.checked_add(bank.sources.reserved_shared_handles())?;
    }

    for domain in &bank.domains {
        if let Some(resources) = domain.resources {
            handles = handles.checked_add(resources.shared_handle_bound)?;
        } else if let Some(bound) = domain.preparation_bound {
            handles = handles.checked_add(bound.shared_handle_bound)?;
        }
    }
    for (index, slot) in bank.publication.slots.iter().enumerate() {
        if let Some(reservation) = slot
            .preparation_reservation
            .filter(|_| Some(index) != excluded_preparation)
        {
            handles = handles.checked_add(reservation.handle_rows)?;
        }
        if let Some(summary) = slot.summary {
            handles = handles.checked_add(summary.shared_resource_handles)?;
        }
    }
    Some(handles)
}
pub(in crate::candidate_custody::eligible) fn remaining_handle_reservation(
    bank: &NativeBank,
) -> Option<usize> {
    let Some(reservation) = bank.publication.reservation else {
        return Some(usize::MAX);
    };
    reservation
        .handle_rows
        .checked_sub(occupied_handles(bank, None)?)
}
pub(super) fn for_candidate(
    bank: &NativeBank,
    summary: AudioCandidateConstructorSummary,
    excluded_preparation: Option<usize>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))] source_work: &native_sources::Work,
) -> Result<Option<RootResourceWitness>, &'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let native_sources = bank
        .sources
        .inventory(source_work)
        .ok_or("native source construction is not sealed")?;
    if bank
        .domains
        .iter()
        .any(|slot| slot.phase != Phase::Vacant && slot.resources.is_none())
    {
        return Err("source construction is not sealed");
    }
    if summary.shared_resource_handles
        > bank
            .publication
            .reservation
            .and_then(|r| {
                r.handle_rows
                    .checked_sub(occupied_handles(bank, excluded_preparation)?)
            })
            .ok_or("constructor handle reservation exceeded")?
    {
        return Err("constructor handle reservation exceeded");
    }
    let Some(native) = bank
        .native
        .as_ref()
        .filter(|native| native.matches(bank.root))
    else {
        // Isolated disposal-only owning-boundary tests may reserve and return
        // without native carriers. They gain no native claim or active authority.
        return Ok(None);
    };
    native
        .data_bytes()
        .ok_or("native terminal custody is not anchored")?;
    let mut witness = RootResourceWitness {
        root: bank.root,
        root_data: bank.metadata_bytes,
        // Actual Root, per-slot Life,3 rings,2 carrier cells and Status payload.
        root_shared_groups: bank
            .publication
            .slots
            .len()
            .checked_add(9)
            .and_then(|n| n.checked_add(native.channel_resources().0))
            .ok_or("resource count overflow")?,
        shared_handle_bound: occupied_handles(bank, excluded_preparation)
            .and_then(|n| n.checked_add(summary.shared_resource_handles))
            .ok_or("resource count overflow")?,
        lifecycle_workers: 1,
        analysis_workers: native.analysis_workers(),
        output_contexts: 1,
        native_workers: 0,
        native_children: 0,
        native_ipc: 0,
        source_workers: 0,
        catalog_channels: 0,
        source_readers: 0,
        kernel_handles: 0,
    };
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        witness.root_shared_groups = witness
            .root_shared_groups
            .checked_add(native_sources.groups)
            .ok_or("native source resource count overflow")?;
        witness.native_workers = native_sources.workers;
        witness.native_children = native_sources.children;
        witness.native_ipc = native_sources.ipc;
    }
    for domain in &bank.domains {
        let Some(resources) = domain.resources else {
            continue;
        };
        witness.source_workers = witness
            .source_workers
            .checked_add(resources.worker_lifetimes)
            .ok_or("resource count overflow")?;
        witness.catalog_channels = witness
            .catalog_channels
            .checked_add(resources.catalog_channel_lifetimes)
            .ok_or("resource count overflow")?;
        witness.source_readers = witness
            .source_readers
            .checked_add(resources.source_reader_lifetimes)
            .ok_or("resource count overflow")?;
        witness.kernel_handles = witness
            .kernel_handles
            .checked_add(resources.kernel_handle_bound)
            .ok_or("resource count overflow")?;
    }
    if !witness.covers(bank.root) {
        return Err("constructor resource lifetime closure refused");
    }
    Ok(Some(witness))
}
