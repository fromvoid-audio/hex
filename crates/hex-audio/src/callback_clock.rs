//! Bounded, control-initialized audio-callback clock contract (`#8324`).
//!
//! # Source-grounded platform evidence (macOS arm64 / Darwin)
//!
//! - Rust 1.94's `std::time::Instant::now()` on macOS calls
//!   `clock_gettime(CLOCK_UPTIME_RAW)` and unwraps the libc return value.
//! - Apple Libc (`gen/clock_gettime.c`) routes `CLOCK_UPTIME_RAW` through
//!   `mach_timebase_info` (cached on first successful lookup in XNU
//!   `libsyscall/wrappers/mach_timebase_info.c`) and `mach_absolute_time`
//!   (`libsyscall/wrappers/mach_absolute_time.s`, arm64 `CALC_MACH_ABSOLUTE_TIME`).
//!   That arm64 routine retries across a commpage timebase offset change and
//!   falls back to a kernel trap if userspace clock access is unavailable.
//! - A local macOS 26.6.2 arm64 probe confirmed `kAudioUnitProperty_CPULoad`
//!   returns `kAudioUnitErr_InvalidProperty (-10879)` on both AUHAL and the
//!   default output unit, so AudioUnit CPU load is not a portable replacement.
//! - CPAL's host-time ratio is prevalidated on control at stream open (`#8466`),
//!   but device-supplied callback timestamps do not measure a later `rtrb` queue
//!   commit or callback exit.
//!
//! # Contract
//!
//! - [`CallbackClock::prewarm_monotonic`] performs one `Instant::now()` read on
//!   the **control** thread before any audio callback starts, ensuring Libc's
//!   `mach_timebase_info` cache is initialized off-callback and establishing a
//!   shared monotonic origin for callback-to-worker timestamp correlation.
//! - [`CallbackClock::unavailable`] provides an explicit zero-syscall,
//!   zero-wait, zero-allocation fallback policy whose [`CallbackClock::try_now_ns`]
//!   always returns `None` without panicking or fabricating exact timing.
//! - [`checked_elapsed_ns`] uses checked subtraction (`end >= start`) so
//!   unavailability or non-monotonicity yields `None` rather than fabricating
//!   `0 ns`.
//! - This boundary adds zero `unsafe` workspace code and does not claim to
//!   eliminate kernel commpage retry/trap paths on arbitrary hardware; physical
//!   device deadline qualification remains separate.

use std::sync::OnceLock;
use std::time::Instant;

/// Process-wide control-prewarmed monotonic origin so callbacks and paired
/// native plugin workers started on control share the exact same monotonic
/// epoch without touching `OnceLock` inside the audio callback.
static SHARED_MONOTONIC_ORIGIN: OnceLock<Instant> = OnceLock::new();

/// Admitted timing policy for a callback/worker clock pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackClockPolicy {
    /// Control-prewarmed monotonic uptime clock (`CLOCK_UPTIME_RAW` on macOS).
    MonotonicUptimeRaw,
    /// Explicit unavailable fallback: never queries the OS clock in callbacks.
    Unavailable,
}

/// Control-initialized, fallible monotonic clock handle shared by value (`Copy`)
/// between the audio callback and its paired worker without locks or heap state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallbackClock {
    origin: Option<Instant>,
}

impl CallbackClock {
    /// Prewarm the monotonic clock on the control thread before callback start.
    ///
    /// Performs the initial `CLOCK_UPTIME_RAW` / `mach_timebase_info` read on
    /// control so first-use timebase caching never occurs inside an audio
    /// callback. All control-prewarmed handles share the same monotonic origin
    /// by value (`Copy`), so callback reads never touch synchronization state.
    pub fn prewarm_monotonic() -> Self {
        let origin = *SHARED_MONOTONIC_ORIGIN.get_or_init(Instant::now);
        Self {
            origin: Some(origin),
        }
    }

    /// Explicit unavailable clock policy: zero syscalls, zero waits, zero
    /// allocations, and zero fabricated timestamps.
    pub const fn unavailable() -> Self {
        Self { origin: None }
    }

