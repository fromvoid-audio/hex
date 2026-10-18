use super::*;
use crate::{
    midi_note_bank::{AnalyticBinding, PreparedMidiNote, SharedContextRef},
    native_device_resources::{
        DeviceResourceBank, DeviceResourceCatalog, PreparedDeviceResourceLease, tests as fixtures,
    },
};
use hex_lattice::{DevicePreparationContext, PreparedAnalyticNote, PreparedSharedAnalyticNote};
use hex_lattice_contract::DeviceContractError;
use std::sync::atomic::AtomicBool;

struct PcmContext(PreparedAnalyticNote);
struct InlinePcm(PreparedAnalyticNote);
impl PreparedNativeInstrument for InlinePcm {
    fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Ok(self.0)
    }
}
impl PreparedSharedAnalyticContext for PcmContext {
    fn declared_context_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<PreparedSharedAnalyticNote, DeviceContractError> {
        Ok(PreparedSharedAnalyticNote::new(
            [0; hex_lattice::SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS],
            0,
        ))
    }
    fn sample(
        &self,
        _: &PreparedSharedAnalyticNote,
        age: u64,
        gate: u64,
        resources: &dyn hex_lattice::DeviceResources,
    ) -> crate::Frame {
        self.0.sample(age, gate, resources)
    }
}
fn owner(
    state: NativeDeviceState,
    lease: PreparedDeviceResourceLease,
    track: TrackId,
    device: DeviceId,
) -> crate::shared_analytic::SharedContextLease {
    let context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &hex_lattice::TuningTable::default(),
        resources: &lease,
    };
    let note = hex_lattice::conformance::resource_pcm::prepare_note(&state, &context, 0).unwrap();
    let context = Box::new(PcmContext(note));
    let bytes = context.declared_context_bytes();
    crate::owned_shared::OwnedShared::new_owned(SharedContextOwner::from_prepared_on_control(
        SharedContextId::fresh().unwrap(),
        SharedContextIdentity {
            tuning: TuningIdentity::default(),
            resource_scope: 0,
            track,
            device,
            device_type: state.type_id(),
            state,
            sample_rate: 48_000,
        },
        bytes,
        context,
        Some(lease.try_into_configuration_on_control(4096).ok().unwrap()),
    ))
}

fn inline_owner(
    state: NativeDeviceState,
    lease: PreparedDeviceResourceLease,
    track: TrackId,
    device: DeviceId,
) -> (crate::shared_analytic::SharedContextLease, InlinePcm) {
    let note = hex_lattice::conformance::resource_pcm::prepare_note(
        &state,
        &DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &lease,
        },
        0,
    )
    .unwrap();
    let owner = crate::owned_shared::OwnedShared::new_owned(
        SharedContextOwner::from_inline_resources_on_control(
            SharedContextId::fresh().unwrap(),
            SharedContextIdentity {
                tuning: TuningIdentity::default(),
                resource_scope: 0,
                track,
                device,
                device_type: state.type_id(),
                state,
                sample_rate: 48_000,
            },
            lease.try_into_configuration_on_control(4096).ok().unwrap(),
            4096,
        )
        .unwrap(),
    );
    (owner, InlinePcm(note))
}

