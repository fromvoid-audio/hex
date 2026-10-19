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

/// True only for a single comparison operation (or its exact history inverse).
/// Strip no audible field: active provider bytes, assets, host metadata, bypass,
/// routing, MIDI, tuning, IDs and every other Project field must stay identical.
pub(crate) fn metadata_pair(
    source: &Project,
    target: &Project,
) -> Result<bool, crate::SessionError> {
    validate_project(source)?;
    validate_project(target)?;
    let mut left = source.snapshot();
    let mut right = target.snapshot();
    let devices = |snapshot: &mut hex_project::Snapshot| {
        snapshot
            .tracks
            .iter_mut()
            .flat_map(|track| track.instrument.iter_mut().chain(track.devices.iter_mut()))
            .chain(
                snapshot
                    .return_tracks
                    .iter_mut()
                    .flat_map(|track| track.devices.iter_mut()),
            )
            .for_each(|device| device.comparison = None);
    };
    devices(&mut left);
    devices(&mut right);
    if left != right || source == target {
        return Ok(false);
    }
    // Prove a real typed Switch/Copy edge, not arbitrary metadata equivalence.
    for (from, to) in [(source, target), (target, source)] {
        for device in from
            .tracks()
            .iter()
            .flat_map(|track| track.instrument().into_iter().chain(track.devices().iter()))
            .chain(
                from.return_tracks()
                    .iter()
                    .flat_map(|track| track.devices().iter()),
            )
        {
            if to
                .device(device.id())
                .is_none_or(|other| other.comparison() == device.comparison())
            {
                continue;
            }
            for action in [
                hex_project::DeviceComparisonAction::Switch,
                hex_project::DeviceComparisonAction::CopyToInactive,
            ] {
                let mut trial = hex_project::Editor::new(from.clone(), 1)?;
                trial.execute(hex_project::Command::CompareDevice {
                    device: device.id(),
                    expected: device.active_variant(),
                    action,
                })?;
                if trial.project() == to {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}
