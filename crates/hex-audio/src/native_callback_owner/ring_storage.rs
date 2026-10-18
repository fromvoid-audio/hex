//! Requested fixed rtrb infrastructure reservation, not payload-child admission.
//! rtrb 0.3.4's closed safe constructor owns one sized Arc<RingBuffer<T>> and
//! one Vec::with_capacity(capacity) backing; neither allocation grows on push/pop.
use std::{alloc::Layout, sync::atomic::AtomicUsize};

fn supported() -> bool {
    let target = match option_env!("HEX_NATIVE_CARRIER_LAYOUT_TARGET") {
        Some("x86_64-unknown-linux-gnu") => cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )),
        Some("aarch64-apple-darwin") => cfg!(all(target_arch = "aarch64", target_os = "macos")),
        _ => false,
    };
    target && option_env!("HEX_NATIVE_RING_LAYOUT_KNOWN") == Some("1")
}

pub(super) fn requested_bytes<T>(ring: &rtrb::RingBuffer<T>) -> Option<usize> {
    quote(ring, supported())
}
fn quote<T>(ring: &rtrb::RingBuffer<T>, qualified: bool) -> Option<usize> {
    if !qualified {
        return None;
    }
    // Vec's pinned Global constructor requests this array layout. Empty and
    // ZST backings request zero bytes; no fictitious backing allocation is added.
    // This includes the full Arc allocation rather than double-charging the
    // RingBuffer body alongside its header. Element-owned children are excluded.
    let backing = Layout::array::<T>(ring.capacity()).ok()?;
    let shared = Layout::array::<AtomicUsize>(2)
        .ok()?
        .extend(Layout::for_value(ring))
        .ok()?
        .0
        .pad_to_align();
    shared.size().checked_add(backing.size())
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests {
    use super::*;
    #[test]
    fn fixed_ring_requests_match_actual_constructor_bytes_including_alignment_and_empty_backing() {
        assert!(supported(), "native fixed-ring gate must execute");
        eprintln!("SUPPORTED native-fixed-rtrb-0.3.4-requested-v1");
        for capacity in [0, 1, 7, 128] {
            let mut endpoints = None;
            let measured = allocation_counter::measure(|| {
                endpoints = Some(rtrb::RingBuffer::<[u8; 17]>::new(capacity));
            });
            let (mut producer, mut consumer) = endpoints.unwrap();
            let bytes = requested_bytes(producer.buffer()).unwrap();
            assert_eq!(measured.count_total, 1 + u64::from(capacity != 0));
            assert_eq!(
                measured.count_current,
                i64::try_from(measured.count_total).unwrap()
            );
            assert_eq!(measured.bytes_total, u64::try_from(bytes).unwrap());
            assert_eq!(measured.bytes_current, i64::try_from(bytes).unwrap());
            assert_eq!(requested_bytes(consumer.buffer()), Some(bytes));
            assert!(u64::try_from(bytes - 1).unwrap() < measured.bytes_total);
            let transfers = allocation_counter::measure(|| {
                if capacity != 0 {
                    for i in 0..128 {
                        producer.push([i; 17]).unwrap();
                        assert_eq!(consumer.pop().unwrap(), [i; 17]);
                    }
                }
                assert_eq!(requested_bytes(producer.buffer()), Some(bytes));
            });
            assert_eq!(
                (
                    transfers.count_total,
                    transfers.bytes_total,
                    transfers.count_current,
                    transfers.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
    }
    #[test]
    fn fixed_ring_zst_has_only_the_shared_allocation_and_unknown_gate_refuses() {
        assert!(supported(), "native fixed-ring gate must execute");
        eprintln!("SUPPORTED native fixed-ring ZST and unsupported refusal");
        let mut endpoints = None;
        let measured =
            allocation_counter::measure(|| endpoints = Some(rtrb::RingBuffer::<()>::new(17)));
        let (producer, _) = endpoints.unwrap();
        assert_eq!(measured.count_total, 1);
        assert_eq!(
            measured.bytes_total,
            u64::try_from(requested_bytes(producer.buffer()).unwrap()).unwrap()
        );
        assert!(quote(producer.buffer(), false).is_none());
    }
}
