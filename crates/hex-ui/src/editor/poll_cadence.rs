//! Adaptive cadence for the editor's control poll (`EditorView::tick`).
//!
//! The poll runs every 16 ms while anything is active: playback (actual or
//! requested), a pending edit or worker request, queued or unacknowledged
//! transport commands, recording, a held pointer, or a tick that changed
//! visible state (meters, playhead, worker ACKs, autoscroll). After
//! [`QUIET_TICKS_BEFORE_IDLE`] consecutive quiet ticks it backs off to
//! [`IDLE_INTERVAL`].
//!
//! Every producer of poll work calls [`EditorView::wake_poll`]: keyboard input
//! (window keystroke interceptor), pointer presses (root capture listener),
//! request admission (`enqueue_request`), every worker submission
//! (`worker_submitted`) and every transport entry point (play/stop/seek,
//! metronome, session launch, prepared seek). A wake interrupts the sleeping
//! idle timer: the next tick runs at once, without the idle delay, and the 16 ms
//! cadence resumes. As a last line of defence, rendering wakes the poll when it
//! finds busy work while idle, so a producer that only notifies is still caught.
//!
//! Unsolicited background notices have no wake source: hex-session, the audio
//! status and MIDI port handles expose only non-blocking `try_*` polls, with no
//! waker hook. Those notices (audio device loss/recovery, update checks,
//! controller-port changes, hardware held-note display) therefore wait at most
//! one [`IDLE_INTERVAL`] (100 ms) while idle. Every worker *response* is
//! solicited: its request keeps `worker_requests_pending` non-zero, so the poll
//! stays at 16 ms until the response arrives. Read-only parameter queries use
//! their panel's pending read instead of the mutation counter, so they cannot
//! block editor gestures or add a pending-edit status badge. Live notes, monitoring and audio
//! rendering do not depend on this poll.
use super::EditorView;
use gpui::{Context, Task, Window};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Poll, Waker};
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
    signal: PollSignal,
    /// The running poll loop; dropping it cancels the pending timer.
    _task: Option<Task<()>>,
}

/// One-bit wake latch shared by the editor and its poll loop.
///
/// UI-thread only in practice; the atomic flag and short waker mutex never
/// touch the audio callback.
#[derive(Clone, Default)]
struct PollSignal(Arc<SignalState>);

#[derive(Default)]
struct SignalState {
    woken: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl PollSignal {
    fn wake(&self) {
        self.0.woken.store(true, Ordering::Release);
        let waker = self
            .0
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn take(&self) -> bool {
        self.0.woken.swap(false, Ordering::AcqRel)
    }
}

/// Resolves when the timer fires or the signal is woken, whichever is first.
struct WakeOrTimer<'a> {
    signal: &'a PollSignal,
    timer: Task<()>,
}

impl Future for WakeOrTimer<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<()> {
        if self.signal.take() {
            return Poll::Ready(());
        }
        *self
            .signal
            .0
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(cx.waker().clone());
        // A wake between the first check and registering the waker.
        if self.signal.take() {
            return Poll::Ready(());
        }
        Pin::new(&mut self.timer).poll(cx)
    }
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
    /// Spawn the poll loop at the active cadence; the owner stores the cadence.
    pub(super) fn spawn_poll(window: &Window, cx: &mut Context<Self>) -> PollCadence {
        let signal = PollSignal::default();
        let loop_signal = signal.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let mut interval = ACTIVE_INTERVAL;
            loop {
                let timer = cx.background_executor().timer(interval);
                if interval == IDLE_INTERVAL {
                    WakeOrTimer {
                        signal: &loop_signal,
                        timer,
                    }
                    .await;
                } else {
                    timer.await;
                }
                // Wakes during an active sleep add no extra tick.
                loop_signal.take();
                match this.update_in(cx, |view, window, cx| view.poll_tick(window, cx)) {
                    Ok(next) => interval = next,
                    // The editor or its window is gone.
                    Err(_) => break,
                }
            }
        });
        PollCadence {
            signal,
            _task: Some(task),
            ..PollCadence::default()
        }
    }

    /// Poll work was produced: leave idle at once, or reset the quiet count.
    pub(super) fn wake_poll(&mut self) {
        let idle = self.poll_cadence.idle();
        self.poll_cadence.quiet_ticks = 0;
        if idle {
            self.poll_cadence.signal.wake();
        }
    }

    /// Rendering found busy work while idle (a producer that only notified).
    pub(super) fn wake_poll_if_busy(&mut self) {
        if self.poll_cadence.idle() && self.poll_busy() {
            self.wake_poll();
        }
    }

    /// One request was submitted to the project worker.
    pub(super) fn worker_submitted(&mut self) {
        self.worker_requests_pending = self.worker_requests_pending.saturating_add(1);
        self.wake_poll();
    }

    fn poll_tick(&mut self, window: &Window, cx: &mut Context<Self>) -> Duration {
        super::device_panel::vst3_parameters::tick(self, cx);
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
        actual_playing
            || self.play_intent.desired_playing(actual_playing)
            || self.edit_pending()
            || self.automation_close_preflight.waiting()
            || self.automation_take_recovery.polling()
            || super::device_panel::vst3_parameters::query_pending(self)
            || self.transport_pending()
    }

    /// Queued or unacknowledged transport commands, so their ACKs keep 16 ms.
    fn transport_pending(&self) -> bool {
        !self.pending_audio_commands.is_empty()
            || self.pending_seek_ticket.is_some()
            || self.pending_metronome_ticket.is_some()
            || self.pending_session_ticket.is_some()
            || self.deferred_play.is_some()
            || self.seek_pending()
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
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
