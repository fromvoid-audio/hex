//! Control-only setup. Endpoint/pool/queue owners exist before spawn.
use crate::endpoint::{Endpoint, EndpointQueues, PageBuffer, PageRequest};
use crate::worker::WorkerQueues;
use crate::{
    AllocationCapacity, EndpointAllocationFootprint, SourceDefinition, StreamConfig,
    StreamRuntimeError, TransportAllocationFootprint,
};
use hex_engine::StreamSourceKey;
use rtrb::RingBuffer;
pub(crate) struct PreparedTransport {
    pub(crate) endpoint: Endpoint,
    pub(crate) free_pages: Vec<Box<PageBuffer>>,
    pub(crate) worker_queues: WorkerQueues,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransportSetupObservation {
    pub endpoint: EndpointAllocationFootprint,
    pub transport: TransportAllocationFootprint,
    pub free_page_handles: AllocationCapacity,
    pub allocated_pages: usize,
    pub reservation_attempts: usize,
}
#[derive(Default)]
pub(crate) struct SetupFaults {
    #[cfg(test)]
    pub(crate) fail_reservation: Option<usize>,
    #[cfg(test)]
    pub(crate) fail_spawn: bool,
}
fn before_reserve(attempts: &mut usize, faults: &SetupFaults) -> Result<(), ()> {
    #[cfg(test)]
    let attempt = *attempts;
    *attempts += 1;
    #[cfg(test)]
    if faults.fail_reservation == Some(attempt) {
        return Err(());
    }
    #[cfg(not(test))]
    let _ = faults;
    Ok(())
}
impl PreparedTransport {
    pub(crate) fn build(
        config: StreamConfig,
        definitions: &[SourceDefinition],
        observation: &mut TransportSetupObservation,
        faults: &SetupFaults,
    ) -> Result<Self, StreamRuntimeError> {
        let allocation_error = |_| StreamRuntimeError::AllocationFailed;
        let mut keys = Vec::new();
        before_reserve(&mut observation.reservation_attempts, faults).map_err(allocation_error)?;
        let reserved = keys.try_reserve_exact(definitions.len());
        observation.endpoint.source_keys =
            AllocationCapacity::of::<StreamSourceKey>(keys.capacity());
        reserved.map_err(|_| StreamRuntimeError::AllocationFailed)?;
        for definition in definitions {
            if definition.key != definition.metadata.key() || keys.contains(&definition.key) {
                return Err(StreamRuntimeError::InvalidConfiguration);
            }
            keys.push(definition.key);
        }
        keys.sort_unstable();
        let mut free_pages = Vec::new();
        before_reserve(&mut observation.reservation_attempts, faults).map_err(allocation_error)?;
        let reserved = free_pages.try_reserve_exact(config.page_buffers);
        observation.free_page_handles =
            AllocationCapacity::of::<Box<PageBuffer>>(free_pages.capacity());
        reserved.map_err(|_| StreamRuntimeError::AllocationFailed)?;
        for _ in 0..config.page_buffers {
            before_reserve(&mut observation.reservation_attempts, faults)
                .map_err(allocation_error)?;
            let mut frames_capacity = 0;
            let page = PageBuffer::new_reported(&mut frames_capacity);
            observation.transport.page_boxing_peak_extra =
                AllocationCapacity::of::<hex_engine::Frame>(
                    observation
                        .transport
                        .page_boxing_peak_extra
                        .capacity
                        .max(frames_capacity),
                );
            let (page, _) = page.map_err(allocation_error)?;
            free_pages.push(page);
            observation.allocated_pages = free_pages.len();
            observation.transport.page_objects =
                AllocationCapacity::of::<PageBuffer>(free_pages.len());
            observation.transport.pcm_frames = AllocationCapacity::of::<hex_engine::Frame>(
                free_pages.iter().map(|page| page.frames.len()).sum(),
            );
        }
        let (request_tx, request_rx) = RingBuffer::new(config.request_capacity);
        observation.transport.request_slots =
            AllocationCapacity::of::<PageRequest>(request_tx.buffer().capacity());
        observation.transport.queue_object_bytes = std::mem::size_of::<RingBuffer<PageRequest>>();
        let (ready_tx, ready_rx) = RingBuffer::new(config.ready_capacity);
        observation.transport.ready_slots =
            AllocationCapacity::of::<Box<PageBuffer>>(ready_tx.buffer().capacity());
        observation.transport.queue_object_bytes = observation
            .transport
            .queue_object_bytes
            .checked_add(std::mem::size_of::<RingBuffer<Box<PageBuffer>>>())
            .ok_or(StreamRuntimeError::InvalidConfiguration)?;
        let (return_tx, return_rx) = RingBuffer::new(config.return_capacity);
        observation.transport.return_slots =
            AllocationCapacity::of::<Box<PageBuffer>>(return_tx.buffer().capacity());
        observation.transport.queue_object_bytes = observation
            .transport
            .queue_object_bytes
            .checked_add(std::mem::size_of::<RingBuffer<Box<PageBuffer>>>())
            .ok_or(StreamRuntimeError::InvalidConfiguration)?;
        let (failure_tx, failure_rx) = RingBuffer::new(config.failure_capacity);
        observation.transport.failure_slots =
            AllocationCapacity::of::<crate::StreamFailure>(failure_tx.buffer().capacity());
        observation.transport.queue_object_bytes = observation
            .transport
            .queue_object_bytes
            .checked_add(std::mem::size_of::<RingBuffer<crate::StreamFailure>>())
            .ok_or(StreamRuntimeError::InvalidConfiguration)?;
        observation.transport.stop_flag_object_bytes =
            std::mem::size_of::<std::sync::atomic::AtomicBool>();
        observation.transport.catalog_owner_object_bytes =
            crate::runtime::catalog_owner_data_bytes();
        let endpoint = Endpoint::new_reported(
            config.page_buffers,
            config.request_capacity,
            keys,
            EndpointQueues {
                request_tx,
                ready_rx,
                return_tx,
                failure_rx,
            },
            &mut observation.endpoint,
            || before_reserve(&mut observation.reservation_attempts, faults),
        )
        .map_err(allocation_error)?;
        Ok(Self {
            endpoint,
            free_pages,
            worker_queues: WorkerQueues {
                requests: request_rx,
                ready: ready_tx,
                returns: return_rx,
                failures: failure_tx,
            },
        })
    }
}
