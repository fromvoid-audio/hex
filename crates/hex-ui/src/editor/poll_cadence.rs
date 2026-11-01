//! Adaptive cadence for the editor's control poll (`EditorView::tick`).
//!
//! The poll runs every 16 ms while anything is active: playback (actual or
//! requested), a pending edit, recording, a held pointer, or a tick that changed
//! visible state (meters, playhead, worker ACKs, autoscroll). After
//! [`QUIET_TICKS_BEFORE_IDLE`] consecutive quiet ticks it backs off to
//! [`IDLE_INTERVAL`]. Keyboard and pointer input restart the 16 ms cadence
//! immediately, so input-driven edits are submitted without the idle delay.
//!
//! Live notes, monitoring and audio rendering do not depend on this poll; idle
//! back-off only delays unsolicited background notices (recovery, update checks)
//! by at most one idle interval.
use super::EditorView;
use gpui::{Context, Task, Window};
use std::time::Duration;

/// Poll interval while anything is active.
pub(super) const ACTIVE_INTERVAL: Duration = Duration::from_millis(16);
/// Poll interval once the editor is stopped and quiet.
pub(super) const IDLE_INTERVAL: Duration = Duration::from_millis(100);
/// Quiet active ticks (about half a second) before backing off.
pub(super) const QUIET_TICKS_BEFORE_IDLE: u32 = 30;

#[derive(Default)]
pub(super) struct PollCadence {
    quiet_ticks: u32,
    /// Total poll wakeups, for idle-cost regression tests and diagnostics.
    wakeups: u64,
    /// The running poll loop; dropping it cancels the pending timer.
    _task: Option<Task<()>>,
}

impl PollCadence {
    /// Record one completed tick and choose the delay before the next one.
    fn after_tick(&mut self, changed: bool, busy: bool) -> Duration {
        self.wakeups = self.wakeups.saturating_add(1);
        if changed || busy {
            self.quiet_ticks = 0;
        } else {
            self.quiet_ticks = self.quiet_ticks.saturating_add(1);
        }
        if self.idle() {
            IDLE_INTERVAL
        } else {
            ACTIVE_INTERVAL
        }
    }

    fn idle(&self) -> bool {
        self.quiet_ticks >= QUIET_TICKS_BEFORE_IDLE
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn wakeups(&self) -> u64 {
        self.wakeups
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn is_idle(&self) -> bool {
        self.idle()
    }
}

impl EditorView {
    /// Spawn a poll loop at the active cadence; the owner stores the task.
    pub(super) fn spawn_poll(window: &Window, cx: &mut Context<Self>) -> PollCadence {
        let task = cx.spawn_in(window, async move |this, cx| {
            let mut interval = ACTIVE_INTERVAL;
            loop {
                cx.background_executor().timer(interval).await;
                match this.update_in(cx, |view, window, cx| view.poll_tick(window, cx)) {
                    Ok(next) => interval = next,
                    // The editor or its window is gone.
                    Err(_) => break,
                }
            }
        });
        PollCadence {
            quiet_ticks: 0,
            wakeups: 0,
            _task: Some(task),
        }
    }

    /// Restart the poll loop at the active cadence, keeping the wakeup count.
    fn restart_poll(&mut self, window: &Window, cx: &mut Context<Self>) {
        let wakeups = self.poll_cadence.wakeups;
        self.poll_cadence = Self::spawn_poll(window, cx);
        self.poll_cadence.wakeups = wakeups;
    }

    /// Input arrived: leave idle immediately, or just reset the quiet count.
    pub(super) fn wake_poll(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.poll_cadence.idle() {
            self.restart_poll(window, cx);
        } else {
            self.poll_cadence.quiet_ticks = 0;
        }
    }

    fn poll_tick(&mut self, window: &Window, cx: &mut Context<Self>) -> Duration {
        let changed = self.tick(cx);
        if changed {
            cx.notify();
        }
        let busy = self.poll_busy() || window.is_pointer_dragging();
        self.poll_cadence.after_tick(changed, busy)
    }

    /// Activity that needs the 16 ms cadence even without a visible change.
    fn poll_busy(&self) -> bool {
        let actual_playing = self.audio.as_ref().is_some_and(|audio| {
            audio
                .control
                .status()
                .is_some_and(|status| status.playing.load(std::sync::atomic::Ordering::Acquire))
        });
        actual_playing || self.play_intent.desired_playing(actual_playing) || self.edit_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backs_off_only_after_sustained_quiet_and_returns_on_activity() {
        let mut cadence = PollCadence::default();
        for _ in 1..QUIET_TICKS_BEFORE_IDLE {
            assert_eq!(cadence.after_tick(false, false), ACTIVE_INTERVAL);
        }
        assert_eq!(cadence.after_tick(false, false), IDLE_INTERVAL);
        assert_eq!(cadence.after_tick(false, false), IDLE_INTERVAL);
        assert_eq!(
            cadence.after_tick(true, false),
            ACTIVE_INTERVAL,
            "visible change"
        );
        for _ in 0..QUIET_TICKS_BEFORE_IDLE {
            cadence.after_tick(false, false);
        }
        assert_eq!(cadence.after_tick(false, true), ACTIVE_INTERVAL, "busy");
        assert_eq!(cadence.wakeups, u64::from(QUIET_TICKS_BEFORE_IDLE) * 2 + 3);
    }

    #[test]
    fn busy_never_idles() {
        let mut cadence = PollCadence::default();
        for _ in 0..10 * QUIET_TICKS_BEFORE_IDLE {
            assert_eq!(cadence.after_tick(false, true), ACTIVE_INTERVAL);
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "poll_cadence_ui_tests.rs"]
mod ui_tests;
