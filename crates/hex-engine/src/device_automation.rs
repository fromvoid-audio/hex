//! Admission for canonical device envelopes. The neutral project/storage model
//! retains authored curves independently of installed providers. Until native
//! and foreign runtime delivery consumes this contract, refuse every nonempty
//! envelope before preparation rather than silently rendering its base value.
//! Existing track-gain and device-internal EQ envelopes retain their owners.
use crate::Invalid;

pub const DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED: &str = "device parameter automation playback is not prepared; use a compatible build or clear the envelope in a copy";

pub(crate) fn validate_project(project: &hex_project::Project) -> Result<(), Invalid> {
    if project.has_device_automation() {
        return Err(Invalid(DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED));
    }
    Ok(())
}
