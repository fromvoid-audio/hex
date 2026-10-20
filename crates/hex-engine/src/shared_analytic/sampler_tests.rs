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
        .execute(hex_project::Command::InsertTrackWithDevice {
            slot: hex_project::TrackDeviceSlot::Instrument,
            before_track: None,
            name: "sampler".into(),
            state: hex_project::default_instrument_state().unwrap(),
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

#[test]
fn actual_project_sampler_context_preparation_retains_concrete_owner_and_reuses_exact_configuration()
 {
    let Some((source, identity, editor)) = canonical_fixture() else {
        return;
    };
    drop(identity);
    let registry = hex_devices::native_instrument_registry().unwrap();
    let preparation = crate::native_device_resources::DeviceResourcePreparation {
        bank: &source,
        max_scope_metadata_bytes: 65_536,
    };
    let first =
        SharedAnalyticContextTable::prepare(editor.project(), &registry, None, Some(preparation))
            .unwrap();
    let second = SharedAnalyticContextTable::prepare(
        editor.project(),
        &registry,
        Some(&first),
        Some(preparation),
    )
    .unwrap();
    assert_eq!(first.owners.len(), 1);
    assert_eq!(second.owners.len(), 1);
    assert_eq!(first.owners[0].id(), second.owners[0].id());
    assert!(Arc::ptr_eq(first.owners[0].arc(), second.owners[0].arc()));
    assert!(first.owners[0].sampler_factory.is_some());
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
    ledger
        .borrow_device_resource_bank_on_control(&source, AudioStorageRole::Caller)
        .unwrap();
    ledger
        .borrow_editor_native_states_on_control(&editor)
        .unwrap();
    resource_storage::visit_table(&first, &mut ledger, AudioStorageRole::Current).unwrap();
    assert!(
        ledger
            .validate_selected_shared_handles_on_control()
            .is_err()
    ); // actual second table's owner field
    resource_storage::visit_table(&second, &mut ledger, AudioStorageRole::Caller).unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert!(ledger.summary().unwrap().require_complete().is_err());
    let state = editor.project().tracks()[0].instrument().unwrap().state();
    let context = hex_lattice::DevicePreparationContext {
        sample_rate: 44_100,
        tuning: first.tuning(),
        resources: first.owners[0].resources.as_ref().unwrap(),
    };
    assert!(registry.prepare_instrument(state, &context).is_err()); // no registry gate bypass
    assert!(SharedAnalyticContextTable::prepare(editor.project(), &registry, None, None).is_err());
}

#[test]
fn actual_arrangement_and_session_sampler_preparation_retains_factory_and_refuses_stale_state() {
    let Some((source, identity, mut editor)) = canonical_fixture() else {
        return;
    };
    let track = identity.track;
    let device = identity.device;
    drop(identity);
    let hex_project::Created::Clip(clip) = editor
        .execute(hex_project::Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(1).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(hex_project::Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::from_quarters(1).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let hex_project::Created::Scene(scene) = editor
        .execute(hex_project::Command::AddScene {
            name: "sampler scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let hex_project::Created::SessionClip(cell) = editor
        .execute(hex_project::Command::CreateSessionMidiClip {
            track,
            scene,
            name: "sampler cell".into(),
            source_length: hex_project::BeatTime::from_quarters(1).unwrap(),
        })
        .unwrap()
    else {
        panic!("cell")
    };
    editor
        .execute(hex_project::Command::AddMidiNote {
            clip: cell,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::from_quarters(1).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let registry = hex_devices::native_instrument_registry().unwrap();
    let contexts = SharedAnalyticContextTable::prepare(
        editor.project(),
        &registry,
        None,
        Some(crate::native_device_resources::DeviceResourcePreparation {
            bank: &source,
            max_scope_metadata_bytes: 65_536,
        }),
    )
    .unwrap();
    let prepared =
        crate::compile::project_with_prepared_audio_sources_and_midi_preview_and_contexts(
            editor.project(),
            &std::collections::BTreeMap::new(),
            1,
            None,
            Some(contexts.clone_on_control()),
            None,
        )
        .unwrap()
        .0;
    let compiled = &prepared.tracks()[0];
    assert!(compiled.mutable_instrument);
    assert_eq!(compiled.instrument_runtime_latency, Some(0));
    let index = contexts.index_for_device(device).unwrap();
    let owner = contexts.owner(index).unwrap();
    let instrument = compiled.instrument.as_ref().unwrap();
    assert!(instrument.mutable);
    // Mutable notes have no analytic samplers; the actual retained table is
    // an independent owning field on InstrumentTrack, not bank metadata.
    assert!(instrument.bank.context_matches(None));
    assert!(Arc::ptr_eq(
        instrument.shared_contexts.arc(),
        contexts.arc()
    ));
    assert!(Arc::ptr_eq(prepared.shared_contexts.arc(), contexts.arc()));
    let session = crate::PreparedSessionInventory::prepare_with_media_and_shared_contexts(
        editor.project(),
        &std::collections::BTreeMap::new(),
        crate::SharedSineTable::new_on_control(),
        None,
        &registry,
        contexts.clone_on_control(),
    )
    .unwrap();
    assert_eq!(session.midi_clips().len(), 1);
    assert!(session.midi_clip(cell).is_some());
    let crate::session_inventory::PreparedTrackRuntimeFactory::Sampler(session_factory) =
        session.mutable_instrument_factory_owner(track).unwrap()
    else {
        panic!("actual retained concrete Session route")
    };
    assert!(Arc::ptr_eq(
        session_factory.arc(),
        owner.sampler_factory.as_ref().unwrap().0.arc()
    ));
    // Exercise PCM from the factory actually retained by the Session producer;
    // this is not a claim of renderer publication or complete graph admission.
    let mut pair = NativeInstrumentRuntimePair::new(
        session_factory.as_ref(),
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
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 127,
        }],
        &mut output,
        owner.resources(),
    )
    .unwrap();
    pair.commit_block();
    assert_eq!(&output[..4], &fixtures::PCM);
    assert_eq!(&output[4..], &[[0.0; 2]; 2]);
    // Ordinary unbound compilation still refuses: this route is preparation,
    // not a registry permit or complete root publication proof.
    assert!(crate::compile::project(editor.project(), &std::collections::BTreeMap::new()).is_err());
    let mut changed = hex_devices::sampler::state::SamplerState::from_native(
        editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .state(),
    )
    .unwrap();
    changed.region.start = 1;
    let previous = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .state();
    let state = changed
        .to_native()
        .unwrap()
        .with_host_metadata_from(previous)
        .unwrap();
    editor
        .execute(hex_project::Command::ReplaceDeviceState { device, state })
        .unwrap();
    assert!(
        crate::compile::project_with_prepared_audio_sources_and_midi_preview_and_contexts(
            editor.project(),
            &std::collections::BTreeMap::new(),
            1,
            None,
            Some(contexts.clone_on_control()),
            None,
        )
        .is_err()
    );
    assert!(
        crate::PreparedSessionInventory::prepare_with_media_and_shared_contexts(
            editor.project(),
            &std::collections::BTreeMap::new(),
            crate::SharedSineTable::new_on_control(),
            None,
            &registry,
            contexts,
        )
        .is_err()
    );
}

#[test]
fn actual_empty_sampler_context_has_no_resource_owner_and_prepares_silent_clip_hosts() {
    use hex_lattice_contract::NativeDeviceMetadata;
    let mut editor = hex_project::Editor::new(Project::new(44_100).unwrap(), 8).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(hex_project::Command::InsertTrackWithDevice {
            name: "empty sampler".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
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
    editor
        .execute(hex_project::Command::ReplaceDeviceState {
            device,
            state: hex_devices::sampler::metadata::METADATA
                .default_state()
                .unwrap(),
        })
        .unwrap();
    let hex_project::Created::Clip(clip) = editor
        .execute(hex_project::Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(1).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(hex_project::Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::from_quarters(1).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let registry = hex_devices::native_instrument_registry().unwrap();
    if !crate::pcm_arc::native_layout_known() {
        assert!(
            SharedAnalyticContextTable::prepare(editor.project(), &registry, None, None).is_err()
        );
        return;
    }
    let contexts =
        SharedAnalyticContextTable::prepare(editor.project(), &registry, None, None).unwrap();
    assert_eq!(contexts.owners.len(), 1);
    let owner = &contexts.owners[0];
    assert!(owner.resources.is_none());
    assert!(owner.state_storage.is_none());
    assert_eq!(owner.identity.resource_scope, 0);
    assert!(owner.resources().is_empty());
    assert!(owner.is_inline_resource_scope());
    let factory = owner.sampler_factory.as_ref().unwrap().0.as_ref();
    assert_eq!(
        hex_lattice::PreparedMutableInstrumentRuntime::max_release_tail_frames(factory),
        0
    );
    if crate::canonical_native_state_storage_layout_supported() {
        eprintln!(
            "executing supported empty Sampler actual canonical-state/context field traversal"
        );
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        ledger
            .borrow_editor_native_states_on_control(&editor)
            .unwrap();
        resource_storage::visit_table(&contexts, &mut ledger, AudioStorageRole::Current).unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert!(ledger.summary().unwrap().require_complete().is_err());
    }
    let compiled =
        crate::compile::project(editor.project(), &std::collections::BTreeMap::new()).unwrap();
    assert!(compiled.tracks()[0].mutable_instrument);
    assert!(compiled.tracks()[0].instrument.as_ref().unwrap().mutable);
    let session = crate::PreparedSessionInventory::prepare_with_media_and_shared_contexts(
        editor.project(),
        &std::collections::BTreeMap::new(),
        crate::SharedSineTable::new_on_control(),
        None,
        &registry,
        contexts.clone_on_control(),
    )
    .unwrap();
    assert!(matches!(
        session.mutable_instrument_factory_owner(track),
        Some(crate::session_inventory::PreparedTrackRuntimeFactory::Sampler(_))
    ));
    let mut pair = NativeInstrumentRuntimePair::new(
        factory,
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: 32,
            max_tail_frames: 44_100 * 600,
        },
    )
    .unwrap();
    let mut output = [[0.125, -0.25]; 128];
    pair.begin_block();
    pair.process_block(
        &[NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 127,
        }],
        &mut output,
        owner.resources(),
    )
    .unwrap();
    pair.commit_block();
    assert_eq!(output, [[0.125, -0.25]; 128]);
    assert!(
        registry
            .definition(hex_devices::sampler::state::TYPE_ID)
            .is_none()
    );
}
