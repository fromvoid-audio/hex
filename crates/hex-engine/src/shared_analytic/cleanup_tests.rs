use super::*;
use std::sync::atomic::AtomicUsize;

struct OpaqueDrop(Arc<AtomicUsize>);
impl Drop for OpaqueDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
impl PreparedSharedAnalyticContext for OpaqueDrop {
    fn declared_context_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedSharedAnalyticNote, hex_lattice_contract::DeviceContractError>
    {
        Ok(hex_lattice::PreparedSharedAnalyticNote::new(
            [0; hex_lattice::SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS],
            0,
        ))
    }
    fn sample(
        &self,
        _: &hex_lattice::PreparedSharedAnalyticNote,
        _: u64,
        _: u64,
        _: &dyn hex_lattice::DeviceResources,
    ) -> crate::Frame {
        [0.0; 2]
    }
}

#[test]
fn retained_context_drop_preflight_refuses_opaque_without_releasing_any_owner() {
    let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "drop audit".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let device = editor.project().track(track).unwrap().instrument().unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    let owner =
        crate::owned_shared::OwnedShared::new_owned(SharedContextOwner::from_prepared_on_control(
            SharedContextId::fresh().unwrap(),
            SharedContextIdentity {
                tuning: TuningIdentity::default(),
                resource_scope: 0,
                track,
                device: device.id(),
                device_type: device.state().type_id(),
                state: device.state().clone(),
                sample_rate: 48_000,
            },
            std::mem::size_of::<OpaqueDrop>(),
            Box::new(OpaqueDrop(Arc::clone(&drops))),
            None,
        ));
    let table = sampler_tests::retained_table_fixture(owner.clone());
    for _ in 0..128 {
        assert_eq!(
            table.refuse_opaque_control_drop(),
            Err(crate::AudioStorageError::UnknownStorage)
        );
    }
    assert_eq!(table.context_count(), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(table);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(owner);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn retained_sampler_context_drop_preflight_keeps_actual_resource_owners() {
    let Some((source, owner, editor)) = sampler_tests::retained_fixture() else {
        eprintln!("unsupported retained sampler context drop preflight");
        return;
    };
    eprintln!("SUPPORTED retained sampler context drop preflight");
    let table = sampler_tests::retained_table_fixture(owner.clone());
    let count = Arc::strong_count(owner.arc());
    for _ in 0..128 {
        table.refuse_opaque_control_drop().unwrap();
    }
    assert_eq!(Arc::strong_count(owner.arc()), count);
    assert!(owner.resources.is_some());
    assert_eq!(table.context_count(), 1);
    drop(table);
    assert!(owner.resources.is_some());
    drop((owner, source, editor));
}