#[test]
fn actual_inline_bank_voices_retain_their_scope_without_enlarging_coefficient_entries() {
    use crate::midi_note_bank::PreparedMidiNoteBank;
    use crate::native_device_resources::retained::RETIRED_RESOURCE_ROOTS;
    let ids = fixtures::ids();
    let Some(bank) = fixtures::bank(&ids) else {
        return;
    };
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &bank,
        &fixtures::specs(&ids),
        fixtures::limits(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let root = DeviceResourceBank::try_new_on_control(
        bank,
        catalog,
        65_536,
        65_536,
        &AtomicBool::new(false),
    )
    .ok()
    .unwrap();
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "inline resources".into(),
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
    let old_state = fixtures::state(ids[0]);
    let target_state = fixtures::state(ids[1]);
    let (old, old_instrument) = inline_owner(
        old_state.clone(),
        root.prepare_lease_on_control(&old_state, 4096).unwrap(),
        track,
        device,
    );
    let (target, target_instrument) = inline_owner(
        target_state.clone(),
        root.prepare_lease_on_control(&target_state, 4096).unwrap(),
        track,
        device,
    );
    assert!(old.analytic_context().is_none());
    assert!(old.require_analytic_context().is_err());
    let mut keys = PreparedMidiNoteBank::empty_keys();
    PreparedMidiNoteBank::add_key(&mut keys, 60, 127).unwrap();
    let old_bank =
        PreparedMidiNoteBank::prepare(&mut keys, &old_instrument, None, Some((0, &old))).unwrap();
    let target_bank =
        PreparedMidiNoteBank::prepare(&mut keys, &target_instrument, None, Some((0, &target)))
            .unwrap();
    let note = old_bank.sampler(old_bank.index(60, 127).unwrap()).unwrap();
    let target_note = target_bank
        .sampler(target_bank.index(60, 127).unwrap())
        .unwrap();
    assert!(matches!(note.note, PreparedMidiNote::Inline(_)));
    assert!(note.inline_without_resources().is_none());
    assert_eq!(note.context_id(), Some(old.id()));
    assert!(note.rebind(0, &target).is_err());
    let carried = note
        .detach(&[])
        .with_context_ref(SharedContextRef::Retained(0));
    drop(old_bank);
    drop(target_bank);
    drop(root);
    let table = Arc::new(SharedAnalyticContextTable {
        owners: vec![target],
        ..Default::default()
    });
    let before = RETIRED_RESOURCE_ROOTS.with(|counter| counter.get());
    let (table, retained) = std::thread::spawn(move || {
        let before = RETIRED_RESOURCE_ROOTS.with(|counter| counter.get());
        let retained = vec![Some(old)];
        for (age, expected) in [[0.25, -0.75], [-0.5, 1.0], [0.75, -0.25], [-1.0, 0.5]]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                carried
                    .sample_at(&table, &retained, &[], 0, age as u64, 4)
                    .unwrap(),
                expected
            );
            assert_eq!(
                target_note
                    .sample_at(&table, &retained, &[], 0, age as u64, 4)
                    .unwrap(),
                [0.5; 2]
            );
        }
        assert!(carried.sample_at(&table, &[], &[], 0, 0, 4).is_err());
        assert!(note.sample_at(&table, &retained, &[], 0, 0, 4).is_err()); // target slot cannot supply old ID
        assert_eq!(RETIRED_RESOURCE_ROOTS.with(|counter| counter.get()), before);
        (table, retained)
    })
    .join()
    .unwrap();
    drop(table);
    assert_eq!(RETIRED_RESOURCE_ROOTS.with(|counter| counter.get()), before);
    drop(retained);
    assert_eq!(
        RETIRED_RESOURCE_ROOTS.with(|counter| counter.get()),
        before + 1
    );
}
#[test]
fn carried_shared_host_reads_its_retained_scope_after_slot_reuse_and_control_release() {
    use crate::native_device_resources::retained::RETIRED_RESOURCE_ROOTS;
    let ids = fixtures::ids();
    let Some(bank) = fixtures::bank(&ids) else {
        return;
    };
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
    let old_state = fixtures::state(ids[0]);
    let new_state = fixtures::state(ids[1]);
    let old_lease = source.prepare_lease_on_control(&old_state, 4096).unwrap();
    let new_lease = source.prepare_lease_on_control(&new_state, 4096).unwrap();
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "resource proof".into(),
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
    let old = owner(old_state, old_lease, track, device);
    let target = owner(new_state, new_lease, track, device);
    let note = PreparedMidiNote::Shared {
        context: SharedContextRef::Revision(0),
        note: old
            .require_analytic_context()
            .unwrap()
            .prepare_note(60, 127)
            .unwrap(),
        id: old.id(),
        binding: AnalyticBinding::Current,
    };
    let carried = note
        .detach(&[])
        .with_context_ref(SharedContextRef::Retained(0));
    let target_note = PreparedMidiNote::Shared {
        context: SharedContextRef::Revision(0),
        note: target
            .require_analytic_context()
            .unwrap()
            .prepare_note(60, 127)
            .unwrap(),
        id: target.id(),
        binding: AnalyticBinding::Current,
    };
    let table = Arc::new(SharedAnalyticContextTable {
        owners: vec![target],
        ..Default::default()
    });
    let drops_before = RETIRED_RESOURCE_ROOTS.with(|count| count.get());
    drop(source); // Caller custody is gone; only actual current/retained host owners remain.
    let (table, retained) = std::thread::spawn(move || {
        let before = RETIRED_RESOURCE_ROOTS.with(|count| count.get());
        let retained = vec![Some(old)];
        let expected = [[0.25, -0.75], [-0.5, 1.0], [0.75, -0.25], [-1.0, 0.5]];
        for (age, expected) in expected.into_iter().enumerate() {
            assert_eq!(
                carried
                    .sample_at(&table, &retained, &[], 0, age as u64, 4)
                    .unwrap(),
                expected
            );
            assert_eq!(
                target_note
                    .sample_at(&table, &retained, &[], 0, age as u64, 4)
                    .unwrap(),
                [0.5; 2]
            );
        }
        // Actual identity check rejects a missing carried owner instead of consulting target slot1.
        assert!(carried.sample_at(&table, &[], &[], 0, 0, 4).is_err());
        assert_eq!(RETIRED_RESOURCE_ROOTS.with(|count| count.get()), before);
        (table, retained)
    })
    .join()
    .unwrap();
    assert_eq!(
        RETIRED_RESOURCE_ROOTS.with(|count| count.get()),
        drops_before
    );
    drop(table);
    assert_eq!(
        RETIRED_RESOURCE_ROOTS.with(|count| count.get()),
        drops_before
    );
    drop(retained);
    assert_eq!(
        RETIRED_RESOURCE_ROOTS.with(|count| count.get()),
        drops_before + 1
    );
}

