//! Canonical source-program compilation and bounded control-side storage.
//!
//! Programs belong to the receiving source inventory, never a contributor note
//! template or DSP twin. Source-global rendering borrows these programs; actual
//! factory grants and project ownership control admission. Quotes include retained vectors,
//! normalized staging, descriptor validation and unread-lane insertion overlap.
use crate::{AudioStorageError, Invalid};
use hex_lattice::{
    ParameterAutomationLane, ParameterAutomationPoint, ParameterAutomationTouchSpan,
    PreparedMutableInstrumentRuntime, PreparedParameterAutomation,
};
use hex_lattice_contract::{
    DeviceAutomationTarget, NativeDeviceMetadata, ParameterAccess, ParameterAutomation,
    ParameterDescriptor, ParameterId,
};

const MAX_PROGRAM_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct Program {
    pub(super) device: hex_project::DeviceId,
    pub(super) values: PreparedParameterAutomation,
}
impl Program {
    pub(super) fn visit<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        // Separate from the inventory-wide marker used by resource factories.
        if ledger.native_payload_once(&self.values)? {
            let (ids, lanes, points) = self.values.storage_vectors();
            ledger.vec(ids)?;
            ledger.vec(lanes)?;
            ledger.vec(points)?;
            ledger.vec(self.values.touch_storage())?;
        }
        Ok(())
    }
}

/// Ordinary program allowance only; four vector rows include empty storage.
pub(super) fn reservation() -> crate::AudioStorageLedgerReservation {
    crate::AudioStorageLedgerReservation {
        data_rows: hex_project::MAX_TRACKS * 4,
        handle_rows: 0,
        root_rows: hex_project::MAX_TRACKS,
        max_retained_bytes: MAX_PROGRAM_BYTES,
        max_construction_peak_bytes: MAX_PROGRAM_BYTES,
    }
}
/// One receiving program per source. Previous retained programs coexist with
/// the current program's retained and temporary backing during compilation.
pub(super) fn construction_bytes(
    project: &hex_project::Project,
    native_input: Option<crate::NativeParameterInputTarget>,
) -> Result<usize, Invalid> {
    let mut retained = 0;
    let mut peak = 0;
    for track in project
        .tracks()
        .iter()
        .filter(|track| track.receives_source_audio())
    {
        let Some(device) = track.instrument().filter(|device| {
            (!device.automation().is_empty() || armed_parameter(device, native_input).is_some())
                && !crate::compile::is_vst3_instrument(device)
        }) else {
            continue;
        };
        let (bytes, temporary) =
            quote_lanes(device.automation(), armed_parameter(device, native_input))?;
        retained = add_budget(retained, bytes)?;
        peak = peak.max(add_budget(retained, temporary)?);
    }
    Ok(peak)
}

#[cfg(test)]
#[cfg(test)]
pub(super) fn quote(device: &hex_project::Device) -> Result<(usize, usize), Invalid> {
    quote_lanes(device.automation(), None)
}

fn armed_parameter(
    device: &hex_project::Device,
    native_input: Option<crate::NativeParameterInputTarget>,
) -> Option<ParameterId> {
    native_input
        .filter(|input| {
            input.device() == device.id()
                && matches!(
                    input.identity().owner(),
                    hex_lattice::ProcessorChainOwner::Source(_)
                )
        })
        .and_then(|input| input.target().provider_parameter())
}

fn quote_lanes(
    automation: &[hex_project::DeviceAutomation],
    armed: Option<ParameterId>,
) -> Result<(usize, usize), Invalid> {
    let extra = armed.is_some_and(|id| {
        !automation
            .iter()
            .any(|lane| lane.target == DeviceAutomationTarget::Parameter(id))
    });
    let lanes = add_budget(automation.len(), usize::from(extra))?;
    let points = automation
        .iter()
        .filter(|lane| lane.is_reading())
        .try_fold(0usize, |n, lane| {
            n.checked_add(lane.point_count())
                .ok_or(Invalid("instrument automation point count overflow"))
        })?;
    let touch = automation
        .iter()
        .filter(|lane| lane.is_reading())
        .try_fold(0usize, |n, lane| {
            n.checked_add(lane.touch.len())
                .ok_or(Invalid("instrument automation touch count overflow"))
        })?;
    let retained = PreparedParameterAutomation::quoted_storage_bytes(lanes, points)
        .and_then(|n| {
            n.checked_add(PreparedParameterAutomation::quoted_touch_storage_bytes(
                touch,
            )?)
        })
        .ok_or(Invalid("instrument automation storage overflow"))?;
    let overlap = PreparedParameterAutomation::quoted_construction_overlap_bytes(lanes, points)
        .and_then(|n| n.checked_add(retained)) // sequential Base-fallback insertion
        .and_then(|n| {
            n.checked_add(lanes.checked_mul(std::mem::size_of::<(
                ParameterDescriptor,
                hex_lattice_contract::ParameterSemantics,
            )>())?)
        })
        .and_then(|n| {
            n.checked_add(
                hex_lattice_contract::MAX_PARAMETERS_PER_DEVICE
                    .checked_mul(std::mem::size_of::<ParameterDescriptor>())?,
            )
        })
        .ok_or(Invalid("instrument automation construction overflow"))?;
    if retained
        .checked_add(overlap)
        .is_none_or(|n| n > MAX_PROGRAM_BYTES)
    {
        return Err(Invalid("instrument automation storage budget exceeded"));
    }
    Ok((retained, overlap))
}

