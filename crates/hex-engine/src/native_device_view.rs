//! Bounded control-side metadata views; never called by the audio callback.
use crate::Invalid;
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState, ParameterDescriptor, ParameterValue};

/// A registered and implemented native processor available to the add menu.
#[derive(Clone, Debug)]
pub struct NativeDeviceCatalogEntry {
    pub type_id: DeviceTypeId,
    pub name: String,
    pub parameters: &'static [ParameterDescriptor],
}

/// Validated typed value paired with its device-owned presentation descriptor.
#[derive(Clone, Copy, Debug)]
pub struct NativeDeviceParameterView {
    pub descriptor: &'static ParameterDescriptor,
    pub value: ParameterValue,
}

/// One immutable device's complete read-only parameter view. Consumers cache
/// this by canonical state identity instead of resolving the registry per knob.
#[derive(Clone, Debug)]
pub struct NativeDeviceView {
    pub type_id: DeviceTypeId,
    pub name: String,
    pub parameters: Vec<NativeDeviceParameterView>,
}

/// Enumerate only actual registered processors, without constructing DSP or
/// defaults. Registry bounds cap this control-side allocation at 256 entries.
pub fn native_device_catalog() -> Result<Vec<NativeDeviceCatalogEntry>, Invalid> {
    let registry = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    registry
        .definitions()
        .iter()
        .map(|definition| {
            Ok(NativeDeviceCatalogEntry {
                type_id: definition.type_id(),
                name: display_name(definition.type_id(), definition.display_name())?,
                parameters: definition.descriptors(),
            })
        })
        .collect()
}

/// Resolve and migrate once, then read all parameters. Unknown, malformed and
/// unsupported future states fail visibly without changing the original state.
pub fn native_device_view(state: &NativeDeviceState) -> Result<NativeDeviceView, Invalid> {
    let registry = hex_devices::native_registry()
        .map_err(|_| Invalid("invalid built-in native device registry"))?;
    let definition = registry
        .definition(state.type_id())
        .ok_or(Invalid("native device type is unavailable"))?;
    let migrated = registry
        .migrate_state(state)
        .map_err(|_| Invalid("native device state is unsupported or malformed"))?;
    let mut parameters = Vec::with_capacity(definition.descriptors().len());
    for descriptor in definition.descriptors() {
        let value = definition
            .read_parameter(&migrated, descriptor.id)
            .map_err(|_| Invalid("native device parameter is unavailable"))?;
        descriptor
            .validate(value)
            .map_err(|_| Invalid("native device parameter value is invalid"))?;
        parameters.push(NativeDeviceParameterView { descriptor, value });
    }
    Ok(NativeDeviceView {
        type_id: state.type_id(),
        name: display_name(state.type_id(), definition.display_name())?,
        parameters,
    })
}

fn display_name(type_id: DeviceTypeId, name: Option<&str>) -> Result<String, Invalid> {
    match name {
        Some(name) => Ok(name.to_owned()),
        None => type_id
            .as_str()
            .map(str::to_owned)
            .map_err(|_| Invalid("native device type identifier is invalid")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_and_batched_values_use_registered_metadata_and_current_state() {
        let catalog = native_device_catalog().unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].name, "utility");
        let original = crate::default_native_device_state(catalog[0].type_id).unwrap();
        let first = catalog[0].parameters[0];
        let changed =
            crate::patch_native_device_parameter(&original, first.id, ParameterValue::Float(0.25))
                .unwrap();
        let view = native_device_view(&changed).unwrap();
        assert_eq!(view.name, "utility");
        assert_eq!(view.parameters.len(), catalog[0].parameters.len());
        assert_eq!(view.parameters[0].value, ParameterValue::Float(0.25));
        assert_eq!(
            crate::native_device_parameter(&original, first.id).unwrap(),
            first.default
        );
    }

    #[test]
    fn unsupported_or_malformed_states_are_not_presented_as_defaults() {
        let entry = native_device_catalog().unwrap().remove(0);
        let unknown =
            NativeDeviceState::new(DeviceTypeId::new("test.missing").unwrap(), 1, vec![1]).unwrap();
        let future = NativeDeviceState::new(entry.type_id, u32::MAX, vec![1]).unwrap();
        let malformed = NativeDeviceState::new(entry.type_id, 1, vec![1]).unwrap();
        for state in [unknown, future, malformed] {
            let before = state.clone();
            assert!(native_device_view(&state).is_err());
            assert_eq!(state, before);
        }
    }
}
