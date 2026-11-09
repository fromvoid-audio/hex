//! Explicit control-side shared-envelope conversion. The provider's actual
//! upgrade owns the forty-node correspondence; host metadata owns normalization.
//! This adapter preserves complete model rows and refuses ambiguous laws before
//! emitting the existing atomic state/inventory command. No DSP or custody lives here.
use super::*;
use hex_engine::{NativeDeviceParameterView, NativeDeviceView};
use hex_lattice_contract::{
    DeviceAutomationTarget as Target, ParameterAccess, ParameterAutomation, ParameterId,
    ParameterValue,
};
use hex_project::{AutomationInterpolation, AutomationValue, DeviceAutomation};

pub(super) fn prepare(
    entry: &Device,
    expected: &NativeDeviceState,
    state: &NativeDeviceState,
    next: &EqState,
) -> Result<Option<DeviceAutomationReplacement>, Invalid> {
    if entry.comparison().is_some() {
        return Err(Invalid(
            "eq8 a/b states need conversion together; keep this eq8 device",
        ));
    }
    let first = next
        .nodes
        .first()
        .ok_or(Invalid("eq8 upgrade node mapping is missing"))?
        .id;
    let canonical = EqState::upgrade_legacy(expected, u64::from(first.value()))
        .map_err(|_| Invalid("eq8 source cannot be upgraded"))?;
    if next != &canonical {
        return Err(Invalid("eq8 upgrade mapping changed; keep this eq8 device"));
    }
    let old = hex_engine::native_device_view(expected)
        .map_err(|_| Invalid("eq8 source metadata is malformed"))?;
    let new = hex_engine::native_device_view(state)
        .map_err(|_| Invalid("eq target metadata is malformed"))?;
    if entry.automation().is_empty() {
        return Ok(None);
    }
    let mut rows = Vec::new();
    rows.try_reserve(entry.automation().len().saturating_mul(2))
        .map_err(|_| Invalid("could not reserve eq8 automation conversion"))?;
    for row in entry.automation() {
        let id = match row.target {
            Target::HostInputGain | Target::HostOutputGain => {
                rows.push(row.clone());
                continue;
            }
            Target::Parameter(id) => id,
            Target::Vst3 { .. } => {
                return Err(Invalid(
                    "foreign eq8 automation cannot be converted; keep this eq8 device",
                ));
            }
        };
        let source = parameter(&old, id)?;
        if matches!(id, hex_engine::eq::MODE | hex_engine::eq::OUTPUT_GAIN) {
            compatible(source, parameter(&new, id)?)?;
            rows.push(row.clone());
            continue;
        }
        let raw = id.value();
        if !(1..=200).contains(&raw) {
            return Err(Invalid(
                "unknown eq8 automation target; keep this eq8 device",
            ));
        }
        let slot = (raw - 1) / 5;
        let node = next
            .nodes
            .get(slot as usize)
            .ok_or(Invalid("eq8 target node is missing"))?;
        let field = [
            NodeParameter::Enabled,
            NodeParameter::Kind,
            NodeParameter::Frequency,
            NodeParameter::Gain,
            NodeParameter::Q,
        ][((raw - 1) % 5) as usize];
        let target = node
            .id
            .parameter(field)
            .map_err(|_| Invalid("eq8 target identity is invalid"))?;
        let destination = parameter(&new, target)?;
        if field != NodeParameter::Kind {
            compatible(source, destination)?;
            let mut converted = row.clone();
            converted.target = Target::Parameter(target);
            rows.push(converted);
            continue;
        }
        admitted(destination)?;
        let slope = node
            .id
            .parameter(NodeParameter::Slope)
            .map_err(|_| Invalid("eq8 slope identity is invalid"))?;
        let slope_destination = parameter(&new, slope)?;
        admitted(slope_destination)?;
        // A changing Linear kind segment cannot be represented by simply
        // linearly interpolating the two new discrete fields.
        for pair in row.points.windows(2) {
            if pair[0].outgoing == AutomationInterpolation::Linear
                && kind(source, pair[0].value.get())? != kind(source, pair[1].value.get())?
            {
                return Err(Invalid(
                    "linear eq8 kind changes need a discrete conversion; keep this eq8 device",
                ));
            }
        }
        rows.push(map(row, target, |value| {
            normalize(destination, ParameterValue::Choice(kind(source, value)?.0))
        })?);
        rows.push(map(row, slope, |value| {
            normalize(
                slope_destination,
                ParameterValue::Choice(kind(source, value)?.1),
            )
        })?);
    }
    rows.sort_by_key(|row| row.target);
    // An explicit inventory is necessary even if only Mode/Output rows keep
    // their IDs: ordinary provider replacement deliberately refuses native rows.
    Ok(Some(DeviceAutomationReplacement::new(
        entry.automation().to_vec(),
        rows,
    )?))
}
fn parameter(
    view: &NativeDeviceView,
    id: ParameterId,
) -> Result<&NativeDeviceParameterView, Invalid> {
    view.parameters
        .iter()
        .find(|parameter| parameter.descriptor.id == id)
        .ok_or(Invalid(
            "unknown eq8 automation target; keep this eq8 device",
        ))
}
fn admitted(parameter: &NativeDeviceParameterView) -> Result<(), Invalid> {
    if parameter.semantics.is_none_or(|s| {
        s.access != ParameterAccess::ReadWrite
            || s.automation != ParameterAutomation::DeviceArrangementEnvelope
    }) {
        return Err(Invalid(
            "eq8 shelf automation has no faithful target; keep this eq8 device",
        ));
    }
    Ok(())
}
fn compatible(
    source: &NativeDeviceParameterView,
    target: &NativeDeviceParameterView,
) -> Result<(), Invalid> {
    admitted(target)?;
    let a = source
        .semantics
        .ok_or(Invalid("eq8 normalization is undeclared"))?;
    let b = target
        .semantics
        .ok_or(Invalid("eq normalization is undeclared"))?;
    if source.descriptor.kind != target.descriptor.kind
        || a.normalization != b.normalization
        || a.stepping != b.stepping
    {
        return Err(Invalid(
            "eq8 normalization has no exact mapping; keep this eq8 device",
        ));
    }
    Ok(())
}
fn kind(source: &NativeDeviceParameterView, normalized: f32) -> Result<(u32, u32), Invalid> {
    let value = source
        .semantics
        .ok_or(Invalid("eq8 kind normalization is undeclared"))?
        .normalization
        .denormalize(source.descriptor, normalized)
        .map_err(|_| Invalid("eq8 kind value is invalid"))?;
    match value {
        ParameterValue::Choice(0) => Ok((0, 0)),
        ParameterValue::Choice(3) => Ok((3, 0)),
        ParameterValue::Choice(4) => Ok((3, 3)),
        ParameterValue::Choice(5) => Ok((4, 0)),
        ParameterValue::Choice(6) => Ok((4, 3)),
        ParameterValue::Choice(7) => Ok((5, 0)),
        ParameterValue::Choice(1 | 2) => Err(Invalid(
            "eq8 shelf kind automation needs its legacy law; keep this eq8 device",
        )),
        _ => Err(Invalid("eq8 kind value is invalid")),
    }
}
fn normalize(target: &NativeDeviceParameterView, value: ParameterValue) -> Result<f32, Invalid> {
    target
        .semantics
        .ok_or(Invalid("eq target normalization is undeclared"))?
        .normalization
        .normalize(target.descriptor, value)
        .map_err(|_| Invalid("eq target value is invalid"))
}
fn map(
    row: &DeviceAutomation,
    target: ParameterId,
    value: impl Fn(f32) -> Result<f32, Invalid>,
) -> Result<DeviceAutomation, Invalid> {
    let mut mapped = row.clone();
    mapped.target = Target::Parameter(target);
    for point in &mut mapped.points {
        point.value = AutomationValue::new(value(point.value.get())?)?;
    }
    for span in &mut mapped.touch {
        for step in &mut span.steps {
            step.value = AutomationValue::new(value(step.value.get())?)?;
        }
    }
    Ok(mapped)
}