#[cfg(test)]
pub(super) fn prepare(
    device: &hex_project::Device,
    definition: &dyn NativeDeviceMetadata,
    factory: &dyn PreparedMutableInstrumentRuntime,
) -> Result<Option<Program>, Invalid> {
    prepare_lanes(
        device.id(),
        device.state(),
        device.automation(),
        definition,
        factory,
    )
}

#[cfg(test)]
fn prepare_lanes(
    device: hex_project::DeviceId,
    state: &hex_lattice_contract::NativeDeviceState,
    automation: &[hex_project::DeviceAutomation],
    definition: &dyn NativeDeviceMetadata,
    factory: &dyn PreparedMutableInstrumentRuntime,
) -> Result<Option<Program>, Invalid> {
    prepare_lanes_with_input(device, state, automation, definition, factory, None)
}

fn prepare_lanes_with_input(
    device: hex_project::DeviceId,
    state: &hex_lattice_contract::NativeDeviceState,
    automation: &[hex_project::DeviceAutomation],
    definition: &dyn NativeDeviceMetadata,
    factory: &dyn PreparedMutableInstrumentRuntime,
    armed: Option<ParameterId>,
) -> Result<Option<Program>, Invalid> {
    hex_project::validate_device_automation(automation)
        .map_err(|_| Invalid("instrument automation lanes are invalid"))?;
    if automation.is_empty() && armed.is_none() {
        return Ok(None);
    }
    let (retained, _) = quote_lanes(automation, armed)?;
    let descriptors = hex_lattice_contract::validated_state_descriptors(definition, state)
        .map_err(|_| Invalid("instrument automation metadata is invalid"))?;
    let grants = factory.parameter_automation_descriptors();
    if grants.len() > hex_lattice_contract::MAX_PARAMETERS_PER_DEVICE
        || grants.windows(2).any(|pair| pair[0].id >= pair[1].id)
        || grants
            .iter()
            .any(|descriptor| descriptor.validate_descriptor().is_err())
    {
        return Err(Invalid("instrument automation grants are invalid"));
    }
    let mut lanes = Vec::new();
    let mut points = Vec::new();
    let mut touch = Vec::new();
    lanes
        .try_reserve_exact(automation.iter().filter(|lane| lane.is_reading()).count())
        .map_err(|_| Invalid("could not reserve instrument automation lanes"))?;
    let point_count = automation
        .iter()
        .filter(|lane| lane.is_reading())
        .try_fold(0usize, |n, lane| n.checked_add(lane.point_count()))
        .ok_or(Invalid("instrument automation point count overflow"))?;
    let touch_count = automation
        .iter()
        .filter(|lane| lane.is_reading())
        .try_fold(0usize, |n, lane| n.checked_add(lane.touch.len()))
        .ok_or(Invalid("instrument automation touch count overflow"))?;
    points
        .try_reserve_exact(point_count)
        .map_err(|_| Invalid("could not reserve instrument automation points"))?;
    touch
        .try_reserve_exact(touch_count)
        .map_err(|_| Invalid("could not reserve instrument automation touch ranges"))?;
    let mut retained_targets = Vec::new();
    retained_targets
        .try_reserve_exact(automation.iter().filter(|lane| !lane.is_reading()).count())
        .map_err(|_| Invalid("could not reserve retained instrument targets"))?;
    for lane in automation {
        let DeviceAutomationTarget::Parameter(id) = lane.target else {
            return Err(Invalid("instrument automation target is unsupported"));
        };
        let descriptor = *descriptors
            .iter()
            .find(|d| d.id == id)
            .ok_or(Invalid("instrument automation parameter is missing"))?;
        if !grants.contains(&descriptor) {
            return Err(Invalid(
                "instrument runtime does not grant parameter automation",
            ));
        }
        let semantics = definition
            .parameter_semantics_for_state(state, id)
            .map_err(|_| Invalid("instrument automation semantics are invalid"))?
            .ok_or(Invalid("instrument automation semantics are undeclared"))?;
        semantics
            .validate(descriptor)
            .map_err(|_| Invalid("instrument automation semantics are invalid"))?;
        if semantics.access != ParameterAccess::ReadWrite
            || semantics.automation != ParameterAutomation::DeviceArrangementEnvelope
        {
            return Err(Invalid("instrument parameter automation is unsupported"));
        }
        if !lane.is_reading() {
            retained_targets.push((descriptor, semantics));
            continue;
        }
        let start = points.len();
        points.extend(lane.points.iter().map(|point| ParameterAutomationPoint {
            beat_raw: point.beat.raw(),
            normalized: point.value.get(),
            step: point.outgoing == hex_project::AutomationInterpolation::Step,
        }));
        lanes.push(ParameterAutomationLane {
            descriptor,
            semantics,
            points: start..points.len(),
        });
    }
    for lane in automation.iter().filter(|lane| lane.is_reading()) {
        for span in &lane.touch {
            let start = points.len();
            points.extend(span.steps.iter().map(|step| ParameterAutomationPoint {
                beat_raw: step.beat.raw(),
                normalized: step.value.get(),
                step: true,
            }));
            touch.push(ParameterAutomationTouchSpan {
                target: lane.target,
                start_beat: span.start.raw(),
                end_beat: span.end.raw(),
                points: start..points.len(),
            });
        }
    }
    let mut values =
        PreparedParameterAutomation::new_with_host_and_touch(lanes, Vec::new(), points, touch)
            .map_err(|_| Invalid("instrument automation program is invalid"))?;
    for (descriptor, semantics) in retained_targets {
        values
            .prepare_external_provider(descriptor, semantics)
            .map_err(|_| Invalid("retained instrument automation target is invalid"))?;
    }
    if let Some(id) = armed {
        let descriptor = *descriptors
            .iter()
            .find(|descriptor| descriptor.id == id)
            .ok_or(Invalid("armed instrument parameter is missing"))?;
        if !grants.contains(&descriptor) {
            return Err(Invalid(
                "instrument runtime does not grant live parameter input",
            ));
        }
        let semantics = definition
            .parameter_semantics_for_state(state, id)
            .map_err(|_| Invalid("armed instrument semantics are invalid"))?
            .ok_or(Invalid("armed instrument semantics are undeclared"))?;
        values
            .arm_provider_input(descriptor, semantics)
            .map_err(|_| Invalid("instrument live parameter input is unsupported"))?;
    }
    if values.storage_bytes().is_none_or(|n| n > retained) {
        return Err(Invalid(
            "instrument automation actual storage exceeds quote",
        ));
    }
    Ok(Some(Program { device, values }))
}

