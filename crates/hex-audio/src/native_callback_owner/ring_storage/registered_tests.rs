//! Actual pinned producer/consumer, with finite before-birth local caps. This
//! does not enable production without its complete Root/family/child union.
use super::super::supported;
use super::*;
use super::{NativePreparedConsumer as Consumer, NativePreparedProducer as Producer};
use hex_engine::{
    AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation,
    sampler_host_fields::{BodyRef, NativeFixedRingFields, NativeFixedRingRegistration},
};

struct Fresh<'a, T> {
    producer: &'a rtrb::Producer<T>,
    consumer: &'a rtrb::Consumer<T>,
    known: bool,
}
impl<T> NativeFixedRingFields for Fresh<'_, T> {
    type Element = T;
    fn producer_core(&self) -> BodyRef<'_> {
        BodyRef::of(self.producer.buffer())
    }
    fn consumer_core(&self) -> BodyRef<'_> {
        BodyRef::of(self.consumer.buffer())
    }
    fn capacity(&self) -> usize {
        self.producer.buffer().capacity()
    }
    fn pinned_constructor_supported(&self) -> bool {
        self.known && supported()
    }
}
fn bound<T>(capacity: usize) -> (usize, usize) {
    let retained =
        NativeFixedRingRegistration::requested_bytes_on_control::<rtrb::RingBuffer<T>, T>(capacity)
            .unwrap();
    let peak = retained
        + NativeFixedRingRegistration::construction_scratch_bytes_on_control::<rtrb::RingBuffer<T>>(
        )
        .unwrap()
        + size_of::<(rtrb::Producer<T>, rtrb::Consumer<T>)>()
        + size_of::<(Producer<T>, Consumer<T>, NativeFixedRingRegistration)>();
    (retained, peak)
}
fn construct<T>(
    capacity: usize,
    retained: usize,
    peak: usize,
) -> Result<(Producer<T>, Consumer<T>, NativeFixedRingRegistration), AudioStorageError> {
    if !supported() {
        return Err(AudioStorageError::UnknownStorage);
    }
    let (needed, needed_peak) = bound::<T>(capacity);
    if needed > retained || needed_peak > peak {
        return Err(AudioStorageError::ReservationExceeded);
    }
    let (mut producer, mut consumer) = native_prepared_ring(capacity);
    let (registration, [producer_lease, consumer_lease]) =
        NativeFixedRingRegistration::pair_actual_fresh_ring_on_control(&Fresh {
            producer: producer.value.as_ref().unwrap(),
            consumer: consumer.value.as_ref().unwrap(),
            known: true,
        })?;
    producer.storage = Some(producer_lease);
    consumer.storage = Some(consumer_lease);
    Ok((producer, consumer, registration))
}

fn data(registration: &NativeFixedRingRegistration) -> Result<usize, AudioStorageError> {
    let mut ledger = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        data_rows: 16,
        handle_rows: 16,
        root_rows: 4,
        max_retained_bytes: 1 << 20,
        max_construction_peak_bytes: 1 << 20,
    })?;
    registration.visit_storage_on_control(&mut ledger)?;
    ledger.validate_selected_shared_handles_on_control()?;
    assert!(ledger.summary()?.require_complete().is_err());
    Ok(ledger.summary()?.data_bytes)
}