#[test]
fn unchanged_authored_state_cannot_reuse_a_context_from_another_actual_source_root() {
    let ids = fixtures::ids();
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "root identity".into(),
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
    let make = || {
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
        let state = fixtures::state(ids[0]);
        let lease = source.prepare_lease_on_control(&state, 4096).unwrap();
        Some(owner(state, lease, track, device))
    };
    let Some(old) = make() else { return };
    let target = make().unwrap();
    assert_eq!(old.identity.state, target.identity.state);
    assert_eq!(
        old.resources().get(0).unwrap().identity.content,
        target.resources().get(0).unwrap().identity.content
    );
    assert_ne!(old.identity.resource_scope, target.identity.resource_scope);
    assert!(!old.compatible_identity(&target.identity));
    assert!(target.continue_from(&old, None, 0).unwrap().is_none());
}

#[test]
fn fresh_inline_owner_inventory_separates_canonical_metadata_and_requires_every_actual_alias() {
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
    let ids = fixtures::ids();
    let Some(bank) = fixtures::bank(&ids) else {
        return;
    };
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
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "metadata owner".into(),
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
    let canonical = fixtures::state(ids[0]);
    let canonical_alias = canonical.clone();
    let (current, _) = inline_owner(
        canonical.clone(),
        source.prepare_lease_on_control(&canonical, 4096).unwrap(),
        track,
        device,
    );
    assert_eq!(current.identity.state, canonical);
    assert!(
        !current
            .identity
            .state
            .shares_payload_storage_with(&canonical)
    );
    assert!(
        !current
            .identity
            .state
            .shares_asset_reference_storage_with(&canonical)
    );
    assert!(canonical.shares_payload_storage_with(&canonical_alias));
    assert!(canonical.shares_asset_reference_storage_with(&canonical_alias));
    let state = fixtures::state(ids[1]);
    let (carried, _) = inline_owner(
        state.clone(),
        source.prepare_lease_on_control(&state, 4096).unwrap(),
        track,
        device,
    );
    let archive = current.clone();
    let reservation = AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 64,
        root_rows: 16,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    };
    let collect = |reservation| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
        ledger.borrow_device_resource_bank_on_control(&source, AudioStorageRole::Caller)?;
        resource_storage::visit_owner(&current, &mut ledger, AudioStorageRole::Current)?;
        resource_storage::visit_owner(&carried, &mut ledger, AudioStorageRole::Predecessor)?;
        assert!(ledger.validate_closed_shared_handles().is_err()); // actual archive alias omitted
        let before = ledger.summary()?;
        resource_storage::visit_owner(&archive, &mut ledger, AudioStorageRole::Archive)?;
        ledger.validate_closed_shared_handles()?;
        let summary = ledger.summary()?;
        assert_eq!(
            summary.data_bytes - before.data_bytes,
            std::mem::size_of::<SharedContextLease>()
        );
        assert_eq!(summary.data_allocations, before.data_allocations);
        Ok::<_, crate::AudioStorageError>(summary)
    };
    let summary = collect(reservation).unwrap();
    let mut exact = reservation;
    exact.max_retained_bytes = summary.retained_bytes;
    exact.max_construction_peak_bytes = summary.construction_peak_bytes;
    assert_eq!(collect(exact).unwrap(), summary);
    exact.max_retained_bytes -= 1;
    assert!(collect(exact).is_err());
    assert!(summary.require_complete().is_err());
    let leaked_metadata = current.identity.state.clone();
    assert!(collect(reservation).is_err()); // metadata alias also must remain accounted
    drop(leaked_metadata);
    collect(reservation).unwrap();
    drop(archive);
    drop(source);
    assert_eq!(
        current.resources().get(0).unwrap().pcm.frame(0),
        Some([0.25, -0.75])
    );
    assert_eq!(
        carried.resources().get(0).unwrap().pcm.frame(0),
        Some([0.5; 2])
    );
}

