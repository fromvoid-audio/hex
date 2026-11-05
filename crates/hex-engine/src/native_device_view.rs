//! Bounded control-side metadata views; never called by the audio callback.
use crate::Invalid;
use hex_lattice_contract::{
    DeviceTypeId, NativeDeviceMetadata, NativeDeviceState, ParameterDescriptor, ParameterValue,
};

/// Intentional host facade for the provider-owned closed scalar contract.
pub use hex_devices::sampler::control::SamplerParameter;

pub fn sampler_parameter_value_on_control(
    parameter: SamplerParameter,
    state: &NativeDeviceState,
) -> Result<f64, Invalid> {
    hex_devices::sampler::control::sampler_parameter_value_on_control(parameter, state)
        .map_err(sampler_control_error)
}

/// Values remain readable when inactive; this never converts a gate mode.
pub fn sampler_parameter_active_on_control(
    parameter: SamplerParameter,
    state: &NativeDeviceState,
) -> Result<bool, Invalid> {
    hex_devices::sampler::control::sampler_parameter_active_on_control(parameter, state)
        .map_err(sampler_control_error)
}

pub fn patch_sampler_parameter_on_control(
    parameter: SamplerParameter,
    state: &NativeDeviceState,
    value: f64,
) -> Result<NativeDeviceState, Invalid> {
    hex_devices::sampler::control::patch_sampler_parameter_on_control(parameter, state, value)
        .map_err(sampler_control_error)
}

fn sampler_control_error(error: hex_devices::sampler::control::ControlError) -> Invalid {
    use hex_devices::sampler::control::ControlError;
    Invalid(match error {
        ControlError::State(error) => return sampler_state_error(error),
        ControlError::InvalidValue(parameter) => match parameter {
            SamplerParameter::AttackMs
            | SamplerParameter::DecayMs
            | SamplerParameter::ReleaseMs => {
                "sampler envelope time must be between zero and 60000 ms"
            }
            SamplerParameter::SustainPercent => {
                "sampler sustain must be between zero and 100 percent"
            }
            SamplerParameter::Gain => "sampler gain must be between zero and four",
            SamplerParameter::Transpose => "sampler pitch must be between -48 and 48 semitones",
            SamplerParameter::LoopEnabled => "sampler loop must be off or on",
            SamplerParameter::LoopCrossfadeMs => {
                "sampler loop crossfade must be between zero and 10000 ms"
            }
            SamplerParameter::StartFrame
            | SamplerParameter::EndFrame
            | SamplerParameter::LoopStartFrame
            | SamplerParameter::LoopEndFrame => {
                "sampler region endpoints must be bounded whole source frames"
            }
        },
        ControlError::RequiresClassic => "loop controls require classic mode",
        ControlError::InvalidRegion => {
            "sampler region must be nonempty and preserve loop and slice bounds"
        }
        ControlError::InvalidSetting => "sampler setting is invalid",
    })
}

/// Exact asset-free catalog state; all other sampler states require source validation.
pub fn is_empty_sampler_state_on_control(state: &NativeDeviceState) -> bool {
    hex_devices::sampler::state::is_empty_state(state)
}

/// Refuse unsupported modes before the host decodes any replacement media.
pub fn validate_sampler_replacement_on_control(state: &NativeDeviceState) -> Result<(), Invalid> {
    if is_empty_sampler_state_on_control(state) {
        return Ok(());
    }
    let authored = hex_devices::sampler::state::SamplerState::from_native(state)
        .map_err(sampler_state_error)?;
    if matches!(
        authored.mode,
        hex_devices::sampler::state::AuthoredMode::Classic { .. }
    ) {
        Ok(())
    } else {
        Err(Invalid(
            "sample replacement currently requires classic mode",
        ))
    }
}
/// Preserve every non-source setting and host envelope; reset only source bounds
/// and Classic loop points. The project transaction binds the new asset identity.
pub fn sampler_replacement_template_on_control(
    state: &NativeDeviceState,
    frames: usize,
) -> Result<(NativeDeviceState, hex_lattice_contract::DeviceAssetSlot), Invalid> {
    validate_sampler_replacement_on_control(state)?;
    use hex_devices::sampler::state::{AuthoredMode, SamplerState};
    let mut authored = if is_empty_sampler_state_on_control(state) {
        SamplerState::for_source_frames(frames).map_err(sampler_state_error)?
    } else {
        SamplerState::from_native(state).map_err(sampler_state_error)?
    };
    let whole = SamplerState::for_source_frames(frames)
        .map_err(sampler_state_error)?
        .region;
    authored.region = whole;
    authored.mode = AuthoredMode::Classic {
        loop_enabled: false,
        loop_region: whole,
    };
    Ok((
        if is_empty_sampler_state_on_control(state) {
            authored
                .to_native()
                .and_then(|s| s.with_host_metadata_from(state))
                .map_err(sampler_state_error)?
        } else {
            authored.replace_in(state).map_err(sampler_state_error)?
        },
        hex_lattice_contract::DeviceAssetSlot::new(hex_devices::sampler::state::SOURCE_SLOT)
            .map_err(sampler_state_error)?,
    ))
}

