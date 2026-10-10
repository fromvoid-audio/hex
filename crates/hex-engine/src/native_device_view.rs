//! Bounded control-side metadata views; never called by the audio callback.
use crate::Invalid;
use hex_lattice_contract::{
    DeviceTypeId, NativeDeviceMetadata, NativeDeviceState, ParameterDescriptor, ParameterValue,
};

/// Distinguishes native effect processors from native track instruments.
/// Type identifiers are resolved only inside the requested class registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeDeviceClass {
    Effect,
    Instrument,
}

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
    /// Whether the editor should enable this control for the current device state.
    pub enabled: bool,
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
    native_device_catalog_for_class(NativeDeviceClass::Effect)
}

/// Enumerate only registered processors from the explicitly selected class.
pub fn native_device_catalog_for_class(
    class: NativeDeviceClass,
) -> Result<Vec<NativeDeviceCatalogEntry>, Invalid> {
    match class {
        NativeDeviceClass::Effect => {
            let registry = hex_devices::native_registry()
                .map_err(|_| Invalid("invalid built-in native device registry"))?;
            catalog_entries(registry.definitions())
        }
        NativeDeviceClass::Instrument => {
            let registry = hex_devices::native_instrument_registry()
                .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
            catalog_entries(registry.definitions())
        }
    }
}

fn catalog_entries<D: NativeDeviceMetadata + ?Sized>(
    definitions: &[&D],
) -> Result<Vec<NativeDeviceCatalogEntry>, Invalid> {
    definitions
        .iter()
        .map(|definition| {
            let type_id = definition.type_id();
            Ok(NativeDeviceCatalogEntry {
                type_id,
                name: display_name(type_id, definition.display_name())?,
                parameters: definition.descriptors(),
            })
        })
        .collect()
}

/// Resolve and migrate once, then read all parameters from the selected class.
pub fn native_device_view_for_class(
    class: NativeDeviceClass,
    state: &NativeDeviceState,
) -> Result<NativeDeviceView, Invalid> {
    match class {
        NativeDeviceClass::Effect => {
            let registry = hex_devices::native_registry()
                .map_err(|_| Invalid("invalid built-in native device registry"))?;
            let definition = registry
                .definition(state.type_id())
                .ok_or(Invalid("native device type is unavailable"))?;
            let migrated = registry
                .migrate_state(state)
                .map_err(|_| Invalid("native device state is unsupported or malformed"))?;
            build_view(state, definition, &migrated)
        }
        NativeDeviceClass::Instrument => {
            let registry = hex_devices::native_instrument_registry()
                .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
            let definition = registry
                .definition(state.type_id())
                .ok_or(Invalid("native instrument type is unavailable"))?;
            let migrated = registry
                .migrate_state(state)
                .map_err(|_| Invalid("native device state is unsupported or malformed"))?;
            build_view(state, definition, &migrated)
        }
    }
}

/// Read one validated parameter using only the selected class registry.
pub fn native_device_parameter_for_class(
    class: NativeDeviceClass,
    state: &NativeDeviceState,
    parameter: hex_lattice_contract::ParameterId,
) -> Result<ParameterValue, Invalid> {
    match class {
        NativeDeviceClass::Effect => {
            let registry = hex_devices::native_registry()
                .map_err(|_| Invalid("invalid built-in native device registry"))?;
            let definition = registry
                .definition(state.type_id())
                .ok_or(Invalid("native device type is unavailable"))?;
            let migrated = registry
                .migrate_state(state)
                .map_err(|_| Invalid("native device state is unsupported or malformed"))?;
            read_parameter(definition, &migrated, parameter)
        }
        NativeDeviceClass::Instrument => {
            let registry = hex_devices::native_instrument_registry()
                .map_err(|_| Invalid("invalid built-in native instrument registry"))?;
            let definition = registry
                .definition(state.type_id())
                .ok_or(Invalid("native instrument type is unavailable"))?;
            let migrated = registry
                .migrate_state(state)
                .map_err(|_| Invalid("native device state is unsupported or malformed"))?;
            read_parameter(definition, &migrated, parameter)
        }
    }
}

