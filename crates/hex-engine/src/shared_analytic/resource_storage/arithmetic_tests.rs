//! Negative actual owner-visitor path; deliberately invalid stored-state pairing.
//! No loaded device construction or acceptance is inferred from this refusal.
use super::*;
use hex_devices::sampler::state::{AuthoredMode, SamplerState};
use hex_lattice_contract::NativeDeviceMetadata;

#[test]
fn resource_inventory_rejects_loaded_slice_in_empty_owner_without_decode_allocation() {
    assert!(crate::audio_storage_constructor_layout_supported());
    let (_, mut identity, _) = super::super::sampler_tests::canonical_fixture().unwrap();
    identity.state = hex_devices::sampler::metadata::METADATA
        .default_state()
        .unwrap();
    let mut authored = SamplerState::for_source_frames(16).unwrap();
    authored.mode = AuthoredMode::Slice {
        starts: vec![0, 4, 8],
        base_note: 48,
        gate: hex_devices::sampler::GateMode::Gate,
    };
    let loaded = authored.to_native().unwrap();
    let mut bytes = loaded.payload().to_vec();
    let last = bytes.len() - 4;
    bytes[last..].copy_from_slice(&u32::MAX.to_le_bytes());
    let malformed =
        NativeDeviceState::new(loaded.type_id(), loaded.state_version(), bytes).unwrap();
    assert!(hex_devices::sampler::state::DeviceStateRead::from_native(&malformed).is_err());
    for (label, state) in [
        ("loaded Slice in empty owner", loaded),
        ("malformed loaded Slice", malformed),
    ] {
        let mut raw = SharedContextOwner::from_empty_sampler_on_control(
            SharedContextId::fresh().unwrap(),
            identity.clone(),
            &hex_lattice::TuningTable::default(),
            65_536,
            MAX_SHARED_ANALYTIC_CONTEXT_BYTES,
        )
        .unwrap();
        // Fault injection is outside measurement. Actual production visitor must
        // refuse the incompatible stored pairing without decoding an owned Vec.
        raw.identity.state = state;
        let owner = OwnedShared::new_owned(raw);
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(crate::AudioStorageLedgerReservation {
                data_rows: 64,
                handle_rows: 64,
                root_rows: 8,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            })
            .unwrap();
        let before = crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get());
        let mut result = Ok(());
        let measured = allocation_counter::measure(|| {
            result = visit_owner_contents(&owner, &mut ledger);
        });
        assert_eq!(result, Err(AudioStorageError::UnknownStorage));
        assert_eq!(measured.count_total, 0, "{label}: {measured:?}");
        assert_eq!(measured.bytes_total, 0, "{label}: {measured:?}");
        assert_eq!(measured.count_current, 0, "{label}: {measured:?}");
        assert_eq!(measured.bytes_current, 0, "{label}: {measured:?}");
        assert_eq!(
            crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()),
            before
        );
        eprintln!(
            "actual resource visitor refusal {label}: allocator={measured:?}, no ID issuance"
        );
    }
}