#[test]
fn actual_fixed_ring_both_drop_orders_capacity_and_element_layout_preserve_real_core_and_backing() {
    for capacity in [0, 1, 7, 128] {
        for consumer_first in [false, true] {
            let (retained, peak) = bound::<[f32; 2]>(capacity);
            for (r, p) in [(retained - 1, peak), (retained, peak - 1)] {
                let refused = allocation_counter::measure(|| {
                    assert!(matches!(
                        construct::<[f32; 2]>(capacity, r, p),
                        Err(AudioStorageError::ReservationExceeded)
                    ));
                });
                assert_eq!((refused.count_total, refused.bytes_total), (0, 0));
            }
            let mut endpoints = None;
            let allocated = allocation_counter::measure(|| {
                endpoints = Some(construct::<[f32; 2]>(capacity, retained, peak).unwrap())
            });
            assert_eq!(
                (allocated.count_total, allocated.bytes_total),
                (2 + u64::from(capacity != 0), retained as u64)
            );
            let (mut producer, mut consumer, registration) = endpoints.unwrap();
            assert!(registration.matches_actual_leases_on_control([
                producer.storage.as_ref().unwrap(),
                consumer.storage.as_ref().unwrap()
            ]));
            assert_eq!(data(&registration).unwrap(), retained);
            let transfers = allocation_counter::measure(|| {
                if capacity != 0 {
                    for _ in 0..128 {
                        producer.push([1.25; 2]).unwrap();
                        assert_eq!(consumer.pop().unwrap(), [1.25; 2]);
                    }
                }
            });
            assert_eq!(
                (
                    transfers.count_total,
                    transfers.count_current,
                    transfers.bytes_total,
                    transfers.bytes_current
                ),
                (0, 0, 0, 0)
            );
            let metadata =
                NativeFixedRingRegistration::metadata_requested_bytes_on_control().unwrap();
            let (first, last);
            if consumer_first {
                first = allocation_counter::measure(|| drop(consumer));
                assert_eq!(data(&registration).unwrap(), retained);
                last = allocation_counter::measure(|| drop(producer));
            } else {
                first = allocation_counter::measure(|| drop(producer));
                assert_eq!(data(&registration).unwrap(), retained);
                last = allocation_counter::measure(|| drop(consumer));
            }
            assert_eq!(
                (first.count_total, first.count_current, first.bytes_current),
                (0, 0, 0)
            );
            assert_eq!(
                (last.count_total, last.count_current, last.bytes_current),
                (
                    0,
                    -(1 + i64::from(capacity != 0)),
                    -((retained - metadata) as i64)
                )
            );
            assert!(registration.is_normally_disposed_on_control());
            assert_eq!(data(&registration).unwrap(), metadata);
        }
    }
    for capacity in [0, 128] {
        let (retained, peak) = bound::<()>(capacity);
        let mut endpoints = None;
        let actual = allocation_counter::measure(|| {
            endpoints = Some(construct::<()>(capacity, retained, peak).unwrap())
        });
        assert_eq!(
            (actual.count_total, actual.bytes_total),
            (2, retained as u64)
        );
        let (producer, consumer, registration) = endpoints.unwrap();
        assert_eq!(data(&registration).unwrap(), retained);
        drop(producer);
        drop(consumer);
        assert!(registration.is_normally_disposed_on_control());
    }
    eprintln!(
        "SUPPORTED actual pinned rtrb0.3.4+metadata allocations; capacities0/1/7/128 bothdroporders+ZST; both-minus1 beforebirth0alloc;128PCM frame handoffs0alloc/free; first actual endpoint drop releases0; last releases exact core+backing; immutable metadata retained; children NOT certified"
    );
}

#[test]
fn actual_fixed_ring_foreign_pair_unknown_gate_and_missing_endpoint_never_close() {
    let (producer, consumer) = rtrb::RingBuffer::<[f32; 2]>::new(7);
    let (_, foreign) = rtrb::RingBuffer::new(7);
    let refused = allocation_counter::measure(|| {
        assert!(matches!(
            NativeFixedRingRegistration::pair_actual_fresh_ring_on_control(&Fresh {
                producer: &producer,
                consumer: &foreign,
                known: true
            }),
            Err(AudioStorageError::SourceMismatch)
        ));
        assert!(matches!(
            NativeFixedRingRegistration::pair_actual_fresh_ring_on_control(&Fresh {
                producer: &producer,
                consumer: &consumer,
                known: false
            }),
            Err(AudioStorageError::UnknownStorage)
        ));
    });
    assert_eq!((refused.count_total, refused.bytes_total), (0, 0));
    let (registration, [first, second]) =
        NativeFixedRingRegistration::pair_actual_fresh_ring_on_control(&Fresh {
            producer: &producer,
            consumer: &consumer,
            known: true,
        })
        .unwrap();
    drop(first); // Missing live endpoint is NOT physical normal destruction.
    assert_eq!(data(&registration), Err(AudioStorageError::UnknownStorage));
    assert!(!registration.is_normally_disposed_on_control());
    drop(second);
    drop(producer);
    drop(consumer);
    assert_eq!(data(&registration), Err(AudioStorageError::UnknownStorage));
}

#[test]
fn actual_last_endpoint_payload_destructor_fault_remains_charged_with_first_failure() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Fails(Arc<AtomicUsize>);
    impl Drop for Fails {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::panic::panic_any(197u64);
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let (retained, peak) = bound::<Fails>(1);
    let (mut producer, consumer, registration) = construct(1, retained, peak).unwrap();
    producer.push(Fails(calls.clone())).ok().unwrap();
    drop(producer);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let failure =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(consumer))).unwrap_err();
    assert_eq!(failure.downcast_ref::<u64>(), Some(&197));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(data(&registration), Err(AudioStorageError::UnknownStorage));
    assert!(!registration.is_normally_disposed_on_control());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Queued Fails's owning Arc is a mandatory separate child, not a claimed
    // byte inside the ring's Element backing or a waived external field.
}
