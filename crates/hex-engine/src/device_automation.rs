//! Canonical device-envelope admission and control-side native program preparation.
//! Retained native/host targets keep their preparation eligibility when reading
//! base values; only authored/touch readings change, not the target union.
//! Utility effect controls are consumed by the existing closed processor family.
//! Host trims are consumed at native effect boundaries (track/main/return).
//! Foreign processors and source-owned instruments/effects refuse host targets;
//! other provider parameter curves still require explicit runtime support.
use crate::{Invalid, NativeDeviceClass};
use hex_lattice::{NativeTransientValue, ProcessorChainOwner, ProcessorInstanceIdentity};
use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId, NativeDeviceState, ParameterId};
pub const DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED: &str = "device parameter automation playback is not prepared; use a compatible build or clear the envelope in a copy";
/// Maximum recorded targets admitted into one prepared binding union.
pub(crate) const MAX_EXTERNAL_PARAMETER_TARGETS: usize = 256;

/// Canonical control-side arm description for one supported native processor.
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
        if let NativeTransientValue::Parameter { id, value } = value {
            let checked = if matches!(self.identity.owner(), ProcessorChainOwner::Source(_)) {
                let registry = hex_devices::native_instrument_registry()
                    .map_err(|_| Invalid("native instrument registry is invalid"))?;
                let definition = registry
                    .definition(self.device_type)
                    .ok_or(Invalid("native parameter input instrument is unsupported"))?;
                hex_lattice_contract::validate_parameter_write(
                    definition,
                    device.state(),
                    id,
                    value,
                )
            } else {
                let registry = hex_devices::native_registry()
                    .map_err(|_| Invalid("native effect registry is invalid"))?;
                let definition = registry
                    .definition(self.device_type)
                    .ok_or(Invalid("native parameter input device is unsupported"))?;
                hex_lattice_contract::validate_parameter_write(
                    definition,
                    device.state(),
                    id,
                    value,
                )
            };
            checked.map_err(|_| {
                Invalid("native parameter input value is invalid for the current state")
            })?;
        }
        Ok(value)
    }
}
/// Resolve one canonical Track/Main/Return native effect or one supported
/// receiving instrument. Foreign and source effect controls are excluded.
/// Current provider capability is explicit; controls alone never grant it.
pub fn native_parameter_input_target(
    project: &hex_project::Project,
    device: hex_project::DeviceId,
    target: DeviceAutomationTarget,
) -> Result<NativeParameterInputTarget, Invalid> {
    if let Some(source) = project
        .native_sources()
        .iter()
        .find(|source| source.devices().any(|candidate| candidate.id() == device))
    {
        let instrument = source.instrument();
        if instrument.id() != device
            || instrument.bypassed()
            || source.receiver().is_none()
            || source.devices().count() != 1
            || !instrument_target_supported(instrument.state(), target)
        {
            return Err(Invalid(
                "receiving instrument parameter input is unsupported",
            ));
        }
        return Ok(NativeParameterInputTarget {
            device,
            target,
            identity: ProcessorInstanceIdentity::new(
                ProcessorChainOwner::Source(source.id().value()),
                device.value(),
            )
            .map_err(|_| Invalid("native instrument input identity is invalid"))?,
            device_type: instrument.state().type_id(),
        });
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
    native_device_automation_target_supported(
        class,
        state,
        DeviceAutomationTarget::Parameter(parameter),
    )
}
/// Provider/class metadata capability, without a project ownership decision.
/// UI assignment uses project_device_automation_target_supported instead:
/// source-owned effects can share this metadata but lack render delivery.
pub fn native_device_automation_target_supported(
    class: NativeDeviceClass,
    state: &NativeDeviceState,
    target: DeviceAutomationTarget,
) -> bool {
    if class == NativeDeviceClass::Instrument {
        return instrument_target_supported(state, target);
    }
    if class != NativeDeviceClass::Effect {
        return false;
    }
    let Ok(registry) = hex_devices::native_registry() else {
        return false;
    };
    target_supported(&registry, state, target)
}
/// Actual-owner control query for track/main/return effects and the four FM4
/// carrier streams at a single-device receiving source. Contributors share that
/// one source program. Unrouted sources, source effects and host trims refuse.
/// This metadata query grants no runtime/resource custody; preparation validates
/// the actual factory grants and bounded program storage before publication.
pub fn project_device_automation_target_supported(
    project: &hex_project::Project,
    device: hex_project::DeviceId,
    target: DeviceAutomationTarget,
) -> bool {
    if let Some(source) = project
        .native_sources()
        .iter()
        .find(|source| source.instrument().id() == device)
    {
        return source.receiver().is_some()
            && source.devices().count() == 1
            && instrument_target_supported(source.instrument().state(), target);
    }
    let Some(device) = effect_devices(project).find(|candidate| candidate.id() == device) else {
        return false;
    };
    let Ok(registry) = hex_devices::native_registry() else {
        return false;
    };
    target_supported(&registry, device.state(), target)
}
/// Canonical track-local, Return and Main effects, excluding retained sources.
pub(crate) fn effect_devices(
    project: &hex_project::Project,
) -> impl Iterator<Item = &hex_project::Device> {
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
        DeviceAutomationTarget::Vst3 { .. } => false,
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
fn instrument_target_supported(state: &NativeDeviceState, target: DeviceAutomationTarget) -> bool {
    let DeviceAutomationTarget::Parameter(id) = target else {
        return false;
    };
    if state.type_id() != hex_devices::fm4::device::FM4_TYPE_ID
        || !(29..=32).any(|value| id == ParameterId::from_static(value))
    {
        return false;
    }
    let Ok(registry) = hex_devices::native_instrument_registry() else {
        return false;
    };
    let Some(definition) = registry.definition(state.type_id()) else {
        return false;
    };
    hex_lattice_contract::validated_parameter_semantics(definition, state, id).is_ok_and(|value| {
        value.is_some_and(|semantics| {
            semantics.access == hex_lattice_contract::ParameterAccess::ReadWrite
                && semantics.automation
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
    crate::device_modulation::validate_source_owners(project)?;
    if project.native_sources().iter().any(|source| {
        source.devices().any(|device| {
            device.automation().iter().any(|lane| {
                device.id() != source.instrument().id()
                    || !(crate::compile::is_vst3_instrument(device)
                        && matches!(lane.target, DeviceAutomationTarget::Vst3 { .. })
                        || source.devices().count() == 1
                            && instrument_target_supported(device.state(), lane.target))
            })
        })
    }) {
        return Err(Invalid(DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED));
    }
    for device in effect_devices(project) {
        crate::device_modulation::EffectModulation::validate(device, project)?;
        if device.automation().iter().any(|lane| {
            // The Arrangement compiler verifies exact foreign capabilities
            // for both effect chains and instrument sources.
            !matches!(lane.target, DeviceAutomationTarget::Vst3 { .. })
                && !target_supported(registry, device.state(), lane.target)
        }) {
            return Err(Invalid(DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED));
        }
    }
    Ok(())
}
pub(crate) fn quote(device: &hex_project::Device) -> Result<usize, crate::AudioStorageError> {
    quote_with_input(device, None)
}
/// Persistent flat backing for retained target eligibility and optional arming.
/// Unread targets keep zero-point base-fallback metadata; a missing live provider
/// adds one ID/metadata slot. A host input arm adds neither.
pub(crate) fn quote_with_input(
    device: &hex_project::Device,
    input: Option<NativeParameterInputTarget>,
) -> Result<usize, crate::AudioStorageError> {
    quote_with_inputs_and_external(device, input, &[])
}
pub(crate) fn quote_with_inputs_and_external(
    device: &hex_project::Device,
    input: Option<NativeParameterInputTarget>,
    external: &[NativeParameterInputTarget],
) -> Result<usize, crate::AudioStorageError> {
    let (provider, host, points, _) = counts_with_external(device, input, external)?;
    hex_lattice::PreparedParameterAutomation::quoted_storage_bytes_with_host(provider, host, points)
        .and_then(|bytes| {
            bytes.checked_add(
                hex_lattice::PreparedParameterAutomation::quoted_touch_storage_bytes(
                    reading_touch_count(device),
                )?,
            )
        })
        .ok_or(crate::AudioStorageError::Overflow)
}
/// Additional temporary old-vector backing while unread targets, live input or
/// external targets add fallback slots. Points/touch buffers are not duplicated;
/// the final union bounds old ID/metadata vectors during sequential insertion.
/// The normalized/compiled point vectors are not duplicated by insertion.
pub(crate) fn input_construction_growth_overlap(
    device: &hex_project::Device,
    input: Option<NativeParameterInputTarget>,
) -> Result<usize, crate::AudioStorageError> {
    input_and_external_growth_overlap(device, input, &[])
}
pub(crate) fn input_and_external_growth_overlap(
    device: &hex_project::Device,
    input: Option<NativeParameterInputTarget>,
    external: &[NativeParameterInputTarget],
) -> Result<usize, crate::AudioStorageError> {
    let (provider, host, _, added_external) = counts_with_external(device, input, external)?;
    if added_external
        || device.automation().iter().any(|lane| {
            !lane.is_reading() && !matches!(lane.target, DeviceAutomationTarget::Vst3 { .. })
        })
    {
        // Any previous vector during sequential insertion is no larger than
        // the final union. This bound covers its overlap with the new backing.
        return hex_lattice::PreparedParameterAutomation::quoted_storage_bytes_with_host(
            provider, host, 0,
        )
        .ok_or(crate::AudioStorageError::Overflow);
    }
    if !input_needs_provider_fallback(device, input)? {
        return Ok(0);
    }
    let (provider, host, _) = prepared_counts(device)?;
    hex_lattice::PreparedParameterAutomation::quoted_storage_bytes_with_host(provider, host, 0)
        .ok_or(crate::AudioStorageError::Overflow)
}

fn counts_with_external(
    device: &hex_project::Device,
    input: Option<NativeParameterInputTarget>,
    external: &[NativeParameterInputTarget],
) -> Result<(usize, usize, usize, bool), crate::AudioStorageError> {
    if external.len() > MAX_EXTERNAL_PARAMETER_TARGETS {
        return Err(crate::AudioStorageError::Capacity);
    }
    let (mut provider, mut host, points) = prepared_counts(device)?;
    let live_fallback = input_needs_provider_fallback(device, input)?;
    provider = provider
        .checked_add(usize::from(live_fallback))
        .ok_or(crate::AudioStorageError::Overflow)?;
    let mut added_external = false;
    for (index, target) in external
        .iter()
        .enumerate()
        .filter(|(_, target)| target.device == device.id())
    {
        if target.device_type != device.state().type_id()
            || external[..index]
                .iter()
                .any(|old| old.device == target.device && old.target == target.target)
        {
            return Err(crate::AudioStorageError::SourceMismatch);
        }
        if device
            .automation()
            .iter()
            .any(|lane| lane.target == target.target)
            || (live_fallback
                && input.is_some_and(|live| {
                    live.device == target.device && live.target == target.target
                }))
        {
            continue;
        }
        if target.target.provider_parameter().is_some() {
            provider = provider
                .checked_add(1)
                .ok_or(crate::AudioStorageError::Overflow)?;
        } else {
            host = host
                .checked_add(1)
                .ok_or(crate::AudioStorageError::Overflow)?;
        }
        added_external = true;
    }
    Ok((provider, host, points, added_external))
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
            .automation()
            .iter()
            .any(|lane| lane.target == input.target))
}
fn reading_touch_count(device: &hex_project::Device) -> usize {
    device
        .reading_automation()
        .map(|lane| lane.touch.len())
        .sum()
}

fn prepared_counts(
    device: &hex_project::Device,
) -> Result<(usize, usize, usize), crate::AudioStorageError> {
    let points = device
        .reading_automation()
        .try_fold(0usize, |sum, lane| sum.checked_add(lane.point_count()))
        .ok_or(crate::AudioStorageError::Overflow)?;
    // Foreign programs retain their separate preflight/representation. Native
    // and host metadata remains eligible even while its authored data is muted.
    let eligible = || {
        device.automation().iter().filter(|lane| {
            lane.is_reading() || !matches!(lane.target, DeviceAutomationTarget::Vst3 { .. })
        })
    };
    let provider = eligible().filter(|lane| !lane.target.is_host()).count();
    Ok((provider, eligible().count() - provider, points))
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
    prepare_with_registry_inputs_and_external(device, registry, input, &[])
}

/// Compile the factory's retained target union with reading data and one separate
/// live-input arm. External targets only add reading slots; they do not arm a
/// gesture, add authored points or own a source. Budget callers must account
/// for each missing slot and transient vector growth before using this path.
pub(crate) fn prepare_with_registry_inputs_and_external(
    device: &hex_project::Device,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    input: Option<NativeParameterInputTarget>,
    external: &[NativeParameterInputTarget],
) -> Result<hex_lattice::PreparedParameterAutomation, Invalid> {
    prepare_with_reading_routes(device, registry, input, external, false)
}

pub(crate) fn prepare_closed_effect(
    device: &hex_project::Device,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    input: Option<NativeParameterInputTarget>,
) -> Result<hex_lattice::PreparedParameterAutomation, Invalid> {
    prepare_with_reading_routes(device, registry, input, &[], true)
}

fn prepare_with_reading_routes(
    device: &hex_project::Device,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    input: Option<NativeParameterInputTarget>,
    external: &[NativeParameterInputTarget],
    foreign_preflighted: bool,
) -> Result<hex_lattice::PreparedParameterAutomation, Invalid> {
    if external.len() > MAX_EXTERNAL_PARAMETER_TARGETS {
        return Err(Invalid("recorded parameter target limit exceeded"));
    }
    let mut program = prepare_envelopes_with_registry(device, registry, foreign_preflighted)?;
    // Reading selects values; retained targets keep the same prepared family,
    // latency and resource promise through Base/Authored changes. Existing
    // base-fallback slots hold metadata without points, a clock or a live arm.
    for lane in device.automation().iter().filter(|lane| !lane.is_reading()) {
        match lane.target {
            DeviceAutomationTarget::Parameter(parameter) => {
                let (descriptor, semantics) = provider_description(device, registry, parameter)?;
                program
                    .prepare_external_provider(descriptor, semantics)
                    .map_err(|_| {
                        Invalid("retained native target preparation exceeds its budget")
                    })?;
            }
            DeviceAutomationTarget::HostInputGain | DeviceAutomationTarget::HostOutputGain => {
                program
                    .prepare_external_host(lane.target)
                    .map_err(|_| Invalid("retained host target preparation exceeds its budget"))?;
            }
            DeviceAutomationTarget::Vst3 { .. } => {}
        }
    }

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
                let (descriptor, semantics) = provider_description(device, registry, parameter)?;
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
    for (index, target) in external
        .iter()
        .enumerate()
        .filter(|(_, target)| target.device == device.id())
    {
        if external[..index]
            .iter()
            .any(|previous| previous.device == target.device && previous.target == target.target)
        {
            return Err(Invalid("duplicate recorded automation target"));
        }
        if target.device_type != device.state().type_id()
            || !target_supported(registry, device.state(), target.target)
        {
            return Err(Invalid(
                "recorded parameter target changed or is unsupported",
            ));
        }
        match target.target {
            DeviceAutomationTarget::Parameter(parameter) => {
                let (descriptor, semantics) = provider_description(device, registry, parameter)?;
                program
                    .prepare_external_provider(descriptor, semantics)
                    .map_err(|_| {
                        Invalid("recorded parameter program is invalid or exceeds its budget")
                    })?;
            }
            host => program.prepare_external_host(host).map_err(|_| {
                Invalid("recorded host gain program is invalid or exceeds its budget")
            })?,
        }
    }
    Ok(program)
}

fn provider_description(
    device: &hex_project::Device,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    parameter: ParameterId,
) -> Result<
    (
        hex_lattice_contract::ParameterDescriptor,
        hex_lattice_contract::ParameterSemantics,
    ),
    Invalid,
> {
    let definition = registry
        .definition(device.state().type_id())
        .ok_or(Invalid("native parameter input device is unsupported"))?;
    let descriptor = hex_lattice_contract::validated_state_descriptors(definition, device.state())
        .map_err(|_| Invalid("native parameter input metadata is invalid"))?
        .into_iter()
        .find(|descriptor| descriptor.id == parameter)
        .ok_or(Invalid("native parameter input parameter no longer exists"))?;
    let semantics =
        hex_lattice_contract::validated_parameter_semantics(definition, device.state(), parameter)
            .map_err(|_| Invalid("native parameter input semantics are invalid"))?
            .ok_or(Invalid("native parameter input semantics are undeclared"))?;
    Ok((descriptor, semantics))
}
fn prepare_envelopes_with_registry(
    device: &hex_project::Device,
    registry: &hex_lattice::NativeDeviceRegistry<'_>,
    foreign_preflighted: bool,
) -> Result<hex_lattice::PreparedParameterAutomation, Invalid> {
    let local = |lane: &&hex_project::DeviceAutomation| {
        !foreign_preflighted || !matches!(lane.target, DeviceAutomationTarget::Vst3 { .. })
    };
    if !device.automation().iter().any(|lane| local(&lane)) {
        return Ok(Default::default());
    }
    let definition = registry
        .definition(device.state().type_id())
        .ok_or(Invalid(DEVICE_AUTOMATION_PLAYBACK_UNSUPPORTED))?;
    let count = device
        .reading_automation()
        .filter(local)
        .try_fold(0usize, |sum, lane| sum.checked_add(lane.point_count()))
        .ok_or(Invalid("device automation point count overflow"))?;
    let mut lanes = Vec::new();
    lanes
        .try_reserve_exact(
            device
                .reading_automation()
                .filter(local)
                .filter(|lane| !lane.target.is_host())
                .count(),
        )
        .map_err(|_| Invalid("could not reserve device automation lanes"))?;
    let mut host = Vec::new();
    host.try_reserve_exact(
        device
            .reading_automation()
            .filter(local)
            .filter(|lane| lane.target.is_host())
            .count(),
    )
    .map_err(|_| Invalid("could not reserve host automation lanes"))?;
    let mut points = Vec::new();
    points
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve device automation points"))?;
    for lane in device.automation().iter().filter(local) {
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
    let mut touch = Vec::new();
    touch
        .try_reserve_exact(reading_touch_count(device))
        .map_err(|_| Invalid("could not reserve device automation touch spans"))?;
    for lane in device.reading_automation() {
        for span in &lane.touch {
            let start = points.len();
            points.extend(
                span.steps
                    .iter()
                    .map(|step| hex_lattice::ParameterAutomationPoint {
                        beat_raw: step.beat.raw(),
                        normalized: step.value.get(),
                        step: true,
                    }),
            );
            touch.push(hex_lattice::ParameterAutomationTouchSpan {
                target: lane.target,
                start_beat: span.start.raw(),
                end_beat: span.end.raw(),
                points: start..points.len(),
            });
        }
    }
    hex_lattice::PreparedParameterAutomation::new_with_host_and_touch(lanes, host, points, touch)
        .map_err(|_| Invalid("device automation program is invalid or exceeds its budget"))
}

#[cfg(test)]
#[path = "device_automation_result_tests.rs"]
mod result_tests;

#[cfg(test)]
#[path = "device_parameter_input_tests.rs"]
mod input_tests;

#[cfg(test)]
mod fm4_source_tests;

#[cfg(test)]
#[path = "device_modulation_admission_tests.rs"]
mod modulation_admission_tests;
