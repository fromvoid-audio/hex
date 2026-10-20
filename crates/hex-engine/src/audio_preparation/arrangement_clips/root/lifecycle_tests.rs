use super::*;
use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
fn limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 1024,
        handle_rows: 1024,
        root_rows: 128,
        max_retained_bytes: 16 * 1024 * 1024,
        max_construction_peak_bytes: 16 * 1024 * 1024,
    }
}
fn compiled(context_only: bool) -> ArrangementClips {
    compiled_mode(if context_only {
        "context-only"
    } else {
        "project"
    })
}
fn compiled_mode(mode: &str) -> ArrangementClips {
    super::super::sampler_compile::tests::compiled_root_owned_rows(mode)
}
#[test]
fn root_owned_actual_inventory_closes_in_both_placements_and_after_restoration() {
    for placement in [
        "context-only",
        "project",
        "track-error",
        "selector-panic",
        "restored",
        "revision-return",
    ] {
        eprintln!("root-owned actual inventory placement={placement}");
        let mut rows = compiled_mode(placement);
        let context = rows.root_context().unwrap().as_ref() as *const _;
        let mut shared = crate::session_shared_preparation::SessionSharedConstruction::default();
        let mut external = None;
        if placement == "restored" {
            rows.restore_root(&mut shared).unwrap();
        }
        if placement == "revision-return" {
            rows.transfer_project_revision(&mut external);
            assert!(rows.can_restore_revision_partial(true, false));
        }
        {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
            ledger.root(&rows, AudioStorageRole::Current).unwrap();
            if external.is_some() {
                ledger.root(&external, AudioStorageRole::Current).unwrap();
            }
            assert!(
                rows.visit_audio_storage(&mut ledger, external.as_ref(), None)
                    .unwrap()
                    .is_none()
            );
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
            let first = ledger.summary().unwrap();
            first.require_known_storage().unwrap();
            // The new embedded-root observer must deduplicate the same field.
            // Existing paired Track observers deliberately reject duplicate
            // fixed constructor IDs, so do not rerun that separate interface.
            rows.root
                .visit_audio_storage(&mut ledger, external.as_ref())
                .unwrap();
            assert_eq!(ledger.summary().unwrap(), first);
            // The previous-context operand may be this SAME actual field
            // already covered by the enclosing root/Project inspection.
            crate::shared_analytic::resource_storage::visit_table(
                rows.root.context_at(external.as_ref()).unwrap(),
                &mut ledger,
                AudioStorageRole::Predecessor,
            )
            .unwrap();
            assert_eq!(ledger.summary().unwrap(), first);
            assert!(first.require_complete().is_err()); // not publication
        }
        if placement == "revision-return" {
            rows.restore_revision_values(&mut external, &mut None);
            assert!(external.is_none());
            assert_eq!(rows.root_context().unwrap().as_ref() as *const _, context);
            rows.restore_root(&mut shared).unwrap();
        }
        let mut parent = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        parent.arrangement_clips = rows;
        parent.cleanup_on_control();
        assert_eq!(
            parent.phase(),
            AudioPreparationPhase::Recoverable,
            "{placement}"
        );
    }
}
#[test]
fn root_owned_omitted_real_child_alias_fails_closure() {
    let rows = compiled(false);
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    ledger.root(&rows, AudioStorageRole::Current).unwrap();
    crate::shared_analytic::resource_storage::visit_table_contents(
        rows.root_context().unwrap(),
        &mut ledger,
    )
    .unwrap();
    // The actual compiled Track's table handle is still live but unvisited.
    assert_eq!(
        ledger.validate_selected_shared_handles_on_control(),
        Err(AudioStorageError::UnknownStorage)
    );
}
#[test]
fn root_owned_wrong_route_and_occupied_slot_refuse_before_any_move() {
    for occupied in [false, true] {
        let mut rows = compiled(false);
        let context = rows.root_context().unwrap().as_ref() as *const _;
        if occupied {
            rows.root.contexts = Some(rows.root_context().unwrap().clone_on_control());
        } else {
            rows.root.context_lifecycle = ContextLifecycle::None;
        }
        let mut shared = crate::session_shared_preparation::SessionSharedConstruction::default();
        assert!(rows.restore_root(&mut shared).is_err());
        assert!(rows.root.project.is_some());
        assert_eq!(
            rows.root.project.as_ref().unwrap().shared_contexts.as_ref() as *const _,
            context
        );
        drop(rows.root.contexts.take());
        rows.root.context_lifecycle = ContextLifecycle::CompilerOwned;
        let mut parent = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        parent.arrangement_clips = rows;
        parent.cleanup_on_control();
        assert_eq!(parent.phase(), AudioPreparationPhase::Recoverable);
    }
}
#[test]
fn root_owned_opaque_context_refuses_before_children_or_context_are_moved() {
    let crate::shared_analytic::cleanup_tests::OpaqueFixture {
        table,
        owner,
        drops,
    } = crate::shared_analytic::cleanup_tests::opaque_fixture();
    drop(owner);
    let mut rows = ArrangementClips::default();
    rows.root.begin_compiler_context().unwrap();
    rows.root.contexts = Some(table);
    let context = rows.root_context().unwrap().as_ref() as *const _;
    let mut shared = crate::session_shared_preparation::SessionSharedConstruction::default();
    assert_eq!(
        rows.restore_root(&mut shared),
        Err(AudioStorageError::UnknownStorage)
    );
    assert_eq!(
        rows.root.dispose_metadata_on_control(),
        Err(AudioStorageError::UnknownStorage)
    );
    assert_eq!(rows.root_context().unwrap().as_ref() as *const _, context);
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(rows.root.context_lifecycle, ContextLifecycle::CompilerOwned);
    drop(rows); // explicit test teardown, never the refused production operation
    assert_eq!(drops.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn root_owned_retirement_unwind_keeps_remaining_metadata_and_first_payload_rooted() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut rows = compiled(false);
    let project = rows.root.project.as_ref().unwrap();
    // Zero-runtime metadata is still a real paired topology owner. Both it and
    // authored tempo backing remain live after the context retirement frontier.
    let graph = project.signal_graph.as_ref().unwrap();
    assert!(!graph.needs_runtime());
    let topology = Arc::as_ptr(graph.arc());
    let tempo = project.tempo_map.as_ref().unwrap().points().as_ptr();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    rows.root.context_probe = Some(crate::audio_preparation::DropProbe(Some(Box::new(
        move || {
            count.fetch_add(1, Ordering::SeqCst);
            panic!("retirement pending keeps remaining metadata");
        },
    ))));
    let mut parent = crate::AudioPreparationCustody::try_new_on_control().unwrap();
    parent.arrangement_clips = rows;
    parent.cleanup_on_control();
    assert_eq!(parent.phase(), AudioPreparationPhase::FailedIncomplete);
    let root = &parent.arrangement_clips.root;
    assert_eq!(root.context_lifecycle, ContextLifecycle::RetirementPending);
    assert!(root.contexts.is_none());
    assert_eq!(Arc::as_ptr(root.topology.as_ref().unwrap().arc()), topology);
    assert_eq!(root.tempo_map.as_ref().unwrap().points().as_ptr(), tempo);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let payload = parent.first_panic.as_ref().unwrap().as_ref() as *const _;
    parent.cleanup_on_control();
    assert!(std::ptr::eq(
        parent.first_panic.as_ref().unwrap().as_ref() as *const _,
        payload
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        parent
            .arrangement_clips
            .root
            .clear_context_retirement()
            .is_err()
    );
    let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
    assert!(
        ledger
            .borrow_custody_on_control(&parent, AudioStorageRole::FailedConstruction)
            .is_err()
    );
}

#[test]
fn actual_context_only_wrong_route_is_discovered_and_parent_refuses_without_drop() {
    let mut rows = compiled(true);
    let context = rows.root_context().unwrap().as_ref() as *const _;
    rows.root.context_lifecycle = ContextLifecycle::None;
    assert!(rows.has_next()); // actual field, not scalar route, discovers it
    let mut parent = crate::AudioPreparationCustody::try_new_on_control().unwrap();
    parent.arrangement_clips = rows;
    parent.cleanup_on_control();
    assert_eq!(parent.phase(), AudioPreparationPhase::FailedIncomplete);
    assert_eq!(
        parent.arrangement_clips.root_context().unwrap().as_ref() as *const _,
        context
    );
    assert!(parent.first_panic.is_none());
    parent.cleanup_on_control();
    assert_eq!(
        parent.arrangement_clips.root_context().unwrap().as_ref() as *const _,
        context
    );
}
