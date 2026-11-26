//! Lightweight condvar wakeup primitive with monotonic sequence numbers to
//! replace fixed-interval `thread::sleep` polling across control workers.
use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[derive(Default)]
pub(in crate::service) struct WakeSignal {
    state: Mutex<u64>,
    cvar: Condvar,
}

impl WakeSignal {
    /// Capture the current notification sequence before checking a condition.
    pub(in crate::service) fn snapshot(&self) -> u64 {
        self.state.lock().map(|guard| *guard).unwrap_or(0)
    }

    /// Wake all waiters observing a sequence prior to this notification.
    pub(in crate::service) fn notify(&self) {
        if let Ok(mut guard) = self.state.lock() {
            *guard = guard.wrapping_add(1);
            self.cvar.notify_all();
        }
    }

    /// Wait up to `timeout` for a notification after `seen`.
    /// Returns immediately if a notification already occurred since `seen`.
    pub(in crate::service) fn wait_since(&self, seen: u64, timeout: Duration) {
        if timeout.is_zero() {
            return;
        }
        if let Ok(guard) = self.state.lock()
            && *guard == seen
        {
            let _ = self
                .cvar
                .wait_timeout_while(guard, timeout, |seq| *seq == seen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn wake_signal_returns_immediately_when_notified_before_or_during_wait() {
        let signal = Arc::new(WakeSignal::default());

        // Notified between snapshot and wait_since -> zero sleep.
        let seq = signal.snapshot();
        signal.notify();
        let t0 = Instant::now();
        signal.wait_since(seq, Duration::from_secs(2));
        assert!(t0.elapsed() < Duration::from_millis(50));

        // Notified from another thread during wait_since -> wakes promptly.
        let seq = signal.snapshot();
        let notifier = Arc::clone(&signal);
        let handle = std::thread::spawn(move || {
            notifier.notify();
        });
        let t1 = Instant::now();
        signal.wait_since(seq, Duration::from_secs(2));
        assert!(t1.elapsed() < Duration::from_millis(200));
        handle.join().unwrap();
    }
}
