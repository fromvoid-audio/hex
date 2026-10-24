//! One known constructor family; declaration is not physical allocator usage.
use std::{alloc::Layout, sync::atomic::AtomicUsize};
/// Sealed constructor declaration for Arc::new(AudioData { retained Vec, None }).
/// The Arc allocation and Vec buffer are independent physical charge locations.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PcmArcReceipt {
    len: usize,
    capacity: usize,
    arc_payload_bytes: usize,
    arc_overhead_bytes: usize,
    buffer_bytes: usize,
}
impl PcmArcReceipt {
    pub(crate) fn current(len: usize, capacity: usize) -> Option<Self> {
        Self::capture(len, capacity, native_layout_known())
    }
    fn capture(len: usize, capacity: usize, known_family: bool) -> Option<Self> {
        if !known_family || len == 0 || capacity < len {
            return None;
        }
        let payload = Layout::new::<crate::AudioData>();
        let requested = Layout::array::<AtomicUsize>(2)
            .ok()?
            .extend(payload)
            .ok()?
            .0
            .pad_to_align();
        let buffer = Layout::array::<crate::Frame>(capacity).ok()?;
        Some(Self {
            len,
            capacity,
            arc_payload_bytes: payload.size(),
            arc_overhead_bytes: requested.size().checked_sub(payload.size())?,
            buffer_bytes: buffer.size(),
        })
    }
    pub(crate) fn declared_bytes(self) -> Option<usize> {
        self.arc_payload_bytes
            .checked_add(self.arc_overhead_bytes)?
            .checked_add(self.buffer_bytes)
    }
    pub(crate) fn checked_allocations(self, audio: &crate::Audio) -> Option<[(usize, usize); 2]> {
        (self.len == audio.frames.values.len()
            && self.capacity == audio.frames.values.capacity()
            && audio.frames._derived.is_none())
        .then_some([
            (self.arc_payload_bytes, self.arc_overhead_bytes),
            (self.buffer_bytes, 0),
        ])
    }
}
pub(crate) fn native_layout_known() -> bool {
    let native = match option_env!("HEX_PCM_ARC_LAYOUT_TARGET") {
        Some("x86_64-unknown-linux-gnu") => cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )),
        Some("aarch64-apple-darwin") => cfg!(all(target_arch = "aarch64", target_os = "macos")),
        _ => false,
    };
    native
        && option_env!("HEX_PCM_ARC_LAYOUT_KNOWN") == Some("1")
        && option_env!("HEX_PCM_ARC_CONSTRUCTOR_FAMILY") == Some("arc-audiodata-retained-vec-v1")
}

#[cfg(test)]
#[path = "../build_support/pcm_clippy.rs"]
mod build_clippy_tests;
#[cfg(test)]
#[path = "../build_support/pcm_gate.rs"]
mod build_gate_tests;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unverified_family_empty_capacity_and_overflow_stay_unknown() {
        assert!(PcmArcReceipt::capture(1, 1, false).is_none());
        assert!(PcmArcReceipt::capture(0, 1, true).is_none());
        assert!(PcmArcReceipt::capture(5, 4, true).is_none());
        assert!(PcmArcReceipt::capture(1, usize::MAX, true).is_none());
        let receipt = PcmArcReceipt::capture(5, 17, true).unwrap();
        assert_eq!(
            receipt.arc_payload_bytes,
            std::mem::size_of::<crate::AudioData>()
        );
        assert_eq!(
            receipt.buffer_bytes,
            17 * std::mem::size_of::<crate::Frame>()
        );
    }
}
