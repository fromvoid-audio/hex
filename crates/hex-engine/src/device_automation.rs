//! Canonical device-envelope admission and control-side native program preparation.
//! Utility effect controls are consumed by the existing closed processor family.
//! Other providers, source-owned instruments/effects and reserved host trims
//! refuse explicitly until their actual runtime/target contracts are implemented.
use crate::{Invalid, NativeDeviceClass};
use hex_lattice_contract::{NativeDeviceState, ParameterId};
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
    for device in project.automation_devices() {
        if device
            .automation()
            .iter()
            .any(|lane| !supported(registry, device.state(), lane.parameter))
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
    hex_lattice::PreparedParameterAutomation::quoted_storage_bytes(
        device.automation().len(),
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
        .try_reserve_exact(device.automation().len())
        .map_err(|_| Invalid("could not reserve device automation lanes"))?;
    let mut points = Vec::new();
    points
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve device automation points"))?;
    for lane in device.automation() {
        if !supported(registry, device.state(), lane.parameter) {
            return Err(Invalid(DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED));
        }
        let descriptors =
            hex_lattice_contract::validated_state_descriptors(definition, device.state())
                .map_err(|_| Invalid("automation parameter metadata is invalid"))?;
        let descriptor = *descriptors
            .iter()
            .find(|d| d.id == lane.parameter)
            .ok_or(Invalid("automation parameter no longer exists"))?;
        let semantics = definition
            .parameter_semantics_for_state(device.state(), lane.parameter)
            .map_err(|_| Invalid("automation parameter semantics are invalid"))?
            .ok_or(Invalid("automation parameter semantics are undeclared"))?;
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
        lanes.push(hex_lattice::ParameterAutomationLane {
            descriptor,
            semantics,
            points: start..points.len(),
        });
    }
    hex_lattice::PreparedParameterAutomation::new(lanes, points)
        .map_err(|_| Invalid("device automation program is invalid or exceeds its budget"))
}

#[cfg(test)]
#[path = "device_automation_result_tests.rs"]
mod result_tests;
