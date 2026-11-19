//! Wall-clock hang guards for tests that wait on workers, services, output
//! retirement or editor drains.
//!
//! The bound is sized for a loaded developer machine (owner applications plus a
//! full local CI build), so ordinary scheduling delay cannot fail a test while a
//! real hang still fails within a finite time. Simulated controller time is a
//! separate value and must not use this guard.
use std::time::{Duration, Instant};

/// Maximum wall-clock wait for asynchronous test progress.
pub(crate) const HANG_GUARD: Duration = Duration::from_secs(30);

/// Deadline for one asynchronous wait, `HANG_GUARD` from now.
pub(crate) fn hang_deadline() -> Instant {
    Instant::now() + HANG_GUARD
}
