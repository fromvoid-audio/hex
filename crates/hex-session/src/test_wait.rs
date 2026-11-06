//! Test-only wait bounds. Test helpers block on the real condition (an event
//! channel `recv_timeout`) or sleep between polls of it, bounded only against
//! hangs. The bound is not a latency budget: unoptimized worker preparation
//! can take seconds on a loaded machine, and a yield spin would take CPU from
//! the very workers being waited for.
use std::time::Duration;

/// Generous hang bound shared by hex-session test waits.
pub(crate) const HANG_BOUND: Duration = Duration::from_secs(60);

/// Sleep for one poll interval in a loop that checks its own `HANG_BOUND`
/// deadline, instead of spinning on `thread::yield_now`.
pub(crate) fn pause() {
    std::thread::sleep(Duration::from_millis(1));
}
