//! Actual copied-channel backing, separate from native source/publication custody.
use super::*;

/// Known heap payload and bounded shared ownership from one channel constructor.
/// Endpoint values are excluded: their inline storage belongs to the host's
/// callback/control cells. Arc headers, allocator overhead and native resources
/// remain opaque; this is neither an RSS estimate nor a publication witness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstructorStorage {
    pub data_bytes: usize,
    pub shared_groups: usize,
    pub shared_handle_bound: usize,
}

/// Known DATA requested by this module's fixed channel factory, before either
/// ring is allocated. Endpoint values and allocator/Arc implementation overhead
/// are excluded, as in the actual paired-channel inventory. This quote grants
/// no source, worker or publication authority.
pub fn channel_storage_bound() -> Option<ConstructorStorage> {
    let ring = QUEUE_CAPACITY
        .checked_mul(std::mem::size_of::<Block>())?
        .checked_add(std::mem::size_of::<RingBuffer<Block>>())?;
    Some(ConstructorStorage {
        data_bytes: ring
            .checked_mul(2)?
            .checked_add(std::mem::size_of::<AtomicBool>())?,
        shared_groups: 3,
        shared_handle_bound: 7,
    })
}

impl Callback {
    /// Visit only this channel's actual rings and revocation payload. Equal
    /// binding/quantum values from another construction do not establish pairing.
    /// Revocation or a returned worker does not reclaim these retained backings.
    /// The handle bound includes both original ring endpoints and all three
    /// private revocation handles, independently of current queue occupancy.
    pub fn constructor_storage_on_control(&self, control: &Control) -> Option<ConstructorStorage> {
        if !self.matches_control(control) {
            return None;
        }
        let ring_bytes = |ring: &RingBuffer<Block>| {
            ring.capacity()
                .checked_mul(std::mem::size_of::<Block>())?
                .checked_add(std::mem::size_of_val(ring))
        };
        Some(ConstructorStorage {
            data_bytes: ring_bytes(self.requests.buffer())?
                .checked_add(ring_bytes(self.replies.buffer())?)?
                .checked_add(std::mem::size_of_val(self.revoked.as_ref()))?,
            shared_groups: 3,
            shared_handle_bound: 7,
        })
    }
}
