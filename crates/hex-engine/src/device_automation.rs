//! Canonical device-envelope admission and control-side native program preparation.
//! Utility effect controls are consumed by the existing closed processor family.
//! Host trims are consumed at native effect boundaries (track/main/return).
//! Foreign processors and source-owned instruments/effects refuse host targets;
//! other provider parameter curves still require explicit runtime support.
use crate::{Invalid, NativeDeviceClass};
use hex_lattice::{NativeTransientValue, ProcessorChainOwner, ProcessorInstanceIdentity};
use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId, NativeDeviceState, ParameterId};
pub const DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED: &str = "device parameter automation playback is not prepared; use a compatible build or clear the envelope in a copy";

/// Canonical control-side arm description for one supported native effect.
/// This Copy identity grants neither publication nor a runtime/source lease.
/// Runtime factory/twin admission and source fencing remain with preparation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeParameterInputTarget {
    device: hex_project::DeviceId,
    target: DeviceAutomationTarget,
    identity: ProcessorInstanceIdentity,
    device_type: DeviceTypeId,
}
impl NativeParameterInputTarget {
    pub fn device(self) -> hex_project::DeviceId {
        self.device
    }
    pub fn target(self) -> DeviceAutomationTarget {
        self.target
    }
    pub fn identity(self) -> ProcessorInstanceIdentity {
        self.identity
    }
    pub fn device_type(self) -> DeviceTypeId {
        self.device_type
    }
    /// Validate canonical units against the actual current state/ownership. This
    /// performs no authored edit, parameter patch, normalization or DSP setup.
    /// The audio owner must separately fence the resulting value to its source.
    pub fn validate_set(
        self,
        project: &hex_project::Project,
        value: NativeTransientValue,
    ) -> Result<NativeTransientValue, Invalid> {
        if value.target() != self.target {
            return Err(Invalid(
                "native parameter input target does not match its value",
            ));
        }
        let current = native_parameter_input_target(project, self.device, self.target)?;
        if current != self {
            return Err(Invalid(
                "native parameter input owner or device type changed",
            ));
        }
        let device = project
            .device(self.device)
            .ok_or(Invalid("native parameter input device no longer exists"))?;
        let registry = hex_devices::native_registry()
            .map_err(|_| Invalid("native effect registry is invalid"))?;
        if let NativeTransientValue::Parameter { id, value } = value {
            let definition = registry
                .definition(self.device_type)
                .ok_or(Invalid("native parameter input device is unsupported"))?;
            hex_lattice_contract::validate_parameter_write(definition, device.state(), id, value)
                .map_err(|_| {
                Invalid("native parameter input value is invalid for the current state")
            })?;
        }
        Ok(value)
    }
}
/// Resolve exactly one canonical Track/Main/Return native effect. Source-owned
/// devices and foreign providers are excluded even when they expose metadata.
/// Current provider capability is explicit; controls alone never grant it.
pub fn native_parameter_input_target(
    project: &hex_project::Project,
    device: hex_project::DeviceId,
    target: DeviceAutomationTarget,
) -> Result<NativeParameterInputTarget, Invalid> {
    if project
        .native_sources()
        .iter()
        .any(|source| source.devices().any(|candidate| candidate.id() == device))
    {
        return Err(Invalid(
            "source-owned native parameter input is unsupported",
        ));
    }
    let mut matching =
        effect_device_owners(project).filter(|(candidate, _)| candidate.id() == device);
    let (current, owner) = matching
        .next()
        .ok_or(Invalid("native parameter input effect no longer exists"))?;
    if matching.next().is_some() {
        return Err(Invalid(
            "native parameter input effect identity is not unique",
        ));
    }
    let registry =
        hex_devices::native_registry().map_err(|_| Invalid("native effect registry is invalid"))?;
    if !target_supported(&registry, current.state(), target) {
        return Err(Invalid("native parameter input target is unsupported"));
    }
    Ok(NativeParameterInputTarget {
        device,
        target,
        identity: ProcessorInstanceIdentity::new(owner, device.value())
            .map_err(|_| Invalid("native parameter input identity is invalid"))?,
        device_type: current.state().type_id(),
    })
}
fn effect_device_owners(
    project: &hex_project::Project,
) -> impl Iterator<Item = (&hex_project::Device, ProcessorChainOwner)> {
    project
        .tracks()
        .iter()
        .flat_map(|track| {
            track
                .devices()
                .iter()
                .map(move |device| (device, ProcessorChainOwner::Track(track.id().value())))
        })
        .chain(project.return_tracks().iter().flat_map(|track| {
            track
                .devices()
                .iter()
                .map(move |device| (device, ProcessorChainOwner::Return(track.id().value())))
        }))
        .chain(
            project
                .main_devices()
                .iter()
                .map(|device| (device, ProcessorChainOwner::Main)),
        )
}

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
    quote_with_input(device, None)
}
/// Persistent flat backing after arm preparation. Only a missing provider
/// reading slot adds an ID and opaque metadata lane; host input adds neither.
pub(crate) fn quote_with_input(
    device: &hex_project::Device,
    input: Option<NativeParameterInputTarget>,
) -> Result<usize, crate::AudioStorageError> {
    let (provider, host, points) = reading_counts(device)?;
    let provider = provider
        .checked_add(usize::from(input_needs_provider_fallback(device, input)?))
        .ok_or(crate::AudioStorageError::Overflow)?;
    hex_lattice::PreparedParameterAutomation::quoted_storage_bytes_with_host(provider, host, points)
        .ok_or(crate::AudioStorageError::Overflow)
}
/// Additional temporary old-vector backing during insertion of a provider
/// fallback. Normal envelope construction already accounts raw metadata/points;
/// this charge covers the old compiled IDs/lanes during arm-time reallocation.
/// The normalized/compiled point vectors are not duplicated by insertion.
pub(crate) fn input_construction_growth_overlap(
    device: &hex_project::Device,
    input: Option<NativeParameterInputTarget>,
) -> Result<usize, crate::AudioStorageError> {
    if !input_needs_provider_fallback(device, input)? {
        return Ok(0);
    }
    let (provider, host, _) = reading_counts(device)?;
    hex_lattice::PreparedParameterAutomation::quoted_storage_bytes_with_host(provider, host, 0)
        .ok_or(crate::AudioStorageError::Overflow)
}
fn input_needs_provider_fallback(
    device: &hex_project::Device,
    input: Option<NativeParameterInputTarget>,
) -> Result<bool, crate::AudioStorageError> {
    let Some(input) = input.filter(|input| input.device == device.id()) else {
        return Ok(false);
    };
    if input.device_type != device.state().type_id() {
        return Err(crate::AudioStorageError::SourceMismatch);
    }
    Ok(input.target.provider_parameter().is_some()
        && !device
            .reading_automation()
            .any(|lane| lane.target == input.target))
}
fn reading_counts(
    device: &hex_project::Device,
) -> Result<(usize, usize, usize), crate::AudioStorageError> {
    let points = device
        .reading_automation()
        .try_fold(0usize, |sum, lane| sum.checked_add(lane.points.len()))
        .ok_or(crate::AudioStorageError::Overflow)?;
    let provider = device
        .reading_automation()
        .filter(|lane| !lane.target.is_host())
        .count();
    Ok((
        provider,
        device.reading_automation().count() - provider,
        points,
    ))
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
    prepare_with_registry_and_input(device, registry, None)
}
/// Prepare reading envelopes first, then arm only the matching canonical
/// device. Provider fallback insertion adds one ID/opaque lane and zero points
/// when no reading lane exists. Budget callers must charge that slot plus the
/// old ID/metadata growth overlap returned before arm_provider_input; host-only
/// arming adds inline target metadata and never invents a provider runtime.
pub(crate) fn prepare_with_registry_and_input(
    device: &hex_project::Device,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    input: Option<NativeParameterInputTarget>,
) -> Result<hex_lattice::PreparedParameterAutomation, Invalid> {
    let mut program = prepare_envelopes_with_registry(device, registry)?;
    if let Some(input) = input.filter(|input| input.device == device.id()) {
        if input.device_type != device.state().type_id()
            || !target_supported(registry, device.state(), input.target)
        {
            return Err(Invalid(
                "native parameter input target changed or is unsupported",
            ));
        }
        match input.target {
            DeviceAutomationTarget::Parameter(parameter) => {
                let definition = registry
                    .definition(input.device_type)
                    .ok_or(Invalid("native parameter input device is unsupported"))?;
                let descriptor =
                    hex_lattice_contract::validated_state_descriptors(definition, device.state())
                        .map_err(|_| Invalid("native parameter input metadata is invalid"))?
                        .into_iter()
                        .find(|descriptor| descriptor.id == parameter)
                        .ok_or(Invalid("native parameter input parameter no longer exists"))?;
                let semantics = hex_lattice_contract::validated_parameter_semantics(
                    definition,
                    device.state(),
                    parameter,
                )
                .map_err(|_| Invalid("native parameter input semantics are invalid"))?
                .ok_or(Invalid("native parameter input semantics are undeclared"))?;
                program
                    .arm_provider_input(descriptor, semantics)
                    .map_err(|_| {
                        Invalid("native parameter input program is invalid or exceeds its budget")
                    })?;
            }
            target => program
                .arm_host_gain_input(target)
                .map_err(|_| Invalid("native host gain input target is invalid"))?,
        }
    }
    Ok(program)
}
fn prepare_envelopes_with_registry(
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
        .reading_automation()
        .try_fold(0usize, |sum, lane| sum.checked_add(lane.points.len()))
        .ok_or(Invalid("device automation point count overflow"))?;
    let mut lanes = Vec::new();
    lanes
        .try_reserve_exact(
            device
                .reading_automation()
                .filter(|lane| !lane.target.is_host())
                .count(),
        )
        .map_err(|_| Invalid("could not reserve device automation lanes"))?;
    let mut host = Vec::new();
    host.try_reserve_exact(
        device
            .reading_automation()
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
        if !lane.is_reading() {
            continue;
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

#[cfg(test)]
#[path = "device_automation_result_tests.rs"]
mod result_tests;

#[cfg(test)]
#[path = "device_parameter_input_tests.rs"]
mod input_tests;
