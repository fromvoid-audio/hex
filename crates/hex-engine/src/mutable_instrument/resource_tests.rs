use super::*;
use crate::native_device_resources::{
    DeviceResourceBank, DeviceResourceCatalog, tests as fixtures,
};
use std::sync::atomic::AtomicBool;

fn source(ids: &[hex_project::AssetId]) -> Option<DeviceResourceBank> {
    let bank = fixtures::bank(ids)?;
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &bank,
        &fixtures::specs(ids),
        fixtures::limits(),
        &AtomicBool::new(false),
    )
    .unwrap();
    Some(
        DeviceResourceBank::try_new_on_control(
            bank,
            catalog,
            65_536,
            65_536,
            &AtomicBool::new(false),
        )
        .ok()
        .unwrap(),
    )
}
fn track() -> TrackId {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "mutable resource proof".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    track
}
#[test]
fn actual_mutable_host_keeps_its_scope_through_rollback_checkpoint_and_tail_move() {
    let ids = fixtures::ids();
    let Some(source) = source(&ids) else { return };
    let state = fixtures::state(ids[0]);
    let lease = source.prepare_lease_on_control(&state, 4096).unwrap();
    let factory = hex_lattice::conformance::resource_pcm::mutable::prepare(
        &state,
        &hex_lattice::DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &lease,
        },
        0,
    )
    .unwrap();
    let mut runtime = MutableInstrumentTrackRuntime::new(&factory, 48_000, Some(lease)).unwrap();
    let resources = runtime.clone_resources_on_control().unwrap();
    let mut snapshot =
        MutableInstrumentTrackRuntime::new_with_configuration(&factory, 48_000, resources).unwrap();
    assert!(snapshot.can_inherit_from(&runtime));
    // These are the actual runtime fields, not detached test aliases. Missing
    // either owner must leave the shared configuration body unclosed.
    let mut ledger =
        crate::AudioRootStorageLedger::try_new_on_control(crate::AudioStorageLedgerReservation {
            data_rows: 64,
            handle_rows: 32,
            root_rows: 8,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        })
        .unwrap();
    ledger
        .borrow_device_resource_bank_on_control(&source, crate::AudioStorageRole::Caller)
        .unwrap();
    ledger
        .borrow_device_resource_configuration_on_control(
            runtime.resources.as_ref().unwrap(),
            crate::AudioStorageRole::Current,
        )
        .unwrap();
    assert!(ledger.validate_closed_shared_handles().is_err());
    let before = ledger.summary().unwrap();
    ledger
        .borrow_device_resource_configuration_on_control(
            snapshot.resources.as_ref().unwrap(),
            crate::AudioStorageRole::Archive,
        )
        .unwrap();
    ledger.validate_closed_shared_handles().unwrap();
    let after = ledger.summary().unwrap();
    assert_eq!(after.data_allocations, before.data_allocations);
    assert_eq!(
        after.data_bytes - before.data_bytes,
        std::mem::size_of::<crate::native_device_resources::DeviceResourceConfiguration>()
    );
    drop(ledger);
    let track = track();
    let mut onsets = MutableOnsetBuffer::default();
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    let mut first = [[0.0; 2]; 2];
    runtime.begin_block();
    runtime
        .process_segment(track, 0, 0, &mut first, &mut onsets, &[], &mut events)
        .unwrap();
    assert_eq!(first, [[0.25, -0.75], [-0.5, 1.0]]);
    runtime.rollback_block();
    let mut retry = [[0.0; 2]; 2];
    runtime.begin_block();
    runtime
        .process_segment(track, 0, 0, &mut retry, &mut onsets, &[], &mut events)
        .unwrap();
    assert_eq!(retry, first);
    runtime.commit_block();
    snapshot.inherit_from(&runtime).unwrap();
    let mut tail = MutableInstrumentTail {
        track_id: track,
        runtime: snapshot,
    };
    drop(source);
    drop(runtime); // Only the actual tail owner now retains the bank.
    tail.runtime.begin_block();
    let mut remaining = [[0.0; 2]; 2];
    tail.runtime
        .process_segment(track, 2, 2, &mut remaining, &mut onsets, &[], &mut events)
        .unwrap();
    assert_eq!(remaining, [[0.75, -0.25], [-1.0, 0.5]]);
    tail.runtime.commit_block();
    assert!(tail.runtime.committed_is_quiescent());
    tail.runtime.reset();
    tail.runtime.begin_block();
    let mut reset = [[0.0; 2]; 1];
    tail.runtime
        .process_segment(track, 0, 0, &mut reset, &mut onsets, &[], &mut events)
        .unwrap();
    assert_eq!(reset, [[0.25, -0.75]]);
}

#[test]
fn mutable_host_refuses_foreign_scope_history_and_wrong_pcm_before_mutating_output() {
    let ids = fixtures::ids();
    let Some(source) = source(&ids) else { return };
    let other_source = self::source(&ids).unwrap();
    let state = fixtures::state(ids[0]);
    let lease = source.prepare_lease_on_control(&state, 4096).unwrap();
    let factory = hex_lattice::conformance::resource_pcm::mutable::prepare(
        &state,
        &hex_lattice::DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &lease,
        },
        0,
    )
    .unwrap();
    let runtime = MutableInstrumentTrackRuntime::new(&factory, 48_000, Some(lease)).unwrap();
    let other_lease = other_source.prepare_lease_on_control(&state, 4096).unwrap();
    let mut foreign =
        MutableInstrumentTrackRuntime::new(&factory, 48_000, Some(other_lease)).unwrap();
    assert!(foreign.pair.can_inherit_from(&runtime.pair)); // Equal PCM/key alone is insufficient.
    assert!(!foreign.can_inherit_from(&runtime));
    assert!(foreign.inherit_from(&runtime).is_err());
    let wrong = source
        .prepare_lease_on_control(&fixtures::state(ids[1]), 4096)
        .unwrap();
    let mut wrong = MutableInstrumentTrackRuntime::new(&factory, 48_000, Some(wrong)).unwrap();
    let mut onsets = MutableOnsetBuffer::default();
    let mut events = Vec::with_capacity(MAX_MUTABLE_EVENTS_PER_QUERY);
    let mut output = [[0.125, -0.25]; 2];
    wrong.begin_block();
    let result = wrong.process_segment(track(), 0, 0, &mut output, &mut onsets, &[], &mut events);
    assert!(matches!(
        result,
        Err(MutableInstrumentRuntimeError::Runtime(
            NativeInstrumentRuntimeError::ResourceUnavailable
        ))
    ));
    assert_eq!(output, [[0.125, -0.25]; 2]);
    wrong.rollback_block();
}
