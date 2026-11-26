//! Validated Mach host timebase conversion and monotonic hardware packet clocks.
//!
//! CoreMIDI delivers `MIDIPacket::timeStamp` in host Mach ticks (`mach_absolute_time`
//! units). Converting ticks to nanoseconds requires a validated positive rational
//! timebase (`numer / denom` nanoseconds per tick). No `unsafe` code is used:
//! `MachTimebase::host()` queries the OS timebase frequency once off-callback
//! (cached in a `OnceLock`) with a validated architecture fallback.

use crate::NoteEvent;
use std::sync::OnceLock;

/// One MIDI packet slice paired with its raw hardware host timestamp ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedPacket<'a> {
    pub timestamp_ticks: u64,
    pub data: &'a [u8],
}

/// An ordered decoded MIDI event paired with its monotonic nanosecond timestamp.
///
/// `timestamp_ns` is converted from the CoreMIDI hardware packet timestamp via
/// [`MachTimebase`] (`0` when delivered through untimed byte-only fixtures).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimestampedNoteEvent {
    pub timestamp_ns: u64,
    pub event: NoteEvent,
}

/// Validated positive rational conversion from Mach host ticks to nanoseconds:
/// `nanos = floor(ticks * numer / denom)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MachTimebase {
    numer: u32,
    denom: u32,
}

const fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

impl MachTimebase {
    /// Identity 1:1 nanosecond timebase (1 tick = 1 ns).
    pub const NANOSECONDS: Self = Self { numer: 1, denom: 1 };

    /// Apple Silicon 24 MHz host timebase (`1_000_000_000 / 24_000_000 = 125 / 3` ns/tick).
    pub const APPLE_SILICON_24MHZ: Self = Self {
        numer: 125,
        denom: 3,
    };

    /// Constructs a validated timebase, reducing `numer / denom` by their GCD.
    /// Rejects zero numerator or zero denominator.
    pub fn new(numer: u32, denom: u32) -> Option<Self> {
        if numer == 0 || denom == 0 {
            return None;
        }
        let divisor = gcd_u64(u64::from(numer), u64::from(denom)) as u32;
        Some(Self {
            numer: numer / divisor,
            denom: denom / divisor,
        })
    }

    /// Derives a validated timebase from a host tick frequency in Hz
    /// (`1_000_000_000 / freq_hz` ns/tick), reduced by GCD.
    pub fn from_frequency_hz(freq_hz: u64) -> Option<Self> {
        if freq_hz == 0 {
            return None;
        }
        let ns_per_sec = 1_000_000_000_u64;
        let divisor = gcd_u64(ns_per_sec, freq_hz);
        let numer = u32::try_from(ns_per_sec / divisor).ok()?;
        let denom = u32::try_from(freq_hz / divisor).ok()?;
        Self::new(numer, denom)
    }

    /// Returns the host platform's validated Mach timebase, queried once off-callback.
    pub fn host() -> Self {
        static HOST: OnceLock<MachTimebase> = OnceLock::new();
        *HOST.get_or_init(detect_host_timebase)
    }

    pub const fn numer(self) -> u32 {
        self.numer
    }

    pub const fn denom(self) -> u32 {
        self.denom
    }

    /// Converts raw host ticks to nanoseconds using checked `u128` arithmetic.
    /// Returns `None` if the converted nanosecond value exceeds `u64::MAX`.
    pub fn ticks_to_nanos(self, ticks: u64) -> Option<u64> {
        let scaled = u128::from(ticks).checked_mul(u128::from(self.numer))?;
        let nanos = scaled / u128::from(self.denom);
        u64::try_from(nanos).ok()
    }

    /// Converts raw host ticks to nanoseconds, saturating at `u64::MAX` on overflow.
    pub fn ticks_to_nanos_saturating(self, ticks: u64) -> u64 {
        self.ticks_to_nanos(ticks).unwrap_or(u64::MAX)
    }

    /// Converts nanoseconds to raw host ticks (`floor(nanos * denom / numer)`)
    /// using checked `u128` arithmetic. Returns `None` on `u64` overflow.
    pub fn nanos_to_ticks(self, nanos: u64) -> Option<u64> {
        let scaled = u128::from(nanos).checked_mul(u128::from(self.denom))?;
        let ticks = scaled / u128::from(self.numer);
        u64::try_from(ticks).ok()
    }

    /// Converts nanoseconds to raw host ticks, saturating at `u64::MAX` on overflow.
    pub fn nanos_to_ticks_saturating(self, nanos: u64) -> u64 {
        self.nanos_to_ticks(nanos).unwrap_or(u64::MAX)
    }
}

