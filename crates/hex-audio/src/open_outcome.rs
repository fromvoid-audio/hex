//! Typed failure outcomes of a default-device output open.
//!
//! A worker or recovery open that validates the document rate refuses a
//! device whose negotiated rate differs as `OutputOpenError::RateMismatch`,
//! carrying both rates in hertz. This is a recoverable state: nothing was
//! exposed to a callback and the project is unchanged, so the caller may
//! retry after the device or project rate changes. Every other refusal stays
//! a lowercase diagnostic in `Failed`.
//!
//! The type does not own a stream, lease or worker; it is plain data that may
//! cross to the UI.

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
}
