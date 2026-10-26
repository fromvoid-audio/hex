//! Canonical device-envelope admission and control-side native program preparation.
//! Utility effect controls are consumed by the existing closed processor family.
//! Host trims are consumed at native effect boundaries (track/main/return).
//! Foreign processors and source-owned instruments/effects refuse host targets;
//! other provider parameter curves still require explicit runtime support.
use crate::{Invalid, NativeDeviceClass};
use hex_lattice_contract::{DeviceAutomationTarget, NativeDeviceState, ParameterId};
pub const DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED: &str = "device parameter automation playback is not prepared; use a compatible build or clear the envelope in a copy";

/// Cheap control query for a real current parameter and implemented runtime.
/// No DSP/runtime construction, authored mutation or capability inference.
pub fn native_parameter_automation_supported(
    class: NativeDeviceClass,
    state: &NativeDeviceState,
    parameter: ParameterId,
) -> bool {
    if class != NativeDeviceClass::Effect {
        return false;
    }
    let Ok(registry) = hex_devices::native_registry() else {
        return false;
    };
    supported(&registry, state, parameter)
}
/// Provider/class metadata capability, without a project ownership decision.
/// UI assignment uses project_device_automation_target_supported instead:
/// source-owned effects can share this metadata but lack render delivery.
pub fn native_device_automation_target_supported(
    class: NativeDeviceClass,
    state: &NativeDeviceState,
    target: DeviceAutomationTarget,
) -> bool {
    if class != NativeDeviceClass::Effect {
        return false;
    }
    let Ok(registry) = hex_devices::native_registry() else {
        return false;
    };
    target_supported(&registry, state, target)
}
/// Actual-owner control query. Only track/main/return effect boundaries consume
/// these curves. A retained source (including a linked/shared instrument or an
/// effect in its chain), missing identity, or foreign provider returns false.
pub fn project_device_automation_target_supported(
    project: &hex_project::Project,
    device: hex_project::DeviceId,
    target: DeviceAutomationTarget,
) -> bool {
    let Some(device) = effect_devices(project).find(|candidate| candidate.id() == device) else {
        return false;
    };
    let Ok(registry) = hex_devices::native_registry() else {
        return false;
    };
    target_supported(&registry, device.state(), target)
}
fn effect_devices(project: &hex_project::Project) -> impl Iterator<Item = &hex_project::Device> {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.devices())
        .chain(
            project
                .return_tracks()
                .iter()
                .flat_map(|track| track.devices()),
        )
        .chain(project.main_devices())
}
fn target_supported(
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    state: &NativeDeviceState,
    target: DeviceAutomationTarget,
) -> bool {
    match target {
        DeviceAutomationTarget::Parameter(id) => supported(registry, state, id),
        DeviceAutomationTarget::HostInputGain | DeviceAutomationTarget::HostOutputGain => {
            // Registry presence alone must not grant foreign providers a host
            // law that would bypass their aligned dry-input history.
            let Ok(native) = hex_devices::native_registry() else {
                return false;
            };
            native.definition(state.type_id()).is_some() && registry.migrate_state(state).is_ok()
        }
    }
}
fn supported(
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    state: &NativeDeviceState,
    parameter: ParameterId,
) -> bool {
    let Some(definition) = registry.definition(state.type_id()) else {
        return false;
    };
    definition
        .supports_parameter_automation(state, parameter)
        .unwrap_or(false)
        && hex_lattice_contract::validated_parameter_semantics(definition, state, parameter)
            .is_ok_and(|semantics| {
                semantics.is_some_and(|s| {
                    s.access == hex_lattice_contract::ParameterAccess::ReadWrite
                        && s.automation
                            == hex_lattice_contract::ParameterAutomation::DeviceArrangementEnvelope
                })
            })
}
pub(crate) fn validate_project(project: &hex_project::Project) -> Result<(), Invalid> {
    let registry =
        hex_devices::native_registry().map_err(|_| Invalid("native effect registry is invalid"))?;
    validate_project_with_registry(project, &registry)
}
pub(crate) fn validate_project_with_registry(
    project: &hex_project::Project,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
) -> Result<(), Invalid> {
    if project.native_sources().iter().any(|source| {
        source
            .devices()
            .any(|device| !device.automation().is_empty())
    }) {
        return Err(Invalid(DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED));
    }
    for device in effect_devices(project) {
        if device
            .automation()
            .iter()
            .any(|lane| !target_supported(registry, device.state(), lane.target))
        {
            return Err(Invalid(DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED));
        }
    }
    Ok(())
}
pub(crate) fn quote(device: &hex_project::Device) -> Result<usize, crate::AudioStorageError> {
    let points = device
        .automation()
        .iter()
        .try_fold(0usize, |sum, lane| sum.checked_add(lane.points.len()))
        .ok_or(crate::AudioStorageError::Overflow)?;
    let provider = device
        .automation()
        .iter()
        .filter(|lane| !lane.target.is_host())
        .count();
    hex_lattice::PreparedParameterAutomation::quoted_storage_bytes_with_host(
        provider,
        device.automation().len() - provider,
        points,
    )
    .ok_or(crate::AudioStorageError::Overflow)
}
pub(crate) fn prepare(
    device: &hex_project::Device,
) -> Result<hex_lattice::PreparedParameterAutomation, Invalid> {
    let registry =
        hex_devices::native_registry().map_err(|_| Invalid("native effect registry is invalid"))?;
    prepare_with_registry(device, &registry)
}
pub(crate) fn prepare_with_registry(
    device: &hex_project::Device,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
) -> Result<hex_lattice::PreparedParameterAutomation, Invalid> {
    if device.automation().is_empty() {
        return Ok(Default::default());
    }
    let definition = registry
        .definition(device.state().type_id())
        .ok_or(Invalid(DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED))?;
    let count = device
        .automation()
        .iter()
        .try_fold(0usize, |sum, lane| sum.checked_add(lane.points.len()))
        .ok_or(Invalid("device automation point count overflow"))?;
    let mut lanes = Vec::new();
    lanes
        .try_reserve_exact(
            device
                .automation()
                .iter()
                .filter(|lane| !lane.target.is_host())
                .count(),
        )
        .map_err(|_| Invalid("could not reserve device automation lanes"))?;
    let mut host = Vec::new();
    host.try_reserve_exact(
        device
            .automation()
            .iter()
            .filter(|lane| lane.target.is_host())
            .count(),
    )
    .map_err(|_| Invalid("could not reserve host automation lanes"))?;
    let mut points = Vec::new();
    points
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve device automation points"))?;
    for lane in device.automation() {
        if !target_supported(registry, device.state(), lane.target) {
            return Err(Invalid(DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED));
        }
        let start = points.len();
        points.extend(
            lane.points
                .iter()
                .map(|point| hex_lattice::ParameterAutomationPoint {
                    beat_raw: point.beat.raw(),
                    normalized: point.value.get(),
                    step: point.outgoing == hex_project::AutomationInterpolation::Step,
                }),
        );
        if let Some(parameter) = lane.target.provider_parameter() {
            let descriptors =
                hex_lattice_contract::validated_state_descriptors(definition, device.state())
                    .map_err(|_| Invalid("automation parameter metadata is invalid"))?;
            let descriptor = *descriptors
                .iter()
                .find(|d| d.id == parameter)
                .ok_or(Invalid("automation parameter no longer exists"))?;
            let semantics = definition
                .parameter_semantics_for_state(device.state(), parameter)
                .map_err(|_| Invalid("automation parameter semantics are invalid"))?
                .ok_or(Invalid("automation parameter semantics are undeclared"))?;
            lanes.push(hex_lattice::ParameterAutomationLane {
                descriptor,
                semantics,
                points: start..points.len(),
            });
        } else {
            host.push(hex_lattice::HostAutomationLane {
                target: lane.target,
                points: start..points.len(),
            });
        }
    }
    hex_lattice::PreparedParameterAutomation::new_with_host(lanes, host, points)
        .map_err(|_| Invalid("device automation program is invalid or exceeds its budget"))
}