/// Classify the closed set of panel configuration fields while preserving every
/// other codec/host field and the exact source binding. Control thread only.
pub fn is_sampler_configuration_history_on_control(
    current: &NativeDeviceState,
    target: &NativeDeviceState,
) -> Result<bool, Invalid> {
    use hex_devices::sampler::state::{AuthoredMode, SamplerState};
    let mut allowed = SamplerState::from_native(current).map_err(sampler_state_error)?;
    let next = SamplerState::from_native(target).map_err(sampler_state_error)?;
    allowed.region = next.region;
    allowed.gain = next.gain;
    allowed.transpose_semitones = next.transpose_semitones;
    let hex_devices::sampler::Envelope {
        attack_seconds,
        decay_seconds,
        sustain,
        release_seconds,
    } = next.envelope;
    allowed.envelope.attack_seconds = attack_seconds;
    allowed.envelope.decay_seconds = decay_seconds;
    allowed.envelope.sustain = sustain;
    allowed.envelope.release_seconds = release_seconds;
    if let (
        AuthoredMode::Classic {
            loop_enabled,
            loop_region,
        },
        AuthoredMode::Classic {
            loop_enabled: next_enabled,
            loop_region: next_region,
        },
    ) = (&mut allowed.mode, &next.mode)
    {
        *loop_enabled = *next_enabled;
        *loop_region = *next_region;
    }
    Ok(allowed.replace_in(current).map_err(sampler_state_error)? == *target)
}

fn sampler_state_error(error: hex_lattice_contract::DeviceContractError) -> Invalid {
    Invalid(match error {
        hex_lattice_contract::DeviceContractError::InvalidStateVersion => {
            "sampler state version is unsupported"
        }
        _ => "sampler state is invalid",
    })
}

/// Reserved host parameter. Device codecs never own this identifier.
pub const DEVICE_OUTPUT_GAIN: hex_lattice_contract::ParameterDescriptor =
    hex_lattice_contract::ParameterDescriptor {
        id: hex_lattice_contract::ParameterId::from_static(u32::MAX),
        label: "output gain",
        kind: hex_lattice_contract::ParameterKind::Float {
            min: hex_lattice_contract::HostGain::MIN_DB,
            max: hex_lattice_contract::HostGain::MAX_DB,
            unit: Some("db"),
        },
        default: ParameterValue::Float(0.0),
        presentation: hex_lattice_contract::ParameterPresentation::Linear,
    };

