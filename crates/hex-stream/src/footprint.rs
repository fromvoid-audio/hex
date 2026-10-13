//! Allocation-capacity accounting, not admission policy or process RSS.
use crate::CatalogFootprint;

/// Actual reserved element capacity, including unused slots. Does not include
/// allocator rounding/headers. Products and sums fail explicitly on overflow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllocationCapacity {
    pub capacity: usize,
    pub element_bytes: usize,
}

impl AllocationCapacity {
    pub(crate) fn of<T>(capacity: usize) -> Self {
        Self {
            capacity,
            element_bytes: std::mem::size_of::<T>(),
        }
    }
    pub fn bytes(self) -> Option<usize> {
        self.capacity.checked_mul(self.element_bytes)
    }
}

fn sum(values: &[usize]) -> Option<usize> {
    values
        .iter()
        .try_fold(0_usize, |total, value| total.checked_add(*value))
}

/// Exclusively owned by one page endpoint. Cache slots hold page pointers:
/// their page objects/PCM are charged once to the shared pool, not here again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EndpointAllocationFootprint {
    pub cache: AllocationCapacity,
    pub pending: AllocationCapacity,
    pub source_keys: AllocationCapacity,
    pub held_returns: AllocationCapacity,
    pub max_cached_pages: usize,
}
impl EndpointAllocationFootprint {
    pub fn bytes(self) -> Option<usize> {
        sum(&[
            self.cache.bytes()?,
            self.pending.bytes()?,
            self.source_keys.bytes()?,
            self.held_returns.bytes()?,
        ])
    }
}

/// Shared transport storage: each queue object/slot allocation is charged once
/// despite its producer/consumer handles. Pool objects/PCM are charged once
/// regardless of their current location (worker, cache, queues or held slots).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransportAllocationFootprint {
    pub page_objects: AllocationCapacity,
    pub pcm_frames: AllocationCapacity,
    pub request_slots: AllocationCapacity,
    pub ready_slots: AllocationCapacity,
    pub return_slots: AllocationCapacity,
    pub failure_slots: AllocationCapacity,
    pub queue_object_bytes: usize,
    pub catalog_owner_object_bytes: usize,
    pub stop_flag_object_bytes: usize,
    /// Largest observed pre-boxing frame Vec capacity. Construction-only:
    /// bounds a possible old/new overlap when excess capacity is discarded.
    /// Pages are constructed sequentially; never charge this per pool page.
    pub page_boxing_peak_extra: AllocationCapacity,
}
impl TransportAllocationFootprint {
    pub fn bytes(self) -> Option<usize> {
        sum(&[
            self.page_objects.bytes()?,
            self.pcm_frames.bytes()?,
            self.request_slots.bytes()?,
            self.ready_slots.bytes()?,
            self.return_slots.bytes()?,
            self.failure_slots.bytes()?,
            self.queue_object_bytes,
            self.catalog_owner_object_bytes,
            self.stop_flag_object_bytes,
        ])
    }
}

/// Worker and immutable catalog allocations retained once per actual catalog.
/// `source_slots` includes inline reader objects AND their fixed 64KiB scratch;
/// do not add the legacy scratch subcomponent again. Shared kernel data/weights
/// are counted once per reused kernel, not once per plan or lease. Catalog
/// membership metadata is an independent allocation and is charged separately.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatalogAllocationFootprint {
    pub source_slots: AllocationCapacity,
    pub definition_path_bytes: usize,
    pub reader_path_bytes: usize,
    pub manifest_bytes: usize,
    pub kernel_object_bytes: usize,
    pub kernel_weight_bytes: usize,
    pub membership_slots: AllocationCapacity,
    pub catalog_object_bytes: usize,
    pub source_scratch: AllocationCapacity,
    pub free_page_handles: AllocationCapacity,
    pub held_page_handles: AllocationCapacity,
    pub held_failures: AllocationCapacity,
}
impl CatalogAllocationFootprint {
    pub fn bytes(self) -> Option<usize> {
        sum(&[
            self.source_slots.bytes()?,
            self.definition_path_bytes,
            self.reader_path_bytes,
            self.manifest_bytes,
            self.kernel_object_bytes,
            self.kernel_weight_bytes,
            self.membership_slots.bytes()?,
            self.catalog_object_bytes,
            self.source_scratch.bytes()?,
            self.free_page_handles.bytes()?,
            self.held_page_handles.bytes()?,
            self.held_failures.bytes()?,
        ])
    }
}