#[test]
fn raw_outer_arc_and_opaque_provider_context_cannot_gain_native_inline_metadata_proof() {
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
    let ids = fixtures::ids();
    let Some(bank) = fixtures::bank(&ids) else {
        return;
    };
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
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "refused metadata".into(),
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
    let state = fixtures::state(ids[0]);
    let opaque = owner(
        state.clone(),
        source.prepare_lease_on_control(&state, 4096).unwrap(),
        track,
        device,
    );
    let untracked = crate::owned_shared::OwnedShared::untracked(Arc::new(
        SharedContextOwner::from_inline_resources_on_control(
            SharedContextId::fresh().unwrap(),
            SharedContextIdentity {
                tuning: TuningIdentity::default(),
                resource_scope: 0,
                track,
                device,
                device_type: state.type_id(),
                state: state.clone(),
                sample_rate: 48_000,
            },
            source
                .prepare_configuration_on_control(&state, 4096)
                .unwrap(),
            4096,
        )
        .unwrap(),
    ));
    for owner in [&opaque, &untracked] {
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                data_rows: 128,
                handle_rows: 64,
                root_rows: 16,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            })
            .unwrap();
        let capacity = ledger.metadata_capacity_bytes();
        assert_eq!(
            ledger.mutate(|ledger| resource_storage::visit_owner(
                owner,
                ledger,
                AudioStorageRole::FailedConstruction
            )),
            Err(crate::AudioStorageError::UnknownStorage)
        );
        assert_eq!(
            ledger.summary(),
            Err(crate::AudioStorageError::UnknownStorage)
        );
        assert_eq!(ledger.metadata_capacity_bytes(), capacity);
    }
    assert_eq!(
        source
            .borrow_original_source_on_control(ids[0], 4096)
            .unwrap()
            .view()
            .unwrap()
            .resident_frames()
            .unwrap()[0],
        [0.25, -0.75]
    );
}