pub const DEVICE_INPUT_GAIN: hex_lattice_contract::ParameterDescriptor =
    hex_lattice_contract::ParameterDescriptor {
        id: hex_lattice_contract::ParameterId::from_static(u32::MAX - 1),
        label: "input gain",
        kind: hex_lattice_contract::ParameterKind::Float {
            min: hex_lattice_contract::HostGain::MIN_DB,
            max: hex_lattice_contract::HostGain::MAX_DB,
            unit: Some("db"),
        },
        default: ParameterValue::Float(0.0),
        presentation: hex_lattice_contract::ParameterPresentation::Linear,
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
    pub descriptor: ParameterDescriptor,
    pub semantics: Option<hex_lattice_contract::ParameterSemantics>,
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
        .filter(|definition| definition.type_id().as_str() != Ok("hex.eq8"))
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

/// Sample a prepared native effect's two processor-lane responses at bounded
/// frequencies. These lanes are processor-defined (EQ8 stereo, L/R or M/S),
/// not a general physical L/R transfer matrix. This control-side query uses
/// prepared coefficients so visualizations cannot drift from the actual DSP.
pub fn native_effect_frequency_response(
    state: &NativeDeviceState,
    sample_rate: u32,
    frequencies_hz: &[f32],
) -> Result<Option<Vec<[f32; 2]>>, Invalid> {
    hex_devices::native_effect_frequency_response(state, sample_rate, frequencies_hz)
        .map_err(|error| Invalid(error.message()))
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
    if parameter == DEVICE_INPUT_GAIN.id {
        return Ok(ParameterValue::Float(state.input_gain_db()));
    }
    if parameter == DEVICE_OUTPUT_GAIN.id {
        return Ok(ParameterValue::Float(state.output_gain_db()));
    }
    let descriptors = hex_lattice_contract::validated_state_descriptors(definition, state)
        .map_err(|_| Invalid("native device descriptors are invalid"))?;
    let descriptor = descriptors
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
    let descriptors = hex_lattice_contract::validated_state_parameters(definition, migrated)
        .map_err(|_| Invalid("native device descriptors are invalid"))?;
    let mut parameters = Vec::with_capacity(descriptors.len());
    for (descriptor, semantics) in descriptors {
        let value = definition
            .read_parameter(migrated, descriptor.id)
            .map_err(|_| Invalid("native device parameter is unavailable"))?;
        descriptor
            .validate(value)
            .map_err(|_| Invalid("native device parameter value is invalid"))?;
        let enabled = definition
            .parameter_enabled(migrated, descriptor.id)
            .map_err(|_| Invalid("native device parameter availability is invalid"))?
            && !semantics
                .is_some_and(|s| s.access == hex_lattice_contract::ParameterAccess::ReadOnly);
        parameters.push(NativeDeviceParameterView {
            descriptor,
            semantics,
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
    use hex_lattice_contract::ParameterKind;

    #[test]
    fn catalog_and_batched_values_use_registered_metadata_and_current_state() {
        let catalog = native_device_catalog().unwrap();
        assert_eq!(catalog.len(), 11);
        let utility = catalog
            .iter()
            .find(|entry| entry.name == "utility")
            .unwrap();
        let original = crate::default_native_device_state(utility.type_id).unwrap();
        let first = utility.parameters[0];
        let changed =
            crate::patch_native_device_parameter(&original, first.id, ParameterValue::Float(0.25))
                .unwrap();
        let view = native_device_view(&changed).unwrap();
        assert_eq!(view.name, "utility");
        assert_eq!(view.parameters.len(), utility.parameters.len());
        assert_eq!(view.parameters[0].value, ParameterValue::Float(0.25));
        assert_eq!(
            crate::native_device_parameter(&original, first.id).unwrap(),
            first.default
        );
        let saturator = catalog
            .iter()
            .find(|entry| entry.name == "saturator")
            .expect("registered Saturator appears in the generic effect catalog");
        let state = crate::default_native_device_state(saturator.type_id).unwrap();
        assert_eq!(native_device_view(&state).unwrap().name, "saturator");
        let compressor = catalog
            .iter()
            .find(|entry| entry.name == "compressor")
            .expect("registered Compressor appears in the generic effect catalog");
        let state = crate::default_native_device_state(compressor.type_id).unwrap();
        assert_eq!(native_device_view(&state).unwrap().name, "compressor");
        let gate = catalog
            .iter()
            .find(|entry| entry.name == "gate")
            .expect("registered Gate appears in the generic effect catalog");
        let state = crate::default_native_device_state(gate.type_id).unwrap();
        assert_eq!(native_device_view(&state).unwrap().name, "gate");
        let reverb = catalog
            .iter()
            .find(|entry| entry.name == "reverb")
            .expect("registered reverb in browser catalog");
        assert_eq!(reverb.parameters.len(), 33);
        let state = crate::default_native_device_state(reverb.type_id).unwrap();
        assert_eq!(native_device_view(&state).unwrap().name, "reverb");
        let overdrive = catalog
            .iter()
            .find(|entry| entry.name == "overdrive")
            .expect("registered overdrive");
        let state = crate::default_native_device_state(overdrive.type_id).unwrap();
        let view = native_device_view(&state).unwrap();
        assert_eq!(view.name, "overdrive");
        assert_eq!(view.parameters.len(), 6);
        let auto_pan = catalog
            .iter()
            .find(|entry| entry.name == "auto pan")
            .expect("registered Auto Pan appears in the generic effect catalog");
        let state = crate::default_native_device_state(auto_pan.type_id).unwrap();
        let view = native_device_view(&state).unwrap();
        assert_eq!(view.name, "auto pan");
        assert_eq!(view.parameters.len(), 4);
        assert_eq!(
            view.parameters[3].descriptor.id,
            hex_lattice_contract::ParameterId::from_static(4)
        );
        assert_eq!(view.parameters[3].value, ParameterValue::Choice(0));
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
    fn prepared_frequency_response_uses_registered_effect_coefficients_and_channel_banks() {
        let state = crate::default_native_device_state(hex_devices::eq8::EQ8_TYPE_ID).unwrap();
        let flat = native_effect_frequency_response(&state, 48_000, &[500.0, 1_000.0, 2_000.0])
            .unwrap()
            .expect("eq8 response");
        assert!(
            flat.iter()
                .flatten()
                .all(|magnitude| (*magnitude - 1.0).abs() < 1e-6)
        );

        let definition = &hex_devices::eq8::EQ8_DEFINITION;
        let mut changed = definition
            .patch_parameter(
                &state,
                hex_devices::eq8::parameter::MODE,
                ParameterValue::Choice(1),
            )
            .unwrap();
        changed = definition
            .patch_parameter(
                &changed,
                hex_devices::eq8::parameter::enabled(1, 3),
                ParameterValue::Bool(true),
            )
            .unwrap();
        changed = definition
            .patch_parameter(
                &changed,
                hex_devices::eq8::parameter::frequency(1, 3),
                ParameterValue::Float(1_000.0),
            )
            .unwrap();
        changed = definition
            .patch_parameter(
                &changed,
                hex_devices::eq8::parameter::gain(1, 3),
                ParameterValue::Float(6.0),
            )
            .unwrap();
        let response =
            native_effect_frequency_response(&changed, 48_000, &[500.0, 1_000.0, 2_000.0])
                .unwrap()
                .expect("eq8 response");
        assert!(response[1][0] > 1.9);
        assert!((response[1][1] - 1.0).abs() < 1e-6);
        assert_eq!(
            native_effect_frequency_response(&changed, 48_000, &[0.0])
                .unwrap_err()
                .0,
            "invalid native response frequency range"
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

    #[test]
    fn instrument_facades_resolve_only_the_instrument_registry() {
        let effects = native_device_catalog_for_class(NativeDeviceClass::Effect).unwrap();
        let instruments = native_device_catalog_for_class(NativeDeviceClass::Instrument).unwrap();
        assert_eq!(effects.len(), 11);
        assert!(effects.iter().any(|entry| entry.name == "rattle"));
        assert!(effects.iter().any(|entry| entry.name == "utility"));
        assert!(effects.iter().any(|entry| entry.name == "eq"));
        assert!(effects.iter().any(|entry| entry.name == "saturator"));
        assert!(effects.iter().any(|entry| entry.name == "compressor"));
        assert!(effects.iter().any(|entry| entry.name == "gate"));
        assert!(effects.iter().any(|entry| entry.name == "auto pan"));
        assert!(effects.iter().any(|entry| entry.name == "reverb"));
        assert!(effects.iter().any(|entry| entry.name == "overdrive"));
        assert!(effects.iter().any(|entry| entry.name == "delay"));
        assert!(effects.iter().any(|entry| entry.name == "limiter"));
        assert_eq!(instruments.len(), 4);
        assert!(instruments.iter().any(|entry| entry.name == "sampler"));
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
            .find(|entry| entry.name == "vector")
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
        assert_eq!(operator_view.name, "vector");
        assert_eq!(operator_view.parameters.len(), 424);
        let enables: Vec<_> = operator_view
            .parameters
            .iter()
            .filter(|parameter| (513..=516).contains(&parameter.descriptor.id.value()))
            .map(|parameter| (parameter.descriptor.id.value(), parameter.value))
            .collect();
        assert_eq!(
            enables,
            vec![
                (513, ParameterValue::Bool(true)),
                (514, ParameterValue::Bool(true)),
                (515, ParameterValue::Bool(true)),
                (516, ParameterValue::Bool(true)),
            ],
            "all four canonical enable facades preserve their true default"
        );
        for parameter in operator_view
            .parameters
            .iter()
            .filter(|parameter| (57..=60).contains(&parameter.descriptor.id.value()))
        {
            assert_eq!(parameter.value, ParameterValue::Choice(0));
        }
        let routing = operator_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id.value() == 61)
            .unwrap();
        assert_eq!(
            routing.value,
            ParameterValue::Choice(1),
            "the product default uses the serial algorithm preset"
        );
        assert_eq!(routing.value, routing.descriptor.default);
        for operator_index in 0..4 {
            let id = hex_lattice_contract::ParameterId::new(74 + operator_index).unwrap();
            let parameter = operator_view
                .parameters
                .iter()
                .find(|parameter| parameter.descriptor.id == id)
                .expect("operator amplitude velocity parameter is exposed");
            assert_eq!(parameter.value, ParameterValue::Float(0.0));
            assert_eq!(
                parameter.descriptor.kind,
                ParameterKind::Float {
                    min: 0.0,
                    max: 1.0,
                    unit: None
                }
            );
        }
        let velocity_id = hex_lattice_contract::ParameterId::new(74).unwrap();
        let patched_operator = crate::patch_native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            &operator_state,
            velocity_id,
            ParameterValue::Float(0.5),
        )
        .expect("generic Operator velocity patch");
        let patched_view =
            native_device_view_for_class(NativeDeviceClass::Instrument, &patched_operator)
                .expect("patched Operator view");
        assert_eq!(
            patched_view
                .parameters
                .iter()
                .find(|parameter| parameter.descriptor.id == velocity_id)
                .expect("patched velocity parameter")
                .value,
            ParameterValue::Float(0.5)
        );
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

#[cfg(test)]
#[path = "native_device_view/semantics_tests.rs"]
mod semantics_tests;