/// Observed construction-only vector capacities plus the existing conservative
/// kernel-table construction reservation. This is a bound, not a sampled peak:
/// definitions/cache can overlap retained allocations; their lifetimes need
/// not overlap each other. No transient is added to the retained total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConstructionAllocationFootprint {
    pub definition_slots: AllocationCapacity,
    pub kernel_cache_slots: AllocationCapacity,
    pub kernel_table_peak_bound_bytes: usize,
}

/// Full capacity report for one endpoint/catalog pairing. Queries allocate,
/// clone and mutate nothing. Not an aggregate ledger/admission decision.
/// Excludes opaque allocator/Arc/channel/thread/OS bookkeeping, thread/stack
/// locals, caller graph/history/export allocations and caller-owned handles.
/// Includes data in heap objects and all owned buffer/vector capacities below.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamAllocationFootprint {
    pub endpoint: EndpointAllocationFootprint,
    pub transport: TransportAllocationFootprint,
    pub catalog: CatalogAllocationFootprint,
    pub construction: ConstructionAllocationFootprint,
    /// Legacy payload/subcomponent report; not an extra additive allocation.
    pub legacy_catalog: CatalogFootprint,
}
impl StreamAllocationFootprint {
    pub fn retained_bytes(self) -> Option<usize> {
        sum(&[
            self.endpoint.bytes()?,
            self.transport.bytes()?,
            self.catalog.bytes()?,
        ])
    }
    pub fn construction_peak_bound_bytes(self) -> Option<usize> {
        // Replace retained table bytes with their construction reservation;
        // adding both would double-charge the same kernel weight allocation.
        let table_extra = self
            .construction
            .kernel_table_peak_bound_bytes
            .checked_sub(self.catalog.kernel_weight_bytes)?;
        // Vec growth can temporarily retain the old cache allocation. Twice
        // its final capacity bounds old + new without assuming a growth factor.
        let cache_peak = self
            .construction
            .kernel_cache_slots
            .bytes()?
            .checked_mul(2)?;
        sum(&[
            self.retained_bytes()?,
            self.construction.definition_slots.bytes()?,
            cache_peak,
            table_extra,
            self.transport.page_boxing_peak_extra.bytes()?,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capacity_products_sums_and_peak_replacement_refuse_overflow() {
        assert_eq!(
            AllocationCapacity {
                capacity: usize::MAX,
                element_bytes: 2
            }
            .bytes(),
            None
        );
        let mut report = StreamAllocationFootprint::default();
        report.catalog.definition_path_bytes = usize::MAX;
        report.catalog.reader_path_bytes = 1;
        assert_eq!(report.retained_bytes(), None);
        report.catalog.reader_path_bytes = 0;
        report.construction.definition_slots = AllocationCapacity::of::<u8>(1);
        assert_eq!(report.construction_peak_bound_bytes(), None);
        report.catalog.definition_path_bytes = 0;
        report.catalog.kernel_weight_bytes = 10;
        report.construction.kernel_table_peak_bound_bytes = 9;
        assert_eq!(report.construction_peak_bound_bytes(), None);
        report.construction.kernel_table_peak_bound_bytes = 30;
        assert_eq!(report.retained_bytes(), Some(10));
        assert_eq!(report.construction_peak_bound_bytes(), Some(31));
        // A representable retained report can still overflow its transient
        // cache-growth bound. No wrapping/saturation authorizes a false budget.
        report.construction.kernel_cache_slots = AllocationCapacity::of::<u8>(usize::MAX);
        assert_eq!(report.retained_bytes(), Some(10));
        assert_eq!(report.construction_peak_bound_bytes(), None);
        report.construction.kernel_cache_slots = AllocationCapacity::default();
        report.transport.page_boxing_peak_extra = AllocationCapacity::of::<u8>(usize::MAX);
        assert_eq!(report.retained_bytes(), Some(10));
        assert_eq!(report.construction_peak_bound_bytes(), None);
    }
}
