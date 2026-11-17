//! Listed stream DATA capacities. Opaque std/allocator/thread costs are separate
//! obligations, never zero-cost claims or aggregate admission.
use crate::types::StreamRuntimeError;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllocationData {
    pub capacity: usize,
    pub element_bytes: usize,
}
impl AllocationData {
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
pub(crate) fn add(a: usize, b: usize) -> Result<usize, StreamRuntimeError> {
    a.checked_add(b).ok_or(StreamRuntimeError::AllocationFailed)
}
pub(crate) fn sum(values: impl IntoIterator<Item = usize>) -> Option<usize> {
    values.into_iter().try_fold(0usize, usize::checked_add)
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EndpointData {
    pub cache: AllocationData,
    pub pending: AllocationData,
    pub source_keys: AllocationData,
    pub held_returns: AllocationData,
}
impl EndpointData {
    pub fn bytes(self) -> Option<usize> {
        sum([
            self.cache.bytes()?,
            self.pending.bytes()?,
            self.source_keys.bytes()?,
            self.held_returns.bytes()?,
        ])
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransportData {
    pub page_objects: usize,
    pub pcm_boxes: usize,
    pub scalar_boxes: usize,
    pub scalar_coverage_boxes: usize,
    /// Audited constructor declaration, NOT an observed Vec capacity. Unknown
    /// unless observed pinned source and supported std family match within the
    /// repository fixed-Cargo-invocation class and exact required CI.
    pub queue_slot_data_declared: usize,
    pub queue_object_data: usize,
    pub catalog_owner_data: usize,
    pub stop_flag_data: usize,
    pub pair_owner_data: usize,
    pub page_boxing_overlap: usize,
    pub queue_constructor_supported: bool,
}
impl TransportData {
    pub fn bytes(self) -> Option<usize> {
        self.queue_constructor_supported.then_some(())?;
        sum([
            self.page_objects,
            self.pcm_boxes,
            self.scalar_boxes,
            self.scalar_coverage_boxes,
            self.queue_slot_data_declared,
            self.queue_object_data,
            self.catalog_owner_data,
            self.stop_flag_data,
            self.pair_owner_data,
        ])
    }
}
/// Constructor-captured scalar capacities for the fixed worker DATA reservation.
/// Not live field addresses or a worker/handle lifetime receipt. Terminal state
/// observations invalidate the report; racing completion only releases storage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkerData {
    pub constructor_complete: bool,
    pub granular: Option<crate::GranularRuntimeData>,
    pub source_slots: AllocationData,
    pub definition_paths: usize,
    pub reader_paths: usize,
    /// Digest backing plus sized manifest owner DATA, conservatively charged
    /// per reader even when immutable owners are shared. Excludes Arc metadata.
    pub manifests: usize,
    pub kernel_objects: usize,
    pub kernel_weights: usize,
    pub pitch_filter_weights: usize,
    pub kernel_handles: usize,
    pub source_scratch: AllocationData,
    pub pitch_scratch: AllocationData,
    pub free_page_handles: AllocationData,
    pub held_page_handles: AllocationData,
    pub held_failures: AllocationData,
    pub membership: AllocationData,
    pub scalar_membership: AllocationData,
    pub scalar_shared_rows: AllocationData,
    pub scalar_source_slots: AllocationData,
    pub scalar_source_backings: usize,
    pub scalar_projection_data: usize,
    pub scalar_projection_groups: usize,
    pub scalar_source_handles: usize,
    pub scalar_cursor_handle_bound: usize,
    pub scalar_job_data: usize,
    pub scalar_deferred_keys: AllocationData,
    pub scalar_deferred_inline: usize,
    pub catalog_object: usize,
    pub construction_definition_slots: AllocationData,
    pub construction_kernel_cache: AllocationData,
    pub construction_cache_old_backings: usize,
}
impl WorkerData {
    pub fn bytes(self) -> Option<usize> {
        self.constructor_complete.then_some(())?;
        sum([
            self.granular
                .map_or(Some(0), crate::GranularRuntimeData::bytes)?,
            self.source_slots.bytes()?,
            self.definition_paths,
            self.reader_paths,
            self.manifests,
            self.kernel_objects,
            self.kernel_weights,
            self.pitch_filter_weights,
            self.source_scratch.bytes()?,
            self.pitch_scratch.bytes()?,
            self.free_page_handles.bytes()?,
            self.held_page_handles.bytes()?,
            self.held_failures.bytes()?,
            self.membership.bytes()?,
            self.scalar_membership.bytes()?,
            self.scalar_shared_rows.bytes()?,
            self.scalar_source_slots.bytes()?,
            self.scalar_source_backings,
            self.scalar_projection_data,
            self.scalar_job_data,
            self.scalar_deferred_keys.bytes()?,
            self.scalar_deferred_inline,
            self.catalog_object,
        ])
    }
    pub fn construction_extra(self) -> Option<usize> {
        sum([
            self.construction_definition_slots.bytes()?,
            self.construction_kernel_cache.bytes()?,
            self.construction_cache_old_backings,
        ])
    }
}
/// Complete only for the DATA fields listed by EndpointData, TransportData and
/// WorkerData. Arc control blocks, mpsc/thread/OS
/// machinery, allocator rounding/headers and stack/thread locals are excluded
/// HERE and MUST remain explicit obligations in the aggregate façade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamDataFootprint {
    pub endpoint: EndpointData,
    pub transport: TransportData,
    pub worker: WorkerData,
}
impl StreamDataFootprint {
    pub fn listed_data_bytes(self) -> Option<usize> {
        sum([
            self.endpoint.bytes()?,
            self.transport.bytes()?,
            self.worker.bytes()?,
        ])
    }
    /// Conservative old/new construction overlap, not sampled process peak.
    /// The whole retained domain is charged once; no cached subset is removed.
    pub fn listed_construction_peak(self) -> Option<usize> {
        sum([
            self.listed_data_bytes()?,
            self.transport.page_boxing_overlap,
            self.worker.construction_extra()?,
        ])
    }
}
pub(crate) fn queue_family_supported() -> bool {
    cfg!(hex_stream_rtrb_source_observation_matches)
        && hex_engine::audio_storage_constructor_layout_supported()
}

#[cfg(test)]
mod checked_data_tests {
    use super::*;
    fn complete_empty() -> StreamDataFootprint {
        StreamDataFootprint {
            endpoint: EndpointData::default(),
            transport: TransportData {
                queue_constructor_supported: true,
                ..Default::default()
            },
            worker: WorkerData {
                constructor_complete: true,
                ..Default::default()
            },
        }
    }
    #[test]
    fn overflow_and_unknown_constructor_cannot_produce_a_data_bound() {
        let mut data = complete_empty();
        assert_eq!(data.listed_data_bytes(), Some(0));
        data.endpoint.cache = AllocationData {
            capacity: usize::MAX,
            element_bytes: 2,
        };
        assert_eq!(data.listed_data_bytes(), None);
        let mut data = complete_empty();
        data.worker.reader_paths = usize::MAX;
        data.worker.manifests = 1;
        assert_eq!(data.listed_data_bytes(), None);
        let mut data = complete_empty();
        data.transport.pcm_boxes = usize::MAX;
        data.transport.page_objects = 1;
        assert_eq!(data.listed_data_bytes(), None);
        let mut data = complete_empty();
        data.worker.construction_definition_slots = AllocationData {
            capacity: usize::MAX,
            element_bytes: 2,
        };
        assert_eq!(data.listed_data_bytes(), Some(0));
        assert_eq!(data.listed_construction_peak(), None);
        let mut data = complete_empty();
        data.transport.queue_constructor_supported = false;
        assert_eq!(data.listed_data_bytes(), None);
        let mut data = complete_empty();
        data.worker.constructor_complete = false;
        assert_eq!(data.listed_data_bytes(), None);
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod constructor_tests {
    use super::*;
    use crate::{
        StreamFailure,
        endpoint::{PageBuffer, PageRequest},
    };
    use std::{alloc::Layout, sync::atomic::AtomicUsize};
    fn probe<T>(capacity: usize) {
        let owner = Layout::array::<AtomicUsize>(2)
            .unwrap()
            .extend(Layout::new::<rtrb::RingBuffer<T>>())
            .unwrap()
            .0
            .pad_to_align()
            .size();
        let slots = capacity.checked_mul(std::mem::size_of::<T>()).unwrap();
        let observed = allocation_counter::measure(|| {
            let (producer, consumer) = rtrb::RingBuffer::<T>::new(capacity);
            assert_eq!(producer.buffer().capacity(), capacity);
            assert!(std::ptr::eq(producer.buffer(), consumer.buffer()));
            drop((producer, consumer));
        });
        assert_eq!(observed.count_total, 2);
        assert_eq!(observed.bytes_total as usize, owner + slots);
        assert_eq!(observed.count_current, 0);
        assert_eq!(observed.bytes_current, 0);
    }
    #[test]
    fn all_four_actual_queue_message_constructors_match_pinned_requested_allocations() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            assert!(!queue_family_supported());
            eprintln!("stream queue constructor: unsupported compiler remains Unknown");
            return;
        }
        assert!(
            queue_family_supported(),
            "observed pinned source must match the supported fixed-invocation class"
        );
        for capacity in [1, 3, 6, 1024, 2048, 8192] {
            probe::<PageRequest>(capacity);
            probe::<Box<PageBuffer>>(capacity); // ready
            probe::<Box<PageBuffer>>(capacity); // return
            probe::<StreamFailure>(capacity);
        }
        eprintln!(
            "stream queue constructor: supported pinned source; four actual message types; 24 capacity/type controls"
        );
    }
}
