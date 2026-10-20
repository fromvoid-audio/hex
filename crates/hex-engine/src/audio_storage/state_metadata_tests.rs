use super::*;
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};
fn state() -> NativeDeviceState {
    NativeDeviceState::new(
        DeviceTypeId::from_static("test.metadata-allocations"),
        1,
        vec![1, 2, 3, 4],
    )
    .unwrap()
    .with_asset_references(vec![DeviceAssetReference {
        slot: DeviceAssetSlot::new(1).unwrap(),
        asset: DeviceAssetKey::new(7).unwrap(),
    }])
    .unwrap()
}
#[test]
fn payload_and_bindings_are_distinct_owner_domains_even_in_the_same_state_value() {
    let state = state();
    let alias = state.clone();
    let equal = self::state();
    let payload = Owner::device_payload(&state);
    let bindings = Owner::device_asset_references(&state);
    let header = Owner::exclusive(&state);
    for (left, right) in [(payload, bindings), (payload, header), (bindings, header)] {
        assert!(!left.same(right));
        assert!(!right.same(left));
    }
    assert!(payload.same(Owner::device_payload(&alias)));
    assert!(bindings.same(Owner::device_asset_references(&alias)));
    assert!(!payload.same(Owner::device_payload(&equal)));
    assert!(!bindings.same(Owner::device_asset_references(&equal)));
    assert_eq!(state.payload_storage_observation(), (4, 2, 0));
    assert_eq!(state.asset_reference_storage_observation(), Some((1, 2, 0)));
}
#[test]
fn payload_handle_cannot_close_a_missing_binding_owner_or_count_the_same_header() {
    let state = state();
    let alias = state.clone();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        data_rows: 8,
        handle_rows: 8,
        root_rows: 2,
        max_retained_bytes: 65_536,
        max_construction_peak_bytes: 65_536,
    })
    .unwrap();
    // Explicit test DATA amounts exercise relation/closure only. These raw
    // declarations do NOT establish a native constructor or storage proof.
    for (index, (owner, location)) in [
        (Owner::device_payload(&state), "payload-test"),
        (Owner::device_asset_references(&state), "bindings-test"),
    ]
    .into_iter()
    .enumerate()
    {
        ledger.bytes(owner, Location(location, 0), 32).unwrap();
        ledger
            .handle_at(
                owner,
                Owner::exclusive(&state),
                Location("state-field", index),
                index as u64 + 1,
            )
            .unwrap();
    }
    assert_eq!(ledger.summary().unwrap().data_bytes, 64);
    assert_eq!(ledger.summary().unwrap().data_allocations, 2);
    assert!(ledger.validate_closed_shared_handles().is_err());
    ledger
        .handle_at(
            Owner::device_payload(&alias),
            Owner::exclusive(&alias),
            Location("state-field", 0),
            3,
        )
        .unwrap();
    // The completed payload relation cannot double-count its handle as one for
    // the different allocation stored in the SAME NativeDeviceState value.
    assert!(ledger.validate_closed_shared_handles().is_err());
    ledger
        .handle_at(
            Owner::device_asset_references(&alias),
            Owner::exclusive(&alias),
            Location("state-field", 1),
            4,
        )
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    assert!(ledger.summary().unwrap().require_complete().is_err());
}

#[test]
fn equal_binding_replacement_retains_payload_but_allocates_a_separate_binding_owner() {
    let state = state();
    let replacement = state
        .clone()
        .with_asset_references(state.asset_references().to_vec())
        .unwrap();
    assert_eq!(state, replacement);
    assert!(Owner::device_payload(&state).same(Owner::device_payload(&replacement)));
    assert!(
        !Owner::device_asset_references(&state).same(Owner::device_asset_references(&replacement))
    );
    assert_eq!(state.payload_storage_observation(), (4, 2, 0));
    assert_eq!(state.asset_reference_storage_observation(), Some((1, 1, 0)));
    assert_eq!(
        replacement.asset_reference_storage_observation(),
        Some((1, 1, 0))
    );
}
#[test]
fn public_empty_session_inventory_does_not_nest_invalid_mutation_or_claim_retirement() {
    let project = hex_project::Project::new(48_000).unwrap();
    let revision = std::sync::Arc::new(
        crate::PreparedRevision::prepare(
            &project,
            &std::collections::BTreeMap::new(),
            crate::SharedSineTable::new_on_control(),
            None,
        )
        .unwrap(),
    );
    let session = crate::SessionPlayback::new(std::sync::Arc::clone(&revision)).unwrap();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        data_rows: 8,
        handle_rows: 8,
        root_rows: 8,
        max_retained_bytes: 65_536,
        max_construction_peak_bytes: 65_536,
    })
    .unwrap();
    ledger
        .borrow_session_inline_resource_owners_on_control(&session, AudioStorageRole::Current)
        .unwrap();
    let summary = ledger.summary().unwrap();
    assert_eq!(summary.data_allocations, 0);
    assert_eq!(summary.data_bytes, 0);
    assert!(summary.require_complete().is_err());
}
