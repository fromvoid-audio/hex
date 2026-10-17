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
) -> Arc<SharedContextOwner> {
    let context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &hex_lattice::TuningTable::default(),
        resources: &lease,
    };
    let note = hex_lattice::conformance::resource_pcm::prepare_note(&state, &context, 0).unwrap();
    let context = Box::new(PcmContext(note));
    let bytes = context.declared_context_bytes();
    Arc::new(SharedContextOwner::from_prepared_on_control(
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
        Some(lease),
    ))
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
        note: old.context().prepare_note(60, 127).unwrap(),
        id: old.id(),
        binding: AnalyticBinding::Current,
    };
    let carried = note
        .detach(&[])
        .with_context_ref(SharedContextRef::Retained(0));
    let target_note = PreparedMidiNote::Shared {
        context: SharedContextRef::Revision(0),
        note: target.context().prepare_note(60, 127).unwrap(),
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