pub(super) fn prepare_for_receiver(
    track: &hex_project::Track,
    factory: Option<&dyn PreparedMutableInstrumentRuntime>,
    registry: &hex_lattice::NativeInstrumentRegistry<'_>,
    native_input: Option<crate::NativeParameterInputTarget>,
    stored_bytes: &mut usize,
) -> Result<Option<Program>, Invalid> {
    let (Some(device), Some(factory)) = (track.instrument(), factory) else {
        return Ok(None);
    };
    let armed = armed_parameter(device, native_input);
    if !track.receives_source_audio() || device.automation().is_empty() && armed.is_none() {
        return Ok(None);
    }
    let (retained, overlap) = quote_lanes(device.automation(), armed)?;
    add_budget(add_budget(*stored_bytes, retained)?, overlap)?;
    let definition = registry
        .definition(device.state().type_id())
        .ok_or(Invalid("instrument automation definition is missing"))?;
    let program = prepare_lanes_with_input(
        device.id(),
        device.state(),
        device.automation(),
        definition,
        factory,
        armed,
    )?;
    let actual = program
        .as_ref()
        .and_then(|p| p.values.storage_bytes())
        .ok_or(Invalid("instrument automation storage is unknown"))?;
    *stored_bytes = add_budget(*stored_bytes, actual)?;
    Ok(program)
}

pub(super) fn add_budget(current: usize, additional: usize) -> Result<usize, Invalid> {
    current
        .checked_add(additional)
        .filter(|n| *n <= MAX_PROGRAM_BYTES)
        .ok_or(Invalid(
            "source automation program pool exceeds storage budget",
        ))
}

#[cfg(test)]
mod tests;