/// Look up descriptors only in the explicitly selected class registry.
pub fn native_device_descriptors_for_class(
    class: NativeDeviceClass,
    type_id: DeviceTypeId,
) -> Result<&'static [ParameterDescriptor], Invalid> {
    match class {
        NativeDeviceClass::Effect => hex_devices::native_registry()
            .map_err(|_| Invalid("invalid built-in native device registry"))?
            .definition(type_id)
            .map(NativeDeviceMetadata::descriptors)
            .ok_or(Invalid("native device type is unavailable")),
        NativeDeviceClass::Instrument => hex_devices::native_instrument_registry()
            .map_err(|_| Invalid("invalid built-in native instrument registry"))?
            .definition(type_id)
            .map(NativeDeviceMetadata::descriptors)
            .ok_or(Invalid("native instrument type is unavailable")),
    }
}

fn read_parameter<D: NativeDeviceMetadata + ?Sized>(
    definition: &D,
    state: &NativeDeviceState,
    parameter: hex_lattice_contract::ParameterId,
) -> Result<ParameterValue, Invalid> {
    let descriptor = definition
        .descriptors()
        .iter()
        .find(|descriptor| descriptor.id == parameter)
        .ok_or(Invalid("native device parameter is unavailable"))?;
    let value = definition
        .read_parameter(state, parameter)
        .map_err(|_| Invalid("native device parameter is unavailable"))?;
    descriptor
        .validate(value)
        .map_err(|_| Invalid("native device parameter value is invalid"))?;
    Ok(value)
}

fn build_view<D: NativeDeviceMetadata + ?Sized>(
    state: &NativeDeviceState,
    definition: &D,
    migrated: &NativeDeviceState,
) -> Result<NativeDeviceView, Invalid> {
    let mut parameters = Vec::with_capacity(definition.descriptors().len());
    for descriptor in definition.descriptors() {
        let value = definition
            .read_parameter(migrated, descriptor.id)
            .map_err(|_| Invalid("native device parameter is unavailable"))?;
        descriptor
            .validate(value)
            .map_err(|_| Invalid("native device parameter value is invalid"))?;
        let enabled = definition
            .parameter_enabled(migrated, descriptor.id)
            .map_err(|_| Invalid("native device parameter availability is invalid"))?;
        parameters.push(NativeDeviceParameterView {
            descriptor,
            value,
            enabled,
        });
    }
    Ok(NativeDeviceView {
        type_id: state.type_id(),
        name: display_name(state.type_id(), definition.display_name())?,
        parameters,
    })
}

