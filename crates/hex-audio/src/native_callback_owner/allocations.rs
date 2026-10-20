//! Sealed allocation lineage captured from the actual fresh private carrier.
//! This records constructor custody, not a live borrow of callback-owned fields.
//! Payload children and all later transfer destinations remain separate proofs.
use super::{Guard, GuardState, Terminal};
use std::{alloc::Layout, sync::atomic::AtomicUsize};

#[derive(Clone, Copy)]
pub(super) struct CarrierAllocation {
    body: usize,
    cell: usize,
    body_layout: Layout,
    cell_layout: Layout,
}

fn layout_known() -> bool {
    let target = match option_env!("HEX_NATIVE_CARRIER_LAYOUT_TARGET") {
        Some("x86_64-unknown-linux-gnu") => cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )),
        Some("aarch64-apple-darwin") => cfg!(all(target_arch = "aarch64", target_os = "macos")),
        _ => false,
    };
    target && option_env!("HEX_NATIVE_CARRIER_LAYOUT_KNOWN") == Some("1")
}

impl CarrierAllocation {
    pub(super) fn capture<T>(guard: &Guard<T>) -> Option<Self> {
        Self::capture_qualified(guard, layout_known())
    }
    fn capture_qualified<T>(guard: &Guard<T>, supported: bool) -> Option<Self> {
        if !supported || std::mem::size_of::<T>() == 0 {
            return None;
        }
        // Only reserve(value)'s actual fresh Box and sized Arc::new cell. The
        // caller cannot supply an address, layout, receipt or adopted owner.
        let cell = guard.shared_cell()?;
        let GuardState::Live { owner, .. } = &guard.state else {
            return None;
        };
        let cell_layout = Layout::array::<AtomicUsize>(2)
            .ok()?
            .extend(Layout::for_value(cell.as_ref()))
            .ok()?
            .0
            .pad_to_align();
        Some(Self {
            body: owner.as_ref() as *const T as usize,
            cell: std::sync::Arc::as_ptr(cell) as usize,
            body_layout: Layout::new::<T>(),
            cell_layout,
        })
    }
    pub(super) fn requested_bytes(self) -> Option<usize> {
        self.body_layout.size().checked_add(self.cell_layout.size())
    }
    pub(super) fn matches_terminal<T>(self, terminal: &Terminal<T>) -> bool {
        terminal
            .cell
            .as_ref()
            .is_some_and(|cell| self.cell == std::sync::Arc::as_ptr(cell) as usize)
            && self.body_layout == Layout::new::<T>()
    }
    pub(super) fn matches_body<T>(self, body: &T) -> bool {
        self.body == body as *const T as usize && self.body_layout == Layout::new::<T>()
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests {
    use super::*;
    #[repr(align(64))]
    struct Aligned([u8; 3]);
    #[test]
    fn carrier_lineage_is_fresh_exact_aligned_and_conserved_through_terminal_recovery() {
        assert!(
            layout_known(),
            "native carrier constructor gate must execute"
        );
        eprintln!("SUPPORTED native-carrier-sized-box-arc-new-v1");
        let (mut guard, mut terminal) = super::super::reserve(Aligned([1, 2, 3]));
        let record = CarrierAllocation::capture(&guard).unwrap();
        let body = guard.owner_ref().unwrap();
        assert_eq!(body.0, [1, 2, 3]);
        let cell_body = Layout::new::<std::sync::OnceLock<std::sync::Mutex<Box<Aligned>>>>();
        let independently_padded = (2 * std::mem::size_of::<AtomicUsize>())
            .div_ceil(cell_body.align())
            * cell_body.align()
            + cell_body.size();
        let expected_cell = independently_padded
            .div_ceil(cell_body.align().max(std::mem::align_of::<AtomicUsize>()))
            * cell_body.align().max(std::mem::align_of::<AtomicUsize>());
        assert_eq!(
            record.requested_bytes(),
            Some(std::mem::size_of::<Aligned>() + expected_cell)
        );
        assert!(record.matches_body(body));
        assert!(record.matches_terminal(&terminal));
        let (other, wrong_terminal) = super::super::reserve(Aligned([1, 2, 3]));
        assert!(!record.matches_body(other.owner_ref().unwrap()));
        assert!(!record.matches_terminal(&wrong_terminal));
        let mut moved = Some(guard);
        guard = moved.take().unwrap();
        let measured = allocation_counter::measure(|| {
            for _ in 0..128 {
                assert!(record.matches_body(guard.owner_ref().unwrap()));
                assert!(record.matches_terminal(&terminal));
                assert!(record.requested_bytes().unwrap() > 0);
            }
            guard.close();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(CarrierAllocation::capture(&guard).is_none());
        let recovered = terminal.take().unwrap();
        assert!(record.matches_body(recovered.as_ref()));
        assert_eq!(recovered.0, [1, 2, 3]);
    }
    #[test]
    fn carrier_lineage_matches_actual_constructor_requests_for_distinct_equal_bodies() {
        assert!(
            layout_known(),
            "native carrier constructor gate must execute"
        );
        eprintln!("SUPPORTED native carrier actual constructor allocation requests");
        let mut owners = None;
        let measured = allocation_counter::measure(|| {
            owners = Some([
                super::super::reserve(Aligned([4, 5, 6])),
                super::super::reserve(Aligned([4, 5, 6])),
            ]);
        });
        let owners = owners.unwrap();
        let first = CarrierAllocation::capture(&owners[0].0).unwrap();
        let second = CarrierAllocation::capture(&owners[1].0).unwrap();
        assert_ne!(first.body, second.body);
        assert_ne!(first.cell, second.cell);
        let expected = first.requested_bytes().unwrap() + second.requested_bytes().unwrap();
        assert_eq!(measured.count_total, 4);
        assert_eq!(measured.count_current, 4);
        assert_eq!(measured.bytes_total, u64::try_from(expected).unwrap());
        assert_eq!(measured.bytes_current, i64::try_from(expected).unwrap());
        assert!(expected > first.requested_bytes().unwrap());
        assert!(u64::try_from(expected - 1).unwrap() < measured.bytes_total);
    }
    #[test]
    fn carrier_lineage_refuses_unsupported_gate_closed_and_zero_sized_constructors() {
        let (mut guard, _) = super::super::reserve(7u32);
        assert!(CarrierAllocation::capture_qualified(&guard, false).is_none());
        guard.close();
        assert!(CarrierAllocation::capture_qualified(&guard, true).is_none());
        let (zero, _) = super::super::reserve(());
        assert!(CarrierAllocation::capture_qualified(&zero, true).is_none());
    }
}

#[cfg(test)]
#[path = "../../build_support/carrier_gate.rs"]
mod constructor_gate_tests;
