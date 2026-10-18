//! Registry-discovered bootstrap authoring evidence; device oracles remain separate.
use hex_lattice::conformance::{check_effect, check_instrument, check_metadata};

#[test]
fn registered_default_states_obey_shared_authoring_contract()
-> Result<(), Box<dyn std::error::Error>> {
    for definition in hex_devices::native_registry()?.definitions() {
        let state = definition.default_state()?;
        check_metadata(*definition, &state)?;
        for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            check_effect(*definition, &state, rate)?;
        }
    }
    for definition in hex_devices::native_instrument_registry()?.definitions() {
        let state = definition.default_state()?;
        check_metadata(*definition, &state)?;
        for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            check_instrument(
                *definition,
                &state,
                rate,
                hex_lattice::NativeInstrumentRuntimeLimits {
                    max_pair_bytes: 2 * 1024 * 1024,
                    max_events_per_block: 259,
                    max_active_occurrences: 64,
                    max_tail_frames: u64::from(rate) * 600,
                },
            )?;
        }
    }
    Ok(())
}

#[test]
fn authored_stateful_and_legacy_fixtures_use_the_same_kit() -> Result<(), Box<dyn std::error::Error>>
{
    use hex_lattice_contract::{NativeDeviceState, ParameterValue};
    let registry = hex_devices::native_registry()?;
    let utility = registry
        .definition(hex_devices::utility::UTILITY_TYPE_ID)
        .ok_or("utility missing")?;
    let state = registry.patch_parameter(
        &utility.default_state()?,
        hex_devices::utility::parameter::DC_REMOVE,
        ParameterValue::Bool(true),
    )?;
    check_metadata(utility, &state)?;
    check_effect(utility, &state, 48_000)?;

    let registry = hex_devices::native_instrument_registry()?;
    let vector = registry
        .definition(hex_devices::operator_state::OPERATOR_TYPE_ID)
        .ok_or("vector missing")?;
    let current = vector.default_state()?;
    // v1 is the documented 224-byte prefix; its migration keeps the legacy law.
    let prefix = current
        .payload()
        .get(..224)
        .ok_or("missing legacy prefix")?;
    let legacy = NativeDeviceState::new(current.type_id(), 1, prefix.to_vec())?;
    check_metadata(vector, &legacy)?;
    Ok(())
}