/// Effect-only compatibility wrapper; class-aware callers should pass an
/// explicit class to `native_device_view_for_class`.
pub fn native_device_view(state: &NativeDeviceState) -> Result<NativeDeviceView, Invalid> {
    native_device_view_for_class(NativeDeviceClass::Effect, state)
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
    fn class_aware_view_exposes_current_conditional_parameter_availability() {
        let catalog = native_device_catalog().unwrap();
        let utility = &catalog[0];
        let state = crate::default_native_device_state(utility.type_id).unwrap();
        let channel = utility
            .parameters
            .iter()
            .find(|parameter| parameter.label == "channel")
            .expect("channel selector");
        let left =
            crate::patch_native_device_parameter(&state, channel.id, ParameterValue::Choice(1))
                .unwrap();
        let view = native_device_view(&left).unwrap();
        for label in ["width", "mid-side", "mid-side balance"] {
            let parameter = view
                .parameters
                .iter()
                .find(|parameter| parameter.descriptor.label == label)
                .expect("conditional control");
            assert!(!parameter.enabled, "{label} is unavailable in left mode");
        }
        let width = view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.label == "width")
            .unwrap();
        assert_eq!(width.value, ParameterValue::Float(1.0));
        let stereo = native_device_view(&state).unwrap();
        assert!(stereo.parameters.iter().all(|parameter| parameter.enabled));
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

    #[test]
    fn instrument_facades_resolve_only_the_instrument_registry() {
        let effects = native_device_catalog_for_class(NativeDeviceClass::Effect).unwrap();
        let instruments = native_device_catalog_for_class(NativeDeviceClass::Instrument).unwrap();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].name, "utility");
        assert_eq!(instruments.len(), 3);
        let sine = instruments
            .iter()
            .find(|entry| entry.name == "sine")
            .expect("original sine instrument remains registered");
        let fm4 = instruments
            .iter()
            .find(|entry| entry.name == "fm4")
            .expect("FM4 instrument is registered");
        let operator = instruments
            .iter()
            .find(|entry| entry.name == "operator")
            .expect("Operator instrument is registered");

        let state = crate::default_native_device_state_for_class(
            NativeDeviceClass::Instrument,
            sine.type_id,
        )
        .unwrap();
        let before = state.clone();
        let view = native_device_view_for_class(NativeDeviceClass::Instrument, &state).unwrap();
        assert_eq!(view.name, "sine");
        assert!(view.parameters.is_empty());
        assert_eq!(
            native_device_descriptors_for_class(NativeDeviceClass::Instrument, sine.type_id,)
                .unwrap(),
            sine.parameters
        );

        assert!(native_device_view_for_class(NativeDeviceClass::Effect, &state).is_err());
        assert!(
            crate::default_native_device_state_for_class(NativeDeviceClass::Effect, sine.type_id,)
                .is_err()
        );
        assert!(
            crate::patch_native_device_parameter_for_class(
                NativeDeviceClass::Effect,
                &state,
                hex_lattice_contract::ParameterId::new(1).unwrap(),
                ParameterValue::Float(0.5),
            )
            .is_err()
        );
        assert_eq!(
            state, before,
            "wrong-class access preserves the source bytes"
        );

        let fm4_state = crate::default_native_device_state_for_class(
            NativeDeviceClass::Instrument,
            fm4.type_id,
        )
        .unwrap();
        let fm4_view =
            native_device_view_for_class(NativeDeviceClass::Instrument, &fm4_state).unwrap();
        assert_eq!(fm4_view.name, "fm4");
        assert_eq!(fm4_view.parameters.len(), 32);
        assert_eq!(fm4_view.parameters[0].value, ParameterValue::Float(1.0));
        assert_eq!(
            native_device_descriptors_for_class(NativeDeviceClass::Instrument, fm4.type_id)
                .unwrap(),
            fm4.parameters
        );
        assert!(native_device_view_for_class(NativeDeviceClass::Effect, &fm4_state).is_err());

        let operator_state = crate::default_native_device_state_for_class(
            NativeDeviceClass::Instrument,
            operator.type_id,
        )
        .unwrap();
        let operator_view =
            native_device_view_for_class(NativeDeviceClass::Instrument, &operator_state).unwrap();
        assert_eq!(operator_view.name, "operator");
        assert_eq!(operator_view.parameters.len(), 69);
        for parameter in operator_view
            .parameters
            .iter()
            .filter(|parameter| (57..=61).contains(&parameter.descriptor.id.value()))
        {
            assert_eq!(parameter.value, ParameterValue::Choice(0));
        }
        assert_eq!(
            native_device_descriptors_for_class(NativeDeviceClass::Instrument, operator.type_id)
                .unwrap(),
            operator.parameters
        );
        assert!(native_device_view_for_class(NativeDeviceClass::Effect, &operator_state).is_err());

        let effect_state = crate::default_native_device_state_for_class(
            NativeDeviceClass::Effect,
            effects[0].type_id,
        )
        .unwrap();
        assert!(
            native_device_view_for_class(NativeDeviceClass::Instrument, &effect_state).is_err()
        );
        assert!(
            crate::patch_native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                &effect_state,
                effects[0].parameters[0].id,
                ParameterValue::Float(0.5),
            )
            .is_err()
        );
        assert_eq!(
            native_device_view_for_class(NativeDeviceClass::Effect, &effect_state)
                .unwrap()
                .name,
            "utility"
        );
    }

    #[test]
    fn unsupported_instrument_states_are_not_rewritten_or_defaulted() {
        let instrument = native_device_catalog_for_class(NativeDeviceClass::Instrument)
            .unwrap()
            .remove(0);
        let future = NativeDeviceState::new(instrument.type_id, u32::MAX, Vec::new()).unwrap();
        let malformed = NativeDeviceState::new(instrument.type_id, 1, vec![1]).unwrap();
        for state in [future, malformed] {
            let before = state.clone();
            assert!(native_device_view_for_class(NativeDeviceClass::Instrument, &state).is_err());
            assert!(
                crate::patch_native_device_parameter_for_class(
                    NativeDeviceClass::Instrument,
                    &state,
                    hex_lattice_contract::ParameterId::new(1).unwrap(),
                    ParameterValue::Float(0.5),
                )
                .is_err()
            );
            assert_eq!(state, before);
        }
    }
}