#[test]
fn actual_prepared_context_and_mutable_host_share_one_configuration_body_until_control_retirement()
{
    use crate::mutable_instrument::{
        MAX_MUTABLE_EVENTS_PER_QUERY, MutableInstrumentTrackRuntime, MutableOnsetBuffer,
    };
    use crate::native_device_resources::retained::RETIRED_RESOURCE_ROOTS;
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
    let ids = fixtures::ids();
    let Some(inputs) = fixtures::bank(&ids) else {
        return;
    };
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &inputs,
        &fixtures::specs(&ids),
        fixtures::limits(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let bank = DeviceResourceBank::try_new_on_control(
        inputs,
        catalog,
        65_536,
        65_536,
        &AtomicBool::new(false),
    )
    .ok()
    .unwrap();
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "prepared resource anchor".into(),
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
    let state = fixtures::state(ids[0]);
    let (anchor, _) = inline_owner(
        state.clone(),
        bank.prepare_lease_on_control(&state, 4096).unwrap(),
        track,
        device,
    );
    let configuration = anchor.resource_configuration().unwrap();
    let factory = hex_lattice::conformance::resource_pcm::mutable::prepare(
        &state,
        &DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &hex_lattice::TuningTable::default(),
            resources: configuration,
        },
        0,
    )
    .unwrap();
    let configuration = configuration
        .try_clone_on_control(std::mem::size_of_val(configuration))
        .unwrap();
    let mut host = MutableInstrumentTrackRuntime::new_with_configuration(
        &factory,
        48_000,
        Some(configuration),
    )
    .unwrap();
    let retired = RETIRED_RESOURCE_ROOTS.with(std::cell::Cell::get);
    drop(bank);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 64,
        root_rows: 16,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    })
    .unwrap();
    ledger
        .mutate(|ledger| resource_storage::visit_owner(&anchor, ledger, AudioStorageRole::Archive))
        .unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err());
    let before = ledger.summary().unwrap();
    ledger
        .mutate(|ledger| host.visit_resource_owner_on_control(ledger, AudioStorageRole::Current))
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let after = ledger.summary().unwrap();
    assert_eq!(after.data_allocations, before.data_allocations);
    assert_eq!(
        after.data_bytes - before.data_bytes,
        std::mem::size_of::<crate::native_device_resources::DeviceResourceConfiguration>()
    );
    drop(ledger);
    let mut output = [[0.0; 2]; 2];
    let mut onsets = MutableOnsetBuffer::default();
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    host.begin_block();
    host.process_segment(track, 0, 0, &mut output, &mut onsets, &[], &mut events)
        .unwrap();
    host.commit_block();
    assert_eq!(output, fixtures::PCM[..2]);
    assert_eq!(RETIRED_RESOURCE_ROOTS.with(std::cell::Cell::get), retired);
    drop(anchor);
    host.begin_block();
    output.fill([0.0; 2]);
    host.process_segment(track, 2, 2, &mut output, &mut onsets, &[], &mut events)
        .unwrap();
    host.commit_block();
    assert_eq!(output, fixtures::PCM[2..]);
    assert_eq!(RETIRED_RESOURCE_ROOTS.with(std::cell::Cell::get), retired);
    drop(host); // Calling control thread owns the final actual configuration.
    assert_eq!(
        RETIRED_RESOURCE_ROOTS.with(std::cell::Cell::get),
        retired + 1
    );
}

#[test]
fn actual_prepared_revision_preserves_context_table_receipts_and_both_physical_headers() {
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
    if !crate::pcm_arc::native_layout_known() {
        return;
    }
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "table metadata".into(),
        })
        .unwrap();
    let revision = crate::PreparedRevision::prepare(
        editor.project(),
        &std::collections::BTreeMap::new(),
        crate::SharedSineTable::new_on_control(),
        None,
    )
    .unwrap();
    let arrangement = revision.arrangement().shared_contexts();
    let session = revision.session().shared_contexts();
    assert!(Arc::ptr_eq(arrangement.arc(), session.arc()));
    assert_eq!(Arc::strong_count(arrangement.arc()), 2);
    let parts = arrangement.parts().unwrap();
    assert_eq!(session.parts(), Some(parts));
    assert!(arrangement.owners.is_empty());
    assert!(arrangement.owners.capacity() > 0); // Empty does not mean free.
    assert!(arrangement.mutable_continuations.capacity() > 0);
    let expected = parts.0
        + parts.1
        + 2 * std::mem::size_of_val(arrangement)
        + arrangement.owners.capacity() * std::mem::size_of::<SharedContextLease>()
        + arrangement.ids.capacity() * std::mem::size_of::<(SharedContextId, u16)>()
        + arrangement.mutable_continuations.capacity() * std::mem::size_of::<(TrackId, DeviceId)>();
    let reservation = AudioStorageLedgerReservation {
        data_rows: 64,
        handle_rows: 32,
        root_rows: 16,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    };
    let mut omitted = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
    omitted
        .mutate(|ledger| {
            resource_storage::visit_table(arrangement, ledger, AudioStorageRole::Current)
        })
        .unwrap();
    assert!(omitted.validate_closed_shared_handles().is_err());
    drop(omitted);
    let inventory = |reservation| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
        ledger.borrow_revision_context_tables_on_control(&revision, AudioStorageRole::Current)?;
        ledger.validate_closed_shared_handles()?;
        ledger.summary()
    };
    let summary = inventory(reservation).unwrap();
    assert_eq!(summary.data_bytes, expected);
    assert_eq!(summary.data_allocations, 3); // Body and two spare Vec buffers.
    assert_eq!(summary.unresolved_handles, 2);
    assert_eq!(
        inventory(AudioStorageLedgerReservation {
            max_retained_bytes: summary.retained_bytes,
            max_construction_peak_bytes: summary.construction_peak_bytes,
            ..reservation
        })
        .unwrap(),
        summary
    );
    assert!(
        inventory(AudioStorageLedgerReservation {
            max_retained_bytes: summary.retained_bytes - 1,
            ..reservation
        })
        .is_err()
    );
    assert!(
        inventory(AudioStorageLedgerReservation {
            max_construction_peak_bytes: summary.construction_peak_bytes - 1,
            ..reservation
        })
        .is_err()
    );
    assert!(
        inventory(AudioStorageLedgerReservation {
            handle_rows: 1,
            ..reservation
        })
        .is_err()
    );
    // Borrowing a genuine extra control handle cannot manufacture a new body.
    let archive = arrangement.clone_on_control();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
    ledger
        .borrow_revision_context_tables_on_control(&revision, AudioStorageRole::Current)
        .unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err());
    let before = ledger.summary().unwrap();
    ledger
        .mutate(|ledger| resource_storage::visit_table(&archive, ledger, AudioStorageRole::Archive))
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let after = ledger.summary().unwrap();
    assert_eq!(after.data_allocations, before.data_allocations);
    assert_eq!(
        after.data_bytes - before.data_bytes,
        std::mem::size_of_val(&archive)
    );
}

