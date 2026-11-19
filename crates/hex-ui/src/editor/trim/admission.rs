//! Trim gesture admission from modal and pending-work state.

pub(super) fn trim_drag_admitted(
    opening_project: bool,
    close_prompt_open: bool,
    worker_requests_pending: usize,
    pending_requests: usize,
) -> bool {
    !opening_project && !close_prompt_open && worker_requests_pending == 0 && pending_requests == 0
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::trim_drag_admitted;

    #[test]
    fn trim_admission_waits_for_open_close_and_canonical_work() {
        assert!(trim_drag_admitted(false, false, 0, 0));
        assert!(!trim_drag_admitted(true, false, 0, 0));
        assert!(!trim_drag_admitted(false, true, 0, 0));
        assert!(!trim_drag_admitted(false, false, 1, 0));
        assert!(!trim_drag_admitted(false, false, 0, 1));
    }
}
