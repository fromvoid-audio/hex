//! Fixed Copy display/control facts from the concrete borrowed Sampler codec.
//! This reads authored state; it neither registers nor prepares an instrument.
use super::NativeDeviceParameterView;
use hex_lattice_contract::{DeviceContractError as Error, NativeDeviceMetadata, NativeDeviceState};

/// Copy the existing 43 authored controls without a state clone, migration,
/// descriptor Vec, PCM/resource lookup or registry construction. The caller
/// must finish its snapshot read before rendering or invoking a UI callback.
pub fn sampler_parameters_on_control(
    state: &NativeDeviceState,
) -> Result<[NativeDeviceParameterView; 43], Error> {
    use hex_devices::sampler::{metadata::METADATA, state::DeviceStateRead};
    DeviceStateRead::from_native(state)?;
    let descriptors = METADATA.descriptors();
    let Some(first) = descriptors.first() else {
        return Err(Error::MalformedState);
    };
    if descriptors.len() != 43 {
        return Err(Error::MalformedState);
    }
    let mut values = [NativeDeviceParameterView {
        descriptor: *first,
        semantics: None,
        value: first.default,
        enabled: false,
    }; 43];
    for (value, descriptor) in values.iter_mut().zip(descriptors) {
        let current = METADATA.read_parameter(state, descriptor.id)?;
        descriptor.validate(current)?;
        let semantics = METADATA.parameter_semantics_for_state(state, descriptor.id)?;
        if let Some(semantics) = semantics {
            semantics.validate(*descriptor)?;
        }
        *value = NativeDeviceParameterView {
            descriptor: *descriptor,
            semantics,
            value: current,
            enabled: METADATA.parameter_enabled(state, descriptor.id)?,
        };
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_devices::sampler::{
        GateMode, Region,
        metadata::{METADATA, parameter},
        state::{AuthoredMode, DeviceState, SamplerState},
    };
    use hex_lattice_contract::ParameterValue;

    #[test]
    fn sampler_scalar_query_preserves_actual_mode_values_without_allocating() {
        for mode in [
            AuthoredMode::Classic {
                loop_region: Some(Region { start: 2, end: 12 }),
            },
            AuthoredMode::OneShot {
                gate: GateMode::Gate,
            },
            AuthoredMode::Slice {
                starts: vec![0, 4, 9],
                base_note: 60,
                gate: GateMode::Trigger,
            },
        ] {
            let mut authored = SamplerState::for_source_frames(16).unwrap();
            authored.mode = mode;
            authored.reverse = true;
            let native = authored.to_native().unwrap();
            let measured = allocation_counter::measure(|| {
                let views = sampler_parameters_on_control(&native).unwrap();
                for value in views {
                    assert_eq!(
                        value.value,
                        METADATA
                            .read_parameter(&native, value.descriptor.id)
                            .unwrap()
                    );
                    assert_eq!(
                        value.enabled,
                        METADATA
                            .parameter_enabled(&native, value.descriptor.id)
                            .unwrap()
                    );
                }
            });
            assert_eq!((measured.count_total, measured.count_current), (0, 0));
            let views = sampler_parameters_on_control(&native).unwrap();
            assert_eq!(
                views
                    .iter()
                    .find(|v| v.descriptor.id == parameter::REVERSE)
                    .unwrap()
                    .value,
                ParameterValue::Bool(true)
            );
        }
    }

    #[test]
    fn sampler_empty_is_disabled_and_foreign_or_malformed_input_refuses() {
        let empty = DeviceState::Empty.to_native().unwrap();
        let values = sampler_parameters_on_control(&empty).unwrap();
        assert!(
            values
                .iter()
                .all(|p| !p.enabled && p.value == p.descriptor.default)
        );
        let foreign = NativeDeviceState::new(
            hex_lattice_contract::DeviceTypeId::from_static("hex.foreign"),
            1,
            vec![],
        )
        .unwrap();
        assert!(matches!(
            sampler_parameters_on_control(&foreign),
            Err(Error::WrongDeviceType)
        ));
        let malformed = NativeDeviceState::new(
            hex_devices::sampler::state::TYPE_ID,
            hex_devices::sampler::state::STATE_VERSION,
            vec![1],
        )
        .unwrap();
        assert!(sampler_parameters_on_control(&malformed).is_err());
    }
}
