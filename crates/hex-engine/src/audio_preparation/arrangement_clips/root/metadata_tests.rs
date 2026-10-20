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
        .execute(Command::InsertTrackWithDevice {
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            name: "destination".into(),
            state: hex_project::default_instrument_state().unwrap(),
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
    assert!(!project.signal_graph.as_ref().unwrap().needs_runtime());
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

#[test]
fn paired_signal_topology_retains_actual_compiler_runtime_aliases_and_control_destination() {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported paired signal topology");
        return;
    }
    eprintln!("SUPPORTED paired signal topology actual compiler/runtime/control fields");
    let fixture = crate::signal_graph::tests::fixture(&[&[], &[7]], 64);
    let revision = crate::signal_graph::tests::prepare(&fixture).unwrap();
    let arrangement = revision.arrangement();
    let graph = arrangement.signal_graph.as_ref().unwrap();
    let runtime = crate::RendererRuntime::new(arrangement);
    let runtime_graph = &runtime.compensation.as_ref().unwrap().graph;
    let delay_graph = runtime
        .compensation
        .as_ref()
        .unwrap()
        .retained_delay_graph_for_test();
    let pointer = std::sync::Arc::as_ptr(graph.arc());
    assert_eq!(std::sync::Arc::as_ptr(runtime_graph.arc()), pointer);
    let mut root = ArrangementRootAssembly {
        topology: Some(graph.clone_on_control()),
        ..Default::default()
    };
    let (body, header) = graph.parts().unwrap();
    assert_eq!(body, size_of::<crate::signal_graph::SignalGraph>());
    let expected = graph.table_body_bytes().unwrap() + header;
    assert_eq!(
        root.capacity(false).unwrap().signal_topology_bytes,
        expected
    );
    let collect = |root: &ArrangementRootAssembly, budget, omit: u8| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(budget)?;
        ledger.root(root, AudioStorageRole::PendingReclaim)?;
        let before = ledger.summary()?;
        if omit != 1 {
            root.visit_audio_storage(&mut ledger, None)?;
        }
        if omit != 2 {
            crate::signal_graph::storage::visit(graph, &mut ledger)?;
        }
        if omit != 3 {
            crate::signal_graph::storage::visit(runtime_graph, &mut ledger)?;
        }
        if omit != 4 {
            crate::signal_graph::storage::visit(delay_graph, &mut ledger)?;
        }
        ledger.validate_selected_shared_handles_on_control()?;
        let after = ledger.summary()?;
        assert_eq!(after.data_bytes - before.data_bytes, expected);
        assert_eq!(ledger.selected_shared_counts_on_control(), (1, 4));
        root.visit_audio_storage(&mut ledger, None)?;
        crate::signal_graph::storage::visit(graph, &mut ledger)?;
        assert_eq!(ledger.summary()?, after);
        Ok::<_, AudioStorageError>(after)
    };
    let summary = collect(&root, limits(), 0).unwrap();
    for omitted in 1..=4 {
        assert!(collect(&root, limits(), omitted).is_err());
    }
    let mut exact = limits();
    exact.max_retained_bytes = summary.retained_bytes;
    exact.max_construction_peak_bytes = summary.construction_peak_bytes;
    assert_eq!(collect(&root, exact, 0).unwrap(), summary);
    exact.max_retained_bytes -= 1;
    assert_eq!(
        collect(&root, exact, 0),
        Err(AudioStorageError::ReservationExceeded)
    );
    // A complete compiler result and its real runtime remain separately alive.
    // The control destination releases only its actual paired handle.
    root.dispose_metadata_on_control().unwrap();
    assert!(!root.has_metadata_disposal());
    assert_eq!(root.capacity(false).unwrap().signal_topology_bytes, 0);
    assert_eq!(std::sync::Arc::as_ptr(graph.arc()), pointer);
    assert_eq!(std::sync::Arc::strong_count(graph.arc()), 3);
    drop(runtime);
    assert_eq!(std::sync::Arc::strong_count(graph.arc()), 1);
    assert_eq!(graph.main_latency(), 7);
    // Unique/equal raw Arc ingress never receives a constructor receipt.
    let mut raw = ArrangementRootAssembly {
        topology: Some(OwnedShared::untracked(std::sync::Arc::clone(graph.arc()))),
        ..Default::default()
    };
    assert!(raw.capacity(false).is_err());
    assert_eq!(
        raw.dispose_metadata_on_control(),
        Err(AudioStorageError::UnsupportedConstructor)
    );
    assert_eq!(
        std::sync::Arc::as_ptr(raw.topology.as_ref().unwrap().arc()),
        pointer
    );
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    assert_eq!(
        ledger.mutate(|ledger| crate::signal_graph::storage::visit(
            raw.topology.as_ref().unwrap(),
            ledger
        )),
        Err(AudioStorageError::UnknownStorage)
    );
    assert_eq!(ledger.summary(), Err(AudioStorageError::UnknownStorage));
    drop(ledger);
    drop(raw);
    assert_eq!(std::sync::Arc::strong_count(graph.arc()), 1);
    let independent = crate::signal_graph::tests::prepare(&fixture).unwrap();
    let independent_graph = independent.arrangement().signal_graph.as_ref().unwrap();
    assert_eq!(graph.as_ref(), independent_graph.as_ref());
    assert_ne!(std::sync::Arc::as_ptr(independent_graph.arc()), pointer);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    crate::signal_graph::storage::visit(graph, &mut ledger).unwrap();
    crate::signal_graph::storage::visit(independent_graph, &mut ledger).unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert_eq!(ledger.selected_shared_counts_on_control(), (2, 2));
    assert_eq!(ledger.summary().unwrap().data_bytes, expected * 2);
    drop(ledger);
    let mut custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
    custody.arrangement_clips.root.topology = Some(graph.clone_on_control());
    custody.arrangement_clips.root.panic_after_metadata_take = true;
    assert!(!custody.dispose_arrangement_clips_on_control());
    assert_eq!(
        custody.phase,
        crate::AudioPreparationPhase::FailedIncomplete
    );
    assert!(custody.first_panic.is_some());
    let failed = &mut custody.arrangement_clips.root;
    assert!(failed.metadata_drop_pending);
    assert!(failed.topology.is_none());
    assert_eq!(
        failed.dispose_metadata_on_control(),
        Err(AudioStorageError::UnknownStorage)
    );
    assert!(failed.capacity(false).is_err());
    assert_eq!(std::sync::Arc::strong_count(graph.arc()), 1);
}

