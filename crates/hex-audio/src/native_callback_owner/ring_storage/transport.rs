//! Affine holders for the THREE existing prepared-audio queues. They neither
//! create a new queue/family nor expose owning raw endpoints. A constructor
//! lease, when present, follows the same endpoint through existing custody.
use hex_engine::sampler_host_fields::NativeFixedRingLease;

pub(crate) struct NativePreparedProducer<T> {
    value: Option<rtrb::Producer<T>>,
    storage: Option<NativeFixedRingLease>,
}
pub(crate) struct NativePreparedConsumer<T> {
    value: Option<rtrb::Consumer<T>>,
    storage: Option<NativeFixedRingLease>,
}
pub(crate) fn native_prepared_ring<T>(
    capacity: usize,
) -> (NativePreparedProducer<T>, NativePreparedConsumer<T>) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity);
    // No legacy endpoint is retrospectively qualified. Complete Root factory
    // preflight/registration must select the constructor-bound path before any
    // endpoint escapes; None here remains explicitly unsupported by that union.
    (
        NativePreparedProducer {
            value: Some(producer),
            storage: None,
        },
        NativePreparedConsumer {
            value: Some(consumer),
            storage: None,
        },
    )
}
// Only the fresh private factory calls this, before either endpoint escapes.
// The record contains no endpoint/Root owner. Its two non-Clone leases follow
// the existing endpoints; retained payload children still require their schema.
struct FreshRing<'a, T> {
    producer: &'a rtrb::Producer<T>,
    consumer: &'a rtrb::Consumer<T>,
}
impl<T> hex_engine::sampler_host_fields::NativeFixedRingFields for FreshRing<'_, T> {
    type Element = T;
    fn producer_core(&self) -> hex_engine::sampler_host_fields::BodyRef<'_> {
        hex_engine::sampler_host_fields::BodyRef::of(self.producer.buffer())
    }
    fn consumer_core(&self) -> hex_engine::sampler_host_fields::BodyRef<'_> {
        hex_engine::sampler_host_fields::BodyRef::of(self.consumer.buffer())
    }
    fn capacity(&self) -> usize {
        self.producer.buffer().capacity()
    }
    fn pinned_constructor_supported(&self) -> bool {
        super::supported()
    }
}
pub(crate) fn registered_prepared_ring<T>(
    capacity: usize,
) -> Result<
    (
        NativePreparedProducer<T>,
        NativePreparedConsumer<T>,
        hex_engine::sampler_host_fields::NativeFixedRingRegistration,
    ),
    hex_engine::AudioStorageError,
> {
    if !super::supported() {
        return Err(hex_engine::AudioStorageError::UnknownStorage);
    }
    let (mut producer, mut consumer) = native_prepared_ring(capacity);
    let (record, [first, second]) = hex_engine::sampler_host_fields::NativeFixedRingRegistration::
        pair_actual_fresh_ring_on_control(&FreshRing {
            producer: producer.value.as_ref().expect("fresh producer"),
            consumer: consumer.value.as_ref().expect("fresh consumer"),
        })?;
    producer.storage = Some(first);
    consumer.storage = Some(second);
    Ok((producer, consumer, record))
}

impl<T> NativePreparedProducer<T> {
    pub(crate) fn is_unregistered(&self) -> bool {
        self.storage.is_none()
    }
    pub(crate) fn matches_constructor_record(
        &self,
        record: &hex_engine::sampler_host_fields::NativeFixedRingRegistration,
    ) -> bool {
        self.value.is_some()
            && self
                .storage
                .as_ref()
                .is_some_and(|lease| record.matches_live_endpoint_on_control(lease, 0))
    }
    pub(crate) fn push(&mut self, value: T) -> Result<(), rtrb::PushError<T>> {
        self.value
            .as_mut()
            .expect("prepared producer already disposed")
            .push(value)
    }
    pub(crate) fn is_abandoned(&self) -> bool {
        self.value.as_ref().is_none_or(|value| value.is_abandoned())
    }
    pub(crate) fn is_full(&self) -> bool {
        self.value
            .as_ref()
            .expect("prepared producer already disposed")
            .is_full()
    }
    pub(crate) fn buffer(&self) -> &rtrb::RingBuffer<T> {
        self.value
            .as_ref()
            .expect("prepared producer already disposed")
            .buffer()
    }
    pub(crate) fn write_chunk_uninit(
        &mut self,
        count: usize,
    ) -> Result<rtrb::chunks::WriteChunkUninit<'_, T>, rtrb::chunks::ChunkError> {
        self.value
            .as_mut()
            .expect("prepared producer already disposed")
            .write_chunk_uninit(count)
    }
    #[cfg(test)]
    pub(crate) fn slots(&self) -> usize {
        self.value
            .as_ref()
            .expect("prepared producer already disposed")
            .slots()
    }
}
impl<T> NativePreparedConsumer<T> {
    pub(crate) fn is_unregistered(&self) -> bool {
        self.storage.is_none()
    }
    pub(crate) fn matches_constructor_record(
        &self,
        record: &hex_engine::sampler_host_fields::NativeFixedRingRegistration,
    ) -> bool {
        self.value.is_some()
            && self
                .storage
                .as_ref()
                .is_some_and(|lease| record.matches_live_endpoint_on_control(lease, 1))
    }
    pub(crate) fn slots(&self) -> usize {
        self.value
            .as_ref()
            .expect("prepared consumer already disposed")
            .slots()
    }
    pub(crate) fn pop(&mut self) -> Result<T, rtrb::PopError> {
        self.value
            .as_mut()
            .expect("prepared consumer already disposed")
            .pop()
    }
    pub(crate) fn peek(&self) -> Result<&T, rtrb::PeekError> {
        self.value
            .as_ref()
            .expect("prepared consumer already disposed")
            .peek()
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.value
            .as_ref()
            .expect("prepared consumer already disposed")
            .is_empty()
    }
    pub(crate) fn buffer(&self) -> &rtrb::RingBuffer<T> {
        self.value
            .as_ref()
            .expect("prepared consumer already disposed")
            .buffer()
    }
}
impl<T> Drop for NativePreparedProducer<T> {
    fn drop(&mut self) {
        if let Some(storage) = self.storage.as_mut() {
            let _ = storage.begin_actual_endpoint_drop_on_control();
        }
        drop(self.value.take());
        if let Some(storage) = self.storage.as_mut() {
            let _ = storage.complete_actual_endpoint_drop_on_control();
        }
    }
}
impl<T> Drop for NativePreparedConsumer<T> {
    fn drop(&mut self) {
        if let Some(storage) = self.storage.as_mut() {
            let _ = storage.begin_actual_endpoint_drop_on_control();
        }
        drop(self.value.take());
        if let Some(storage) = self.storage.as_mut() {
            let _ = storage.complete_actual_endpoint_drop_on_control();
        }
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
#[path = "registered_tests.rs"]
mod registered_tests;
