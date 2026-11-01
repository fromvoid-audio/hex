//! Historical saved device states keep opening (Pulse #3956). Payload layouts
//! come from the version-bump commits named beside each device's state version:
//! compressor d236adc6e (v1) and 1ce92988b (v2), auto pan 65e935227 (v1),
//! sampler 57f122b59 (v1).
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceContractError, DeviceTypeId,
    NativeDeviceMetadata, NativeDeviceState, ParameterValue,
};

fn floats(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}
fn state(type_id: &'static str, version: u32, payload: Vec<u8>) -> NativeDeviceState {
    NativeDeviceState::new(DeviceTypeId::from_static(type_id), version, payload).unwrap()
}
/// Migrate through the real registry (the engine's path on reopen), then check
/// the result decodes and migrates again unchanged.
fn migrate(old: &NativeDeviceState) -> NativeDeviceState {
    let registry = crate::native_registry().unwrap();
    let migrated = registry.migrate_state(old).unwrap();
    assert_eq!(
        registry.migrate_state(&migrated).unwrap(),
        migrated,
        "idempotent"
    );
    migrated
}

#[test]
fn compressor_v1_and_v2_states_migrate_with_their_original_behaviour() {
    use crate::compressor::{COMPRESSOR_DEFINITION as D, COMPRESSOR_STATE_VERSION, parameter};
    let read = |state: &NativeDeviceState, id| D.read_parameter(state, id).unwrap();
    let v1 = state(
        "hex.compressor",
        1,
        floats(&[-24.0, 4.0, 10.0, 120.0, 6.0, 2.0]),
    );
    let migrated = migrate(&v1);
    assert_eq!(migrated.state_version(), COMPRESSOR_STATE_VERSION);
    assert_eq!(
        read(&migrated, parameter::THRESHOLD),
        ParameterValue::Float(-24.0)
    );
    assert_eq!(
        read(&migrated, parameter::MAKEUP),
        ParameterValue::Float(2.0)
    );
    assert_eq!(
        read(&migrated, parameter::DRY_WET),
        ParameterValue::Float(1.0)
    );
    assert_eq!(
        read(&migrated, parameter::DETECTOR),
        ParameterValue::Choice(0)
    );
    let v2 = state(
        "hex.compressor",
        2,
        floats(&[-24.0, 4.0, 10.0, 120.0, 6.0, 2.0, 0.25]),
    );
    let migrated = migrate(&v2);
    assert_eq!(
        read(&migrated, parameter::DRY_WET),
        ParameterValue::Float(0.25)
    );
    assert_eq!(
        read(&migrated, parameter::DETECTOR),
        ParameterValue::Choice(0)
    );
    // Each version keeps its exact length; newer versions refuse.
    for (version, length) in [(1, 28), (2, 24), (2, 32), (3, 28)] {
        let malformed = state("hex.compressor", version, vec![0; length]);
        assert_eq!(
            D.migrate_state(&malformed),
            Err(DeviceContractError::MalformedState)
        );
    }
    let newer = state("hex.compressor", 4, floats(&[0.0; 9]));
    assert_eq!(
        D.migrate_state(&newer),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
}

#[test]
fn auto_pan_v1_state_migrates_to_a_sine_waveform() {
    use crate::auto_pan::{AUTO_PAN_DEFINITION as D, AUTO_PAN_STATE_VERSION, parameter};
    let v1 = state("hex.auto-pan", 1, floats(&[0.75, 2.0, 90.0]));
    let migrated = migrate(&v1);
    assert_eq!(migrated.state_version(), AUTO_PAN_STATE_VERSION);
    assert_eq!(
        D.read_parameter(&migrated, parameter::AMOUNT).unwrap(),
        ParameterValue::Float(0.75)
    );
    assert_eq!(
        D.read_parameter(&migrated, parameter::WAVEFORM).unwrap(),
        ParameterValue::Choice(0)
    );
    assert_eq!(
        D.migrate_state(&state("hex.auto-pan", 1, vec![0; 16])),
        Err(DeviceContractError::MalformedState)
    );
    assert_eq!(
        D.migrate_state(&state("hex.auto-pan", 3, floats(&[0.0; 4]))),
        Err(DeviceContractError::UnsupportedStateVersion)
    );
}

#[test]
fn sampler_v1_states_migrate_keeping_loop_behaviour_gains_and_source() {
    use crate::sampler::Region;
    use crate::sampler::state::{AuthoredMode, STATE_VERSION, SamplerState};
    let source = vec![DeviceAssetReference {
        slot: DeviceAssetSlot::new(crate::sampler::state::SOURCE_SLOT).unwrap(),
        asset: DeviceAssetKey::new(9).unwrap(),
    }];
    let authored = |loop_enabled| {
        let mut state = SamplerState::for_source_frames(16).unwrap();
        state.region = Region { start: 2, end: 14 };
        state.mode = AuthoredMode::Classic {
            loop_enabled,
            loop_region: Region { start: 3, end: 11 },
        };
        state
    };
    // v1 kept loop coordinates only while looping; off stored (0, 0).
    let v1 = |loop_enabled: bool| {
        let mut payload = authored(loop_enabled)
            .to_native()
            .unwrap()
            .payload()
            .to_vec();
        if !loop_enabled {
            payload[16..24].fill(0);
        }
        NativeDeviceState::new(crate::sampler::state::TYPE_ID, 1, payload)
            .unwrap()
            .with_asset_references(source.clone())
            .unwrap()
            .with_output_gain_millidb(-1500)
            .unwrap()
    };
    let definition = &crate::sampler::catalog::DEFINITION;
    for loop_enabled in [true, false] {
        let old = v1(loop_enabled);
        let migrated = definition
            .migrate_state(&old)
            .unwrap()
            .with_host_metadata_from(&old)
            .unwrap();
        assert_eq!(migrated.state_version(), STATE_VERSION);
        assert_eq!(migrated.asset_references(), old.asset_references());
        assert_eq!(migrated.output_gain_millidb(), -1500);
        let mut expected = authored(loop_enabled);
        if !loop_enabled {
            // Off stays off; the dormant loop covers the playback region.
            expected.mode = AuthoredMode::Classic {
                loop_enabled: false,
                loop_region: Region { start: 2, end: 14 },
            };
        }
        assert_eq!(SamplerState::from_native(&migrated).unwrap(), expected);
        assert_eq!(
            definition.migrate_state(&migrated).unwrap(),
            migrated,
            "idempotent"
        );
    }
    // A v1 classic-off state with loop coordinates was malformed and stays so.
    let mut payload = authored(false).to_native().unwrap().payload().to_vec();
    payload[16] = 3;
    let malformed = NativeDeviceState::new(crate::sampler::state::TYPE_ID, 1, payload).unwrap();
    assert!(SamplerState::from_native(&malformed).is_err());
    let newer =
        NativeDeviceState::new(crate::sampler::state::TYPE_ID, 3, b"empty".to_vec()).unwrap();
    assert!(definition.migrate_state(&newer).is_err());
}
