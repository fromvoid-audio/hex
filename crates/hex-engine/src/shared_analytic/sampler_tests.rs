//! Actual concrete factory owners; no registry/publication permission is inferred.
use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole,
    audio_storage::Owner,
    native_device_resources::{DeviceResourceBank, DeviceResourceCatalog, tests as fixtures},
    owned_shared::OwnedShared,
};
use hex_devices::sampler::{
    GateMode, PreparedSamplerRuntime,
    state::{AuthoredMode, SamplerState},
};
use hex_lattice::{
    NativeInstrumentEvent, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
};
use std::sync::atomic::AtomicBool;

fn reservation() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 128,
        root_rows: 16,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}

pub(crate) fn canonical_fixture() -> Option<(
    DeviceResourceBank,
    SharedContextIdentity,
    hex_project::Editor,
)> {
    let ids = fixtures::ids();
    let bank = fixtures::bank(&ids)?;
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &bank,
        &fixtures::specs(&ids),
        fixtures::limits(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let source = DeviceResourceBank::try_new_on_control(
        bank,
        catalog,
        65_536,
        65_536,
        &AtomicBool::new(false),
    )
    .ok()
    .unwrap();
    let mut editor = hex_project::Editor::new(Project::new(44_100).unwrap(), 8).unwrap();
    for (n, expected) in ids.iter().enumerate() {
        let hex_project::Created::Asset(asset) = editor
            .execute(hex_project::Command::AddAsset {
                path: format!("source-{n}.wav").into(),
                sample_rate: 44_100,
                frames: 4,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        assert_eq!(&asset, expected);
    }
    let hex_project::Created::Track(track) = editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let mut authored = SamplerState::for_source_frames(4).unwrap();
    authored.mode = AuthoredMode::OneShot {
        gate: GateMode::Trigger,
    };
    let state = authored
        .to_native()
        .unwrap()
        .with_asset_references(fixtures::state(ids[0]).asset_references().to_vec())
        .unwrap();
    editor
        .execute(hex_project::Command::ReplaceDeviceState {
            device,
            state: state.clone(),
        })
        .unwrap();
    let identity = SharedContextIdentity {
        tuning: TuningIdentity::default(),
        resource_scope: 0,
        track,
        device,
        device_type: state.type_id(),
        state: state.clone(),
        sample_rate: 44_100,
    };
    Some((source, identity, editor))
}

pub(crate) fn retained_fixture()
-> Option<(DeviceResourceBank, SharedContextLease, hex_project::Editor)> {
    let (source, identity, editor) = canonical_fixture()?;
    let configuration = source
        .prepare_lease_on_control(&identity.state, 4096)
        .unwrap()
        .try_into_configuration_on_control(4096)
        .ok()
        .unwrap();
    let owner = SharedContextOwner::from_sampler_resources_on_control(
        SharedContextId::fresh().unwrap(),
        identity,
        configuration,
        &hex_lattice::TuningTable::default(),
        65_536,
        MAX_SHARED_ANALYTIC_CONTEXT_BYTES,
    )
    .unwrap();
    Some((source, OwnedShared::new_owned(owner), editor))
}

pub(crate) fn retained_table_fixture(
    owner: SharedContextLease,
) -> OwnedShared<SharedAnalyticContextTable> {
    OwnedShared::new_owned(SharedAnalyticContextTable {
        ids: vec![(owner.id(), 0)],
        payload_bytes: owner.declared_bytes,
        owners: vec![owner],
        ..Default::default()
    })
}

#[test]
fn concrete_sampler_factory_charges_body_header_and_every_actual_alias() {
    let Some((source, identity, editor)) = canonical_fixture() else {
        return;
    };
    let state = identity.state.clone();
    let resources = || {
        source
            .prepare_lease_on_control(&state, 4096)
            .unwrap()
            .try_into_configuration_on_control(4096)
            .ok()
            .unwrap()
    };
    let (body, header) = OwnedShared::<PreparedSamplerRuntime>::declared_parts().unwrap();
    assert!(body <= MAX_SHARED_ANALYTIC_CONTEXT_BYTES);
    // Reject a concrete body exceeding the remaining global payload before allocating it.
    assert!(
        SharedContextOwner::from_sampler_resources_on_control(
            SharedContextId::fresh().unwrap(),
            identity.clone(),
            resources(),
            &hex_lattice::TuningTable::default(),
            65_536,
            body - 1,
        )
        .is_err()
    );
    assert!(
        SharedContextOwner::from_sampler_resources_on_control(
            SharedContextId::fresh().unwrap(),
            identity.clone(),
            resources(),
            &hex_lattice::TuningTable::default(),
            body + header - 1,
            body,
        )
        .is_err()
    );
    let owner = OwnedShared::new_owned(
        SharedContextOwner::from_sampler_resources_on_control(
            SharedContextId::fresh().unwrap(),
            identity,
            resources(),
            &hex_lattice::TuningTable::default(),
            65_536,
            body,
        )
        .unwrap(),
    );
    assert_eq!(owner.declared_bytes, body);
    let (factory, handle) = owner.sampler_factory.as_ref().unwrap();
    let alias = factory.clone();
    let collect = |reservation| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
        ledger.borrow_device_resource_bank_on_control(&source, AudioStorageRole::Caller)?;
        resource_storage::visit_owner(&owner, &mut ledger, AudioStorageRole::Current)?;
        // An actual extra field must be visited; source identity alone is insufficient.
        assert!(ledger.validate_closed_shared_handles().is_err());
        let before = ledger.summary()?;
        ledger.root(&alias, AudioStorageRole::Archive)?;
        ledger.shared(
            alias.arc(),
            alias.parts(),
            Owner::exclusive(&alias),
            crate::audio_only_inputs::handle()?,
            None,
        )?;
        ledger.validate_closed_shared_handles()?;
        let after = ledger.summary()?;
        assert_eq!(after.data_allocations, before.data_allocations);
        assert_eq!(
            after.data_bytes - before.data_bytes,
            std::mem::size_of_val(&alias)
        );
        Ok::<_, crate::AudioStorageError>(after)
    };
    let summary = collect(reservation()).unwrap();
    assert!(summary.data_bytes >= body + header);
    assert!(summary.require_complete().is_err()); // retirement/custody is a separate proof
    let mut exact = reservation();
    exact.max_retained_bytes = summary.retained_bytes;
    exact.max_construction_peak_bytes = summary.construction_peak_bytes;
    assert_eq!(collect(exact).unwrap(), summary);
    exact.max_retained_bytes -= 1;
    assert!(collect(exact).is_err());
    // The concrete field is included as its own physical allocation. Visiting
    // it independently first adds body+header exactly once, never inline handle twice.
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .shared(
            factory.arc(),
            factory.parts(),
            Owner::exclusive(factory),
            *handle,
            None,
        )
        .unwrap();
    let factory_only = ledger.summary().unwrap();
    assert_eq!(factory_only.data_allocations, 1);
    assert_eq!(factory_only.data_bytes, body + header);
    drop(ledger);
    drop(alias);
    // Both the actual source root and canonical model can retire on control;
    // the owner keeps its exact retained configuration and scalar factory.
    drop(source);
    drop(editor);
    let mut pair = NativeInstrumentRuntimePair::new(
        factory.as_ref(),
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 32,
            max_tail_frames: 44_100 * 600,
        },
    )
    .unwrap();
    let mut output = [[0.0; 2]; 6];
    pair.begin_block();
    pair.process_block(
        &[
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 127,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 1,
                occurrence_id: 1,
            },
        ],
        &mut output,
        owner.resources(),
    )
    .unwrap();
    pair.commit_block();
    assert_eq!(&output[..4], &fixtures::PCM);
    assert_eq!(&output[4..], &[[0.0; 2]; 2]);
}

