//! Shared display-only block caret for native text and numeric editing.
//!
//! The caret blinks in fixed ~500 ms steps. Each visible phase change is
//! scheduled by one executor timer that notifies only the input entity, so an
//! idle focused field costs two window frames per second instead of a frame loop.
//! Input activity or moving/resizing the caret restarts the visible phase,
//! including edits that leave text and geometry unchanged.
//!
//! Selection, IME and source text stay with their input owners. Keyed state,
//! including the pending timer, is dropped when that caret stops painting
//! (focus loss or removal), which cancels further scheduling. GPUI retains the
//! last painted frame's keyed state until its next draw, so at most the one
//! already-scheduled wake may still notify the owner; it schedules nothing more.
use gpui::{App, Bounds, EntityId, PaintQuad, Pixels, Task, Window};
use std::time::{Duration, Instant};

/// Length of one visible or hidden blink phase.
const BLINK_PHASE: Duration = Duration::from_millis(500);
/// Visible caret opacity over the glyph it covers.
const OPACITY: f32 = 0.5;
/// Timer slack so a wake always lands inside the next phase.
const WAKE_SLACK: Duration = Duration::from_millis(1);

struct Blink {
    epoch: Instant,
    anchor: Bounds<Pixels>,
    activity: u64,
    /// Phase the pending timer wakes for, with the task that owns the wake.
    wake: Option<(u128, Task<()>)>,
}

fn phase(elapsed: Duration) -> u128 {
    elapsed.as_nanos() / BLINK_PHASE.as_nanos()
}

fn visible(phase: u128) -> bool {
    phase & 1 == 0
}

/// Time from `elapsed` until the next phase begins, plus slack.
fn until_next_phase(elapsed: Duration) -> Duration {
    let into = Duration::from_nanos((elapsed.as_nanos() % BLINK_PHASE.as_nanos()) as u64);
    BLINK_PHASE.saturating_sub(into) + WAKE_SLACK
}

pub(crate) fn paint(
    mut block: PaintQuad,
    owner: EntityId,
    activity: u64,
    window: &mut Window,
    cx: &mut App,
) {
    let now = cx.background_executor().now();
    let anchor = block.bounds;
    let state = window.use_keyed_state(("text-block-caret", owner.as_u64()), cx, |_, _| Blink {
        epoch: now,
        anchor,
        activity,
        wake: None,
    });
    let (current, elapsed, needs_wake) = state.update(cx, |blink, _| {
        if blink.anchor != anchor || blink.activity != activity {
            blink.epoch = now;
            blink.anchor = anchor;
            blink.activity = activity;
            blink.wake = None;
        }
        let elapsed = now.saturating_duration_since(blink.epoch);
        let current = phase(elapsed);
        let needs_wake = blink
            .wake
            .as_ref()
            .is_none_or(|(target, _)| *target != current + 1);
        (current, elapsed, needs_wake)
    });
    if needs_wake {
        let delay = until_next_phase(elapsed);
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(delay).await;
            // The app may be gone at shutdown; there is then nothing to repaint.
            let _ = cx.update(|cx| cx.notify(owner));
        });
        state.update(cx, |blink, _| blink.wake = Some((current + 1, task)));
    }
    if visible(current) {
        block.background = block.background.opacity(OPACITY);
        window.paint_quad(block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blink_alternates_in_fixed_phases_starting_visible() {
        assert!(visible(phase(Duration::ZERO)));
        assert!(visible(phase(Duration::from_millis(499))));
        assert!(!visible(phase(Duration::from_millis(500))));
        assert!(!visible(phase(Duration::from_millis(999))));
        assert!(visible(phase(Duration::from_millis(1000))));
    }

    #[test]
    fn wake_lands_just_inside_the_next_phase() {
        for ms in [0, 1, 250, 499, 500, 731, 10_000] {
            let elapsed = Duration::from_millis(ms);
            let wake = elapsed + until_next_phase(elapsed);
            assert_eq!(phase(wake), phase(elapsed) + 1, "elapsed {ms} ms");
        }
    }
}
