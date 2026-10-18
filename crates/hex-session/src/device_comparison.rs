//! Control-side provider validation for both authored comparison slots.
//! No DSP instance, media admission, runtime owner or callback work is created.
use hex_engine::{Invalid, NativeDeviceClass};
use hex_lattice_contract::NativeDeviceState;
use hex_project::{DeviceComparison, Project};

pub(crate) fn validate_states(
    class: NativeDeviceClass,
    state: &NativeDeviceState,
    comparison: Option<&DeviceComparison>,
) -> Result<(), Invalid> {
    for authored in std::iter::once(state).chain(comparison.map(DeviceComparison::inactive)) {
        hex_engine::native_device_view_for_class(class, authored)?;
        if authored.type_id() != state.type_id() {
            return Err(Invalid(
                "device comparison provider does not match active state",
            ));
        }
    }
    Ok(())
}

/// Existing Engine compilation validates active states. A comparison adds one
/// inactive provider input which must also refuse malformed payloads before the
/// candidate can enter canonical state/history or a newly opened Session.
pub(crate) fn validate_project(project: &Project) -> Result<(), Invalid> {
    for track in project.tracks() {
        if let Some(device) = track
            .instrument()
            .filter(|device| device.comparison().is_some())
        {
            validate_states(
                NativeDeviceClass::Instrument,
                device.state(),
                device.comparison(),
            )?;
        }
        for device in track
            .devices()
            .iter()
            .filter(|device| device.comparison().is_some())
        {
            validate_states(
                NativeDeviceClass::Effect,
                device.state(),
                device.comparison(),
            )?;
        }
    }
    for device in project
        .return_tracks()
        .iter()
        .flat_map(|track| track.devices())
        .filter(|device| device.comparison().is_some())
    {
        validate_states(
            NativeDeviceClass::Effect,
            device.state(),
            device.comparison(),
        )?;
    }
    Ok(())
}
