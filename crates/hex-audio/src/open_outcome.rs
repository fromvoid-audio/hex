//! Typed failure outcomes of a default-device output open.
//!
//! A worker or recovery open that validates the document rate refuses a
//! device whose rate differs, unless the backend converts it (below), as
//! `OutputOpenError::RateMismatch`, carrying both rates in hertz. This is a
//! recoverable state: nothing was exposed to a callback and the project is
//! unchanged, so the caller may retry after the device or project rate
//! changes. Every other refusal stays a lowercase diagnostic in `Failed`.
//!
//! The type does not own a stream, lease or worker; it is plain data that may
//! cross to the UI.
//!
//! # Output rate conversion (#4130)
//!
//! CoreAudio's output unit (AUHAL) converts its client stream format to the
//! device's nominal rate, so an output stream may run at the project rate on a
//! device at another rate. CPAL 0.17.3 builds output streams with exactly that
//! client format and does not change the device's nominal rate (it does so only
//! for input streams, where AUHAL cannot convert). Hex therefore requests the
//! project rate from an ordinary CoreAudio output and the operating system
//! resamples after the callback: the user's device rate is never changed, so
//! nothing needs restoring, and the callback contract and allocation behaviour
//! are unchanged. The conversion latency belongs to CoreAudio and is not
//! declared to hex, like the rest of the physical output latency.
//!
//! The macOS plugin bridge selects a separate hardware buffer and verifies the
//! initialized CoreAudio client slice against its transport quantum before
//! opening. Other backends keep the typed mismatch refusal.

use std::fmt;

/// The default output negotiated `device_hz`, but the open required the
/// project's `project_hz`. Both are sample rates in hertz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateMismatch {
    pub device_hz: u32,
    pub project_hz: u32,
}

impl fmt::Display for RateMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "default output rate is {} hz; project requires {} hz",
            self.device_hz, self.project_hz
        )
    }
}

/// Why an output open produced no output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputOpenError {
    /// Recoverable: the device runs at a different rate than the project.
    RateMismatch(RateMismatch),
    /// Any other refusal, as a lowercase diagnostic.
    Failed(String),
}

impl OutputOpenError {
    /// The typed rate mismatch, when this is one.
    pub fn rate_mismatch(&self) -> Option<RateMismatch> {
        match self {
            Self::RateMismatch(mismatch) => Some(*mismatch),
            Self::Failed(_) => None,
        }
    }
}

impl fmt::Display for OutputOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RateMismatch(mismatch) => mismatch.fmt(f),
            Self::Failed(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for OutputOpenError {}

impl From<RateMismatch> for OutputOpenError {
    fn from(mismatch: RateMismatch) -> Self {
        Self::RateMismatch(mismatch)
    }
}

impl From<String> for OutputOpenError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

impl From<&str> for OutputOpenError {
    fn from(error: &str) -> Self {
        Self::Failed(error.to_owned())
    }
}

impl From<OutputOpenError> for String {
    fn from(error: OutputOpenError) -> Self {
        error.to_string()
    }
}

/// Whether this build's output backend resamples a client stream to the
/// device rate (CoreAudio AUHAL on macOS).
pub(crate) const BACKEND_CONVERTS_OUTPUT_RATE: bool = cfg!(target_os = "macos");

/// Rate to request for an output stream: the device's own rate, or the
/// required project rate when `backend_converts` and it is a supported rate.
/// Otherwise a differing project rate is the typed recoverable mismatch.
pub(crate) fn negotiate_stream_rate(
    project_hz: Option<u32>,
    device_hz: u32,
    backend_converts: bool,
) -> Result<u32, RateMismatch> {
    match project_hz {
        Some(project_hz)
            if project_hz != device_hz
                && backend_converts
                && (8_000..=192_000).contains(&project_hz) =>
        {
            Ok(project_hz)
        }
        _ => validate_expected_rate(project_hz, device_hz).map(|()| device_hz),
    }
}

/// `Ok` when no rate was required or the device matches it.
pub(crate) fn validate_expected_rate(
    project_hz: Option<u32>,
    device_hz: u32,
) -> Result<(), RateMismatch> {
    match project_hz {
        Some(project_hz) if project_hz != device_hz => Err(RateMismatch {
            device_hz,
            project_hz,
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_differing_device_rate_is_a_typed_recoverable_mismatch() {
        let mismatch = RateMismatch {
            device_hz: 44_100,
            project_hz: 48_000,
        };
        assert_eq!(validate_expected_rate(Some(48_000), 44_100), Err(mismatch));
        assert_eq!(validate_expected_rate(Some(48_000), 48_000), Ok(()));
        assert_eq!(validate_expected_rate(None, 44_100), Ok(()));

        let error = OutputOpenError::from(mismatch);
        assert_eq!(error.rate_mismatch(), Some(mismatch));
        assert_eq!(
            error.to_string(),
            "default output rate is 44100 hz; project requires 48000 hz"
        );
        let failed = OutputOpenError::from("no default audio output device");
        assert_eq!(failed.rate_mismatch(), None);
        assert_eq!(String::from(failed), "no default audio output device");
    }

    #[test]
    fn a_converting_backend_runs_the_stream_at_the_project_rate() {
        // A 44.1 kHz project on a 48 kHz device streams at 44.1 kHz.
        assert_eq!(
            negotiate_stream_rate(Some(44_100), 48_000, true),
            Ok(44_100)
        );
        assert_eq!(
            negotiate_stream_rate(Some(48_000), 48_000, true),
            Ok(48_000)
        );
        // Without a required rate the device default is kept.
        assert_eq!(negotiate_stream_rate(None, 48_000, true), Ok(48_000));
        // A backend without conversion (or a fixed-quantum output) refuses.
        assert_eq!(
            negotiate_stream_rate(Some(44_100), 48_000, false),
            Err(RateMismatch {
                device_hz: 48_000,
                project_hz: 44_100,
            })
        );
        // An unsupported project rate is never requested from the backend.
        for project_hz in [7_999, 192_001] {
            assert_eq!(
                negotiate_stream_rate(Some(project_hz), 48_000, true),
                Err(RateMismatch {
                    device_hz: 48_000,
                    project_hz,
                })
            );
        }
    }
}