#[test]
fn actual_sampler_prepared_and_canonical_state_visitors_share_one_physical_charge_in_either_order()
{
    let Some((source, owner, editor)) = retained_fixture() else {
        return;
    };
    if !crate::canonical_native_state_storage_layout_supported() {
        return;
    }
    eprintln!("executing actual sampler prepared/model canonical state joint storage path");
    let collect = |canonical_first| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        ledger
            .borrow_device_resource_bank_on_control(&source, AudioStorageRole::Caller)
            .unwrap();
        ledger
            .borrow_editor_native_states_on_control(&editor)
            .unwrap();
        if canonical_first {
            ledger
                .borrow_canonical_native_state_on_control(&owner.identity.state)
                .unwrap();
        }
        resource_storage::visit_owner(&owner, &mut ledger, AudioStorageRole::Current).unwrap();
        let summary = ledger.summary().unwrap();
        let counts = ledger.selected_shared_counts_on_control();
        ledger
            .borrow_canonical_native_state_on_control(&owner.identity.state)
            .unwrap();
        assert_eq!(ledger.summary().unwrap(), summary);
        assert_eq!(ledger.selected_shared_counts_on_control(), counts);
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert!(summary.require_complete().is_err());
        (summary, counts)
    };
    assert_eq!(collect(false), collect(true));
}