#[test]
fn fresh_feed_metadata_remains_charged_and_refuses_retry_after_disposal_panic() {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    for _ in 0..2 {
        editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap();
    }
    let snapshot = editor
        .project()
        .snapshot()
        .with_return_feed(
            editor.project().return_tracks()[0].id(),
            editor.project().return_tracks()[1].id(),
            0.5,
        )
        .unwrap();
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let mut root = ArrangementRootAssembly {
        feed: ReturnFeedPlan::prepare(&project).unwrap(),
        ..Default::default()
    };
    let bytes = root.feed.backing_bytes().unwrap();
    assert!(bytes > 0);
    assert_eq!(
        root.capacity(false).unwrap().fresh_feed_backing_bytes,
        bytes
    );
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    ledger.root(&root, AudioStorageRole::Current).unwrap();
    let before = ledger.summary().unwrap();
    root.visit_audio_storage(&mut ledger, None).unwrap();
    let after = ledger.summary().unwrap();
    assert_eq!(after.data_bytes - before.data_bytes, bytes);
    assert_eq!(after.data_allocations - before.data_allocations, 3);
    root.visit_audio_storage(&mut ledger, None).unwrap();
    assert_eq!(ledger.summary().unwrap(), after);
    drop(ledger);
    assert_eq!(root.reserve(&project), Err(AudioStorageError::Capacity));
    assert_eq!(root.feed.backing_bytes().unwrap(), bytes);
    let mut clean = ArrangementRootAssembly {
        feed: ReturnFeedPlan::prepare(&project).unwrap(),
        ..Default::default()
    };
    assert_eq!(clean.reserve(&project), Err(AudioStorageError::Capacity));
    clean.dispose_metadata_on_control().unwrap();
    assert!(!clean.has_metadata_disposal());
    clean.reserve(&project).unwrap();
    root.panic_after_metadata_take = true;
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || root.dispose_metadata_on_control()
        ))
        .is_err()
    );
    assert_eq!(root.feed.backing_bytes().unwrap(), bytes);
    assert!(root.has_metadata_disposal());
    assert_eq!(root.capacity(false), Err(AudioStorageError::UnknownStorage));
    root.panic_after_metadata_take = false;
    assert_eq!(
        root.dispose_metadata_on_control(),
        Err(AudioStorageError::UnknownStorage)
    );
    // Fixture owns the latched root until control teardown; no production reset.
}