#[test]
fn raw_context_table_arc_cannot_gain_a_constructor_receipt_from_empty_fields_or_alias_counts() {
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
    let raw = crate::owned_shared::OwnedShared::untracked(Arc::new(
        SharedAnalyticContextTable::default(),
    ));
    let mut ledger = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        data_rows: 64,
        handle_rows: 32,
        root_rows: 16,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    })
    .unwrap();
    assert!(matches!(
        ledger.mutate(|ledger| resource_storage::visit_table(
            &raw,
            ledger,
            AudioStorageRole::Current
        )),
        Err(crate::AudioStorageError::UnknownStorage)
    ));
    assert!(ledger.summary().is_err());
    assert_eq!(Arc::strong_count(raw.arc()), 1);
}

#[test]
fn actual_cached_midi_template_keeps_its_original_table_receipt_across_revision_retirement() {
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
    use hex_project::{BeatTime, Command, Created};
    if !crate::pcm_arc::native_layout_known() {
        return;
    }
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "retained midi table".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "source scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let length = BeatTime::from_quarters(4).unwrap();
    let Created::SessionClip(cell) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "source cell".into(),
            source_length: length,
        })
        .unwrap()
    else {
        panic!("cell")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    for clip in [cell, clip] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(1).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let table = crate::SharedSineTable::new_on_control();
    let media = std::collections::BTreeMap::new();
    let first =
        crate::PreparedRevision::prepare(editor.project(), &media, table.clone(), None).unwrap();
    let second =
        crate::PreparedRevision::prepare(editor.project(), &media, table, Some(first.session()))
            .unwrap();
    assert!(std::ptr::eq(
        first.session().midi_clips()[0].instrument_template(),
        second.session().midi_clips()[0].instrument_template()
    ));
    assert!(!Arc::ptr_eq(
        first.arrangement().shared_contexts().arc(),
        second.arrangement().shared_contexts().arc()
    ));
    let reservation = AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 64,
        root_rows: 32,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    };
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
    ledger
        .borrow_revision_context_tables_on_control(&second, AudioStorageRole::Current)
        .unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err());
    ledger
        .borrow_revision_context_tables_on_control(&first, AudioStorageRole::Predecessor)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let union = ledger.summary().unwrap();
    drop(ledger);
    drop(first);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation).unwrap();
    ledger
        .borrow_revision_context_tables_on_control(&second, AudioStorageRole::Current)
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let retained = ledger.summary().unwrap();
    assert!(retained.data_bytes < union.data_bytes);
    // Both old cached and fresh current table bodies remain, with fewer actual
    // header fields. Retiring the old revision does not copy/recreate its body.
    assert_eq!(retained.data_allocations, union.data_allocations);
}