fn detect_host_timebase() -> MachTimebase {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.tbfrequency"])
            .output()
            && output.status.success()
            && let Ok(text) = std::str::from_utf8(&output.stdout)
            && let Ok(freq_hz) = text.trim().parse::<u64>()
            && let Some(timebase) = MachTimebase::from_frequency_hz(freq_hz)
        {
            return timebase;
        }
        #[cfg(target_arch = "aarch64")]
        {
            MachTimebase::APPLE_SILICON_24MHZ
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            MachTimebase::NANOSECONDS
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        MachTimebase::NANOSECONDS
    }
}

/// Stateful per-connection clock that converts raw CoreMIDI host timestamps to
/// monotonic non-decreasing nanoseconds.
///
/// CoreMIDI packets with `timestamp_ticks == 0` ("now" / immediate) or slightly
/// regressive hardware timestamps never move the stream backwards: they clamp
/// to the last observed nanosecond instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MonotonicTimestampClock {
    last_nanos: u64,
}

impl MonotonicTimestampClock {
    pub fn last_nanos(&self) -> u64 {
        self.last_nanos
    }

    pub fn advance(&mut self, timebase: MachTimebase, timestamp_ticks: u64) -> u64 {
        if timestamp_ticks == 0 {
            return self.last_nanos;
        }
        let converted = timebase.ticks_to_nanos_saturating(timestamp_ticks);
        if converted > self.last_nanos {
            self.last_nanos = converted;
        }
        self.last_nanos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mach_timebase_validates_reduces_and_converts_accurately() {
        assert_eq!(MachTimebase::new(0, 1), None);
        assert_eq!(MachTimebase::new(1, 0), None);
        assert_eq!(MachTimebase::from_frequency_hz(0), None);

        let apple_silicon = MachTimebase::from_frequency_hz(24_000_000).unwrap();
        assert_eq!(apple_silicon, MachTimebase::APPLE_SILICON_24MHZ);
        assert_eq!(apple_silicon.numer(), 125);
        assert_eq!(apple_silicon.denom(), 3);
        // 24_000_000 ticks at 24 MHz = exactly 1_000_000_000 ns (1 second).
        assert_eq!(
            apple_silicon.ticks_to_nanos(24_000_000),
            Some(1_000_000_000)
        );
        // 24 ticks = 1 microsecond = 1000 ns.
        assert_eq!(apple_silicon.ticks_to_nanos(24), Some(1_000));
        // Overflow returns None on checked conversion and u64::MAX on saturating.
        assert_eq!(apple_silicon.ticks_to_nanos(u64::MAX), None);
        assert_eq!(apple_silicon.ticks_to_nanos_saturating(u64::MAX), u64::MAX);
        // Exact inverse nanos -> ticks conversion.
        assert_eq!(apple_silicon.nanos_to_ticks(0), Some(0));
        assert_eq!(
            apple_silicon.nanos_to_ticks(1_000_000_000),
            Some(24_000_000)
        );
        assert_eq!(apple_silicon.nanos_to_ticks(1_000), Some(24));
        assert_eq!(
            apple_silicon.nanos_to_ticks_saturating(u64::MAX),
            apple_silicon.nanos_to_ticks(u64::MAX).unwrap()
        );

        let intel = MachTimebase::from_frequency_hz(1_000_000_000).unwrap();
        assert_eq!(intel, MachTimebase::NANOSECONDS);
        assert_eq!(intel.ticks_to_nanos(123_456_789), Some(123_456_789));

        let host = MachTimebase::host();
        assert!(host.numer() > 0 && host.denom() > 0);
    }

    #[test]
    fn monotonic_clock_never_regresses_on_zero_or_out_of_order_packets() {
        let tb = MachTimebase::APPLE_SILICON_24MHZ;
        let mut clock = MonotonicTimestampClock::default();
        assert_eq!(clock.advance(tb, 0), 0);
        assert_eq!(clock.advance(tb, 24_000), 1_000_000);
        // Zero ("immediate") packet stays monotonic at last_nanos.
        assert_eq!(clock.advance(tb, 0), 1_000_000);
        // Slightly older packet clamps to last_nanos.
        assert_eq!(clock.advance(tb, 12_000), 1_000_000);
        // Advancing packet moves forward accurately.
        assert_eq!(clock.advance(tb, 48_000), 2_000_000);
        assert_eq!(clock.last_nanos(), 2_000_000);
    }
}
