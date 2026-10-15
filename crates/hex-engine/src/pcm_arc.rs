//! One known constructor family; declaration is not physical allocator usage.
use std::{alloc::Layout, sync::atomic::AtomicUsize};
#[derive(Clone, Copy, Debug)]
pub(crate) struct PcmArcReceipt {
    len: usize,
    payload_bytes: usize,
    overhead_bytes: usize,
}
#[derive(Clone, Copy, Debug)]
enum PcmArcOrigin {
    ValidatedNonemptyVec,
    Unknown,
}
impl PcmArcReceipt {
    pub(crate) fn current(len: usize) -> Option<Self> {
        let known = native_layout_known();
        Self::capture(
            len,
            if known {
                PcmArcOrigin::ValidatedNonemptyVec
            } else {
                PcmArcOrigin::Unknown
            },
            known,
        )
    }
    fn capture(len: usize, origin: PcmArcOrigin, gate: bool) -> Option<Self> {
        if !gate || len == 0 || !matches!(origin, PcmArcOrigin::ValidatedNonemptyVec) {
            return None;
        }
        let payload = Layout::array::<crate::Frame>(len).ok()?;
        let requested = Layout::array::<AtomicUsize>(2)
            .ok()?
            .extend(payload)
            .ok()?
            .0
            .pad_to_align();
        Some(Self {
            len,
            payload_bytes: payload.size(),
            overhead_bytes: requested.size().checked_sub(payload.size())?,
        })
    }
    pub(crate) fn checked_parts(self, len: usize) -> Option<(usize, usize)> {
        (self.len == len).then_some((self.payload_bytes, self.overhead_bytes))
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
    native && option_env!("HEX_PCM_ARC_LAYOUT_KNOWN") == Some("1")
}

#[cfg(test)]
#[path = "../build_support/pcm_gate.rs"]
mod build_gate_tests;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsupported_origin_empty_gate_and_overflow_stay_unknown() {
        assert!(PcmArcReceipt::capture(1, PcmArcOrigin::Unknown, true).is_none());
        assert!(PcmArcReceipt::capture(0, PcmArcOrigin::ValidatedNonemptyVec, true).is_none());
        assert!(PcmArcReceipt::capture(1, PcmArcOrigin::ValidatedNonemptyVec, false).is_none());
        assert!(
            PcmArcReceipt::capture(usize::MAX, PcmArcOrigin::ValidatedNonemptyVec, true).is_none()
        );
        let receipt = PcmArcReceipt::capture(5, PcmArcOrigin::ValidatedNonemptyVec, true).unwrap();
        assert_eq!(
            receipt.checked_parts(5).unwrap().0,
            5 * std::mem::size_of::<crate::Frame>()
        );
        assert!(receipt.checked_parts(4).is_none());
    }
}
