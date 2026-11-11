//! Bounded test-only observations of the exact shared path painter inputs.
use crate::theme::{self, FadeHandleState, FadeHandleStyle};
use gpui::{Bounds, Pixels, VisualTestContext};
use std::{cell::RefCell, collections::VecDeque};

thread_local! {
    static PAINTS: RefCell<VecDeque<(Bounds<Pixels>, FadeHandleStyle)>> = const { RefCell::new(VecDeque::new()) };
}

pub(super) fn record(bounds: Bounds<Pixels>, style: FadeHandleStyle) {
    PAINTS.with_borrow_mut(|paints| {
        if paints.len() == 512 {
            paints.pop_front();
        }
        paints.push_back((bounds, style));
    });
}

fn painted_style(visual: &mut VisualTestContext, selector: &'static str) -> FadeHandleStyle {
    let bounds = visual.debug_bounds(selector).expect("actual fade handle");
    PAINTS.with_borrow(|paints| {
        paints
            .iter()
            .rev()
            .find(|(paint, _)| *paint == bounds)
            .map(|(_, style)| *style)
            .expect("actual shared fade path paint")
    })
}

#[track_caller]
pub(in crate::editor) fn assert_style(
    visual: &mut VisualTestContext,
    selector: &'static str,
    state: FadeHandleState,
) {
    assert_eq!(
        painted_style(visual, selector),
        theme::fade_handle_style(state),
        "{selector}: {state:?}"
    );
}

pub(in crate::editor) fn assert_not_active(visual: &mut VisualTestContext, selector: &'static str) {
    assert_ne!(
        painted_style(visual, selector),
        theme::fade_handle_style(FadeHandleState::Active),
        "{selector}: capture retired with terminal"
    );
}
