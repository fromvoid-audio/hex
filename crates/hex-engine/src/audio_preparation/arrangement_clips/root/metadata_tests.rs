//! Selected actual metadata ownership, not complete compiler/native admission.
use super::*;
use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
use hex_project::{Command, Created};
use std::collections::BTreeMap;

fn limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 128,
        handle_rows: 128,
        root_rows: 16,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}

#[test]
fn fresh_arrangement_metadata_moves_actual_backings_and_latches_abnormal_disposal() {
    let Some((bank, _, mut editor)) = crate::shared_analytic::sampler_tests::canonical_fixture()
    else {
        eprintln!("unsupported actual fresh Arrangement metadata");
        return;
    };
    eprintln!("SUPPORTED actual fresh Arrangement timing/routing metadata");
    let source = editor.project().tracks()[0].id();
    let Created::Track(destination) = editor
        .execute(Command::AddMidiTrack {
            name: "destination".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::SetMidiSource {
            destination,
            source: Some(source),
        })
        .unwrap();
    let project_tempo = editor.project().tempo_map().points().as_ptr();
    let project_time = editor
        .project()
        .tempo_map()
        .beat_to_frame(hex_project::BeatTime::ONE)
        .unwrap();
    let prepare = || {
        let revision = crate::PreparedRevision::prepare_with_audio_transforms_and_device_resources(
            editor.project(),
            &BTreeMap::new(),
            crate::SharedSineTable::new_on_control(),
            None,
            &crate::audio_transform::AudioTransformPreparation::default(),
            crate::native_device_resources::DeviceResourcePreparation {
                bank: &bank,
                max_scope_metadata_bytes: 65_536,
            },
        )
        .unwrap();
        // Fixture-only extraction of the actual fresh OWNED value. No raw Arc
        // is adopted or assigned a receipt by the production metadata helpers.
        let arrangement = std::sync::Arc::clone(revision.arrangement());
        drop(revision);
        let Ok(value) = std::sync::Arc::try_unwrap(arrangement) else {
            panic!("fresh owned Arrangement")
        };
        value
    };
    let mut project = prepare();
    assert!(project.signal_graph.is_none());
    assert_eq!(project.midi_routing, [(source, destination)]);
    project.midi_routing.try_reserve_exact(7).unwrap();
    let layout = project
        .tempo_map
        .as_ref()
        .unwrap()
        .owned_allocation_layout();
    let tempo_bytes = layout.point_capacity() * size_of::<hex_project::TempoPoint>()
        + layout.prefix_capacity() * size_of::<u128>();
    let routing_bytes =
        project.midi_routing.capacity() * size_of::<(hex_project::TrackId, hex_project::TrackId)>();
    let tempo_pointer = project.tempo_map.as_ref().unwrap().points().as_ptr();
    let route_pointer = project.midi_routing.as_ptr();
    assert_ne!(tempo_pointer, project_tempo);
    let mut root = ArrangementRootAssembly {
        project: Some(project),
        ..Default::default()
    };
    let aggregate = root.capacity(false).unwrap();
    assert_eq!(aggregate.tempo_backing_bytes, tempo_bytes);
    assert_eq!(aggregate.midi_routing_backing_bytes, routing_bytes);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    ledger.root(&root, AudioStorageRole::Current).unwrap();
    let before = ledger.summary().unwrap();
    root.visit_audio_storage(&mut ledger, None).unwrap();
    let after = ledger.summary().unwrap();
    assert_eq!(
        after.data_bytes - before.data_bytes,
        aggregate.bytes().unwrap()
    );
    root.visit_audio_storage(&mut ledger, None).unwrap();
    assert_eq!(ledger.summary().unwrap(), after);
    drop(ledger);
    // Other products stay real and separately rooted. Their nonempty/native
    // acceptance is deliberately NOT inferred from this metadata-only move.
    let mut project = root.project.take().unwrap();
    assert!(root.can_restore_project());
    root.install_restored_metadata(
        project.tempo_map.take(),
        mem::take(&mut project.midi_routing),
    );
    assert_eq!(
        root.tempo_map.as_ref().unwrap().points().as_ptr(),
        tempo_pointer
    );
    assert_eq!(root.midi_routing.as_ptr(), route_pointer);
    let collect = |budget| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
        ledger.root(&root, AudioStorageRole::Current)?;
        let before = ledger.summary()?;
        root.visit_audio_storage(&mut ledger, None)?;
        let summary = ledger.summary()?;
        assert_eq!(
            summary.data_bytes - before.data_bytes,
            tempo_bytes + routing_bytes
        );
        assert_eq!(summary.data_allocations - before.data_allocations, 3);
        Ok::<_, AudioStorageError>(summary)
    };
    let summary = collect(limits()).unwrap();
    let mut exact = limits();
    exact.max_retained_bytes = summary.retained_bytes;
    exact.max_construction_peak_bytes = summary.construction_peak_bytes;
    assert_eq!(collect(exact).unwrap(), summary);
    exact.max_retained_bytes -= 1;
    assert_eq!(collect(exact), Err(AudioStorageError::ReservationExceeded));
    root.midi_routing.clear();
    assert_eq!(
        root.capacity(false).unwrap().midi_routing_backing_bytes,
        routing_bytes
    );
    assert!(root.has_metadata_disposal()); // Empty reserved backing still needs retirement.
    root.dispose_metadata_on_control().unwrap();
    assert!(!root.has_metadata_disposal());
    assert_eq!(root.capacity(false).unwrap().bytes().unwrap(), 0);
    assert_eq!(
        editor.project().tempo_map().points().as_ptr(),
        project_tempo
    );
    assert_eq!(
        editor
            .project()
            .tempo_map()
            .beat_to_frame(hex_project::BeatTime::ONE)
            .unwrap(),
        project_time
    );
    assert!(crate::audio_preparation::validate_constructor_project(editor.project()).is_err());
    let mut other = prepare();
    let mut custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
    custody
        .arrangement_clips
        .root
        .install_restored_metadata(other.tempo_map.take(), mem::take(&mut other.midi_routing));
    let route_capacity = custody.arrangement_clips.root.midi_routing.capacity();
    custody.arrangement_clips.root.panic_after_metadata_take = true;
    assert!(!custody.dispose_arrangement_clips_on_control());
    assert_eq!(
        custody.phase,
        crate::AudioPreparationPhase::FailedIncomplete
    );
    assert!(custody.first_panic.is_some());
    let failed = &mut custody.arrangement_clips.root;
    assert!(failed.metadata_drop_pending);
    assert!(failed.tempo_map.is_none());
    assert_eq!(failed.midi_routing.capacity(), route_capacity);
    assert!(matches!(
        failed.capacity(false),
        Err(AudioStorageError::UnknownStorage)
    ));
    assert_eq!(
        failed.dispose_metadata_on_control(),
        Err(AudioStorageError::UnknownStorage)
    );
    assert!(failed.reserve(editor.project()).is_err());
    assert_eq!(failed.midi_routing.capacity(), route_capacity);
    drop((project, other)); // Outside selected metadata retirement, bank retained.
}