    /// Policy governing this clock instance.
    pub const fn policy(self) -> CallbackClockPolicy {
        match self.origin {
            Some(_) => CallbackClockPolicy::MonotonicUptimeRaw,
            None => CallbackClockPolicy::Unavailable,
        }
    }

    /// True when the clock is configured to read monotonic timestamps.
    pub const fn is_available(self) -> bool {
        self.origin.is_some()
    }

    /// Read the current monotonic timestamp as nanoseconds since the
    /// control-captured origin, or `None` when unavailable, non-monotonic, or
    /// overflowing `u64`.
    #[inline]
    pub fn try_now_ns(self) -> Option<u64> {
        let origin = self.origin?;
        let elapsed = Instant::now().checked_duration_since(origin)?;
        u64::try_from(elapsed.as_nanos()).ok()
    }

    /// Read the current `Instant` when available, or `None` when unavailable.
    #[inline]
    pub fn now_instant(self) -> Option<Instant> {
        self.origin.map(|_| Instant::now())
    }

    /// Measure nanoseconds elapsed since `start_ns`, returning `None` when
    /// either endpoint is unavailable or the clock did not advance monotonically.
    #[inline]
    pub fn try_elapsed_ns(self, start_ns: Option<u64>) -> Option<u64> {
        let start = start_ns?;
        let end = self.try_now_ns()?;
        end.checked_sub(start)
    }
}

/// Checked nanosecond difference between two optional timestamps from the same
/// [`CallbackClock`] origin. Never fabricates `0` on unavailability or regression.
#[inline]
pub const fn checked_elapsed_ns(start_ns: Option<u64>, end_ns: Option<u64>) -> Option<u64> {
    match (start_ns, end_ns) {
        (Some(start), Some(end)) => end.checked_sub(start),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prewarmed_clock_is_monotonic_and_unavailable_clock_never_fabricates_timing() {
        let clock = CallbackClock::prewarm_monotonic();
        let peer_clock = CallbackClock::prewarm_monotonic();
        assert_eq!(clock, peer_clock);
        assert_eq!(clock.policy(), CallbackClockPolicy::MonotonicUptimeRaw);
        assert!(clock.is_available());
        let first = clock.try_now_ns().expect("prewarmed clock returns ns");
        let second = peer_clock.try_now_ns().expect("second read returns ns");
        assert!(second >= first);
        assert_eq!(
            checked_elapsed_ns(Some(first), Some(second)),
            Some(second - first)
        );
        // Backwards reading never fabricates 0 or panics.
        assert_eq!(checked_elapsed_ns(Some(500), Some(499)), None);
        assert_eq!(checked_elapsed_ns(None, Some(500)), None);
        assert_eq!(checked_elapsed_ns(Some(500), None), None);

        let unavailable = CallbackClock::unavailable();
        assert_eq!(unavailable.policy(), CallbackClockPolicy::Unavailable);
        assert!(!unavailable.is_available());
        assert_eq!(unavailable.try_now_ns(), None);
        assert_eq!(unavailable.now_instant(), None);
        assert_eq!(unavailable.try_elapsed_ns(Some(100)), None);
        assert_eq!(unavailable.try_elapsed_ns(None), None);
    }

    #[cfg(feature = "allocation-probe")]
    #[test]
    fn callback_clock_reads_allocate_zero_bytes_in_both_policies() {
        let monotonic = CallbackClock::prewarm_monotonic();
        let unavailable = CallbackClock::unavailable();
        let measured = allocation_counter::measure(|| {
            for _ in 0..256 {
                let start = monotonic.try_now_ns();
                let elapsed = monotonic.try_elapsed_ns(start);
                assert!(elapsed.is_some());
                assert!(unavailable.try_now_ns().is_none());
                assert!(unavailable.try_elapsed_ns(start).is_none());
            }
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current,
            ),
            (0, 0, 0, 0)
        );
    }
}
