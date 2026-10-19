use super::*;
use hex_project::{BeatTime, Command, Created};
struct Fixture {
    bank: DeviceResourceBank,
    editor: hex_project::Editor,
    previous: OwnedShared<SharedAnalyticContextTable>,
    preparation: crate::AudioPreparationCustody,
    signal: crate::RetainedControlCancellationSignal,
    worker: crate::RetainedControlCancellation,
    job: crate::RetainedControlJob,
    transforms: AudioTransformPreparation,
}
impl Fixture {
    fn new() -> Self {
        assert!(crate::audio_storage_constructor_layout_supported());
        let (bank, identity, mut editor) =
            crate::shared_analytic::sampler_tests::canonical_fixture().unwrap();
        drop(identity);
        let track = editor.project().tracks()[0].id();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
        let previous = SharedAnalyticContextTable::prepare(
            editor.project(),
            &hex_devices::native_instrument_registry().unwrap(),
            None,
            Some(DeviceResourcePreparation {
                bank: &bank,
                max_scope_metadata_bytes: 65_536,
            }),
        )
        .unwrap();
        let signal = crate::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
        let job = crate::RetainedControlJob::try_from_owned_on_control(
            &mut Some(0_u8),
            usize::MAX,
            |_: &mut u8| Ok::<_, ()>(crate::RetainedControlJobProgress::Pending),
        )
        .unwrap();
        let worker = signal.bind_worker_on_control(job.route());
        let transforms =
            AudioTransformPreparation::with_retained_cancellation_on_control(worker.clone());
        Self {
            bank,
            editor,
            previous,
            preparation: crate::AudioPreparationCustody::try_new_on_control().unwrap(),
            signal,
            worker,
            job,
            transforms,
        }
    }
    fn original(&self, growth: usize) -> AudioRootStorageLedger<'_> {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        ledger
            .borrow_device_resource_bank_on_control(&self.bank, AudioStorageRole::Caller)
            .unwrap();
        self.signal.visit_storage_on_control(&mut ledger).unwrap();
        self.worker.visit_storage_on_control(&mut ledger).unwrap();
        ledger.growth(growth).unwrap();
        ledger
    }
}
fn limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 1024,
        handle_rows: 1024,
        root_rows: 128,
        max_retained_bytes: 16 * 1024 * 1024,
        max_construction_peak_bytes: 16 * 1024 * 1024,
    }
}
#[test]
fn actual_closed_sampler_compile_binds_bank_history_parent_and_constructs_both_selectors() {
    let f = Fixture::new();
    let original = f.original(4096);
    let original_summary = original.summary().unwrap();
    let mut rows = ArrangementClips::default();
    let mut outcome = Ok(());
    let measured = allocation_counter::measure(|| {
        outcome = rows.compile_original_sampler_on_control(
            &f.editor,
            &f.preparation,
            &f.bank,
            Some(&f.previous),
            &original,
            &f.transforms,
            limits(),
        );
    });
    eprintln!("closed actual compiler outcome={outcome:?}, allocations={measured:?}");
    outcome.unwrap();
    assert_eq!(original.summary().unwrap(), original_summary);
    let actual = rows.root.project().unwrap();
    assert_eq!(actual.tracks.len(), 1);
    assert!(actual.tracks[0].mutable_instrument);
    assert_eq!(actual.tracks[0].instrument.as_ref().unwrap().spans.len(), 1);
    assert_eq!(actual.shared_contexts.owners().count(), 1);
    assert!(rows.root.context_at(None).is_some());
    assert_eq!(f.preparation.phase(), AudioPreparationPhase::Vacant);
    assert!(f.job.inspect_payload::<u8, _>(|value| *value == 0).unwrap());
}

// Test-only independent normal inventory oracle: no arithmetic return is used
// to calculate the tight conservative DATA boundary fed to the real operation.
fn normal_bounds(
    rows: &ArrangementClips,
    f: &Fixture,
    original: &AudioRootStorageLedger<'_>,
    reservation: AudioStorageLedgerReservation,
) -> (usize, usize) {
    let construction = rows
        .sampler_construction(
            f.editor.project(),
            DeviceResourcePreparation {
                bank: &f.bank,
                max_scope_metadata_bytes: 65_536,
            },
        )
        .unwrap();
    f.transforms
        .with_pcm_inventory_on_control(|inventory| {
            let inputs = Inputs {
                rows,
                preparation: &f.preparation,
                editor: &f.editor,
                resources: &f.bank,
                previous: Some(&f.previous),
                original,
            };
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
            inputs.visit(inventory, &mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            let actual = ledger.summary()?;
            assert!(actual.require_complete().is_err());
            let cache = inventory.original_only_storage_cost()?;
            let metadata =
                ledger.metadata_capacity_bytes().unwrap() + size_of::<AudioRootStorageLedger<'_>>();
            let retained = actual.data_bytes
                + metadata
                + construction.retained
                + cache.transaction_new_backing_bytes;
            // Preserve the original live growth independently from new observed
            // growth; original_ledger_storage deliberately does not import it.
            let previous = original.summary()?;
            let old_growth = previous.construction_peak_bytes - previous.retained_bytes;
            let peak = actual.data_bytes
                + metadata
                + construction.peak
                + cache.transaction_new_backing_bytes
                + actual.construction_peak_bytes
                - actual.retained_bytes
                + old_growth;
            Ok((retained, peak))
        })
        .unwrap()
}

#[test]
fn actual_aggregate_exact_and_minus_one_limits_refuse_before_inspection_or_owner_birth() {
    for mode in ["exact", "retained-minus1", "peak-minus1", "row-shortage"] {
        let f = Fixture::new();
        let original = f.original(4096);
        let before = original.summary().unwrap();
        let mut rows = ArrangementClips::default();
        let (retained, peak) = normal_bounds(&rows, &f, &original, limits());
        let mut selected = AudioStorageLedgerReservation {
            max_retained_bytes: retained,
            max_construction_peak_bytes: peak,
            ..limits()
        };
        match mode {
            "retained-minus1" => selected.max_retained_bytes -= 1,
            "peak-minus1" => selected.max_construction_peak_bytes -= 1,
            "row-shortage" => {
                selected.data_rows = 1;
                selected.max_retained_bytes = limits().max_retained_bytes;
                selected.max_construction_peak_bytes = limits().max_construction_peak_bytes;
            }
            _ => {}
        }
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        let ids = crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get());
        let mut outcome = Ok(());
        let allocations = allocation_counter::measure(|| {
            outcome = rows.compile_original_sampler_on_control(
                &f.editor,
                &f.preparation,
                &f.bank,
                Some(&f.previous),
                &original,
                &f.transforms,
                selected,
            );
        });
        eprintln!(
            "ACTUAL bound {mode}: conservative retained={retained} peak={peak}, outcome={outcome:?}, alloc={allocations:?}, paired births={} clones={}",
            crate::owned_shared::birth_count(),
            crate::owned_shared::clone_count()
        );
        match mode {
            "exact" => {
                outcome.unwrap();
                assert!(rows.root.project().is_some());
                assert!(crate::owned_shared::birth_count() > 0);
                assert!(allocations.bytes_max <= peak as u64);
            }
            "row-shortage" => {
                assert_eq!(outcome, Err(AudioStorageError::Capacity));
                assert_eq!(crate::owned_shared::birth_count(), 0);
                assert_eq!(crate::owned_shared::clone_count(), 0);
                assert!(allocations.count_total > 0); // only normal inspection backing
                assert!(rows.root.project().is_none());
            }
            _ => {
                assert_eq!(outcome, Err(AudioStorageError::ReservationExceeded));
                assert_eq!(allocations.count_total, 0);
                assert_eq!(allocations.bytes_total, 0);
                assert_eq!(allocations.count_current, 0);
                assert_eq!(allocations.bytes_current, 0);
                assert_eq!(crate::owned_shared::birth_count(), 0);
                assert_eq!(crate::owned_shared::clone_count(), 0);
                assert_eq!(
                    crate::audio_only_inputs::ISSUED_HANDLES.with(|count| count.get()),
                    ids
                );
                assert!(rows.root.project().is_none());
                assert!(rows.root.context_at(None).is_none());
                let actual = rows.capacity_at(false, None, None).unwrap();
                assert_eq!(actual.tracks.row_bytes, 0);
                assert_eq!(actual.returns.row_bytes, 0);
            }
        }
        assert_eq!(original.summary().unwrap(), before);
    }
}

#[test]
fn actual_original_growth_and_standalone_bank_import_order_are_preserved() {
    let f = Fixture::new();
    let rows = ArrangementClips::default();
    let first = {
        let original = f.original(0);
        normal_bounds(&rows, &f, &original, limits())
    };
    let second = {
        let original = f.original(12_345);
        normal_bounds(&rows, &f, &original, limits())
    };
    // Equal backing and bank roots, measured with only the selected original live.
    assert_eq!(first.0, second.0);
    assert_eq!(second.1 - first.1, 12_345);
}

#[test]
fn actual_cheap_bound_cannot_skip_normal_closure_or_accept_an_omitted_context_alias() {
    let f = Fixture::new();
    let original = f.original(0);
    let omitted_alias = f.previous.clone_on_control();
    let mut rows = ArrangementClips::default();
    crate::owned_shared::reset_birth_count();
    crate::owned_shared::reset_clone_count();
    assert_eq!(
        rows.compile_original_sampler_on_control(
            &f.editor,
            &f.preparation,
            &f.bank,
            Some(&f.previous),
            &original,
            &f.transforms,
            limits()
        ),
        Err(AudioStorageError::UnknownStorage)
    );
    assert_eq!(crate::owned_shared::birth_count(), 0);
    assert_eq!(crate::owned_shared::clone_count(), 0);
    assert!(rows.root.project().is_none());
    assert!(rows.root.context_at(None).is_none());
    drop(omitted_alias);
    rows.compile_original_sampler_on_control(
        &f.editor,
        &f.preparation,
        &f.bank,
        Some(&f.previous),
        &original,
        &f.transforms,
        limits(),
    )
    .unwrap();
}

#[test]
fn actual_failed_parent_and_pending_destination_refuse_before_owner_activity() {
    for failed_parent in [true, false] {
        let mut f = Fixture::new();
        if failed_parent {
            f.preparation.phase = AudioPreparationPhase::FailedIncomplete;
        }
        let original = f.original(0);
        let mut rows = ArrangementClips::default();
        if !failed_parent {
            rows.metadata_disposal = Disposal::Pending;
        }
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        let mut outcome = Ok(());
        let allocations = allocation_counter::measure(|| {
            outcome = rows.compile_original_sampler_on_control(
                &f.editor,
                &f.preparation,
                &f.bank,
                Some(&f.previous),
                &original,
                &f.transforms,
                limits(),
            );
        });
        assert!(outcome.is_err());
        assert_eq!(allocations.count_total, 0);
        assert_eq!(allocations.bytes_total, 0);
        assert_eq!(crate::owned_shared::birth_count(), 0);
        assert_eq!(crate::owned_shared::clone_count(), 0);
        assert!(rows.root.project().is_none());
    }
}

#[test]
fn actual_cancelled_route_refuses_before_normal_backing_and_preserves_original() {
    let f = Fixture::new();
    let original = f.original(4096);
    let before = original.summary().unwrap();
    f.signal.cancel(f.job.route());
    let mut rows = ArrangementClips::default();
    crate::owned_shared::reset_birth_count();
    crate::owned_shared::reset_clone_count();
    let mut outcome = Ok(());
    let allocations = allocation_counter::measure(|| {
        outcome = rows.compile_original_sampler_on_control(
            &f.editor,
            &f.preparation,
            &f.bank,
            Some(&f.previous),
            &original,
            &f.transforms,
            limits(),
        );
    });
    assert_eq!(outcome, Err(AudioStorageError::SourceMismatch));
    assert_eq!(allocations.count_total, 0);
    assert_eq!(crate::owned_shared::birth_count(), 0);
    assert_eq!(crate::owned_shared::clone_count(), 0);
    assert_eq!(original.summary().unwrap(), before);
    assert!(rows.root.project().is_none());
    // No escaped inspection guard prevents a later control transaction.
    f.transforms.compile_transaction(|| Ok(())).unwrap();
}

#[test]
fn actual_sampler_compile_keeps_equal_content_return_plans_physically_distinct() {
    let mut f = Fixture::new();
    for _ in 0..2 {
        f.editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap();
    }
    let project = f.editor.project();
    let snapshot = project
        .snapshot()
        .with_return_feed(
            project.return_tracks()[0].id(),
            project.return_tracks()[1].id(),
            0.5,
        )
        .unwrap();
    f.editor = hex_project::Editor::new(hex_project::Project::from_snapshot(snapshot).unwrap(), 16)
        .unwrap();
    let original = f.original(8192);
    let mut rows = ArrangementClips::default();
    let (retained, peak) = normal_bounds(&rows, &f, &original, limits());
    rows.compile_original_sampler_on_control(
        &f.editor,
        &f.preparation,
        &f.bank,
        Some(&f.previous),
        &original,
        &f.transforms,
        AudioStorageLedgerReservation {
            max_retained_bytes: retained,
            max_construction_peak_bytes: peak,
            ..limits()
        },
    )
    .unwrap();
    let actual = rows.root.project().unwrap();
    assert_eq!(actual.return_buses.len(), 2);
    let fresh_bytes = actual.return_feed_plan.backing_bytes().unwrap();
    assert!(fresh_bytes > 0);
    let capacity = rows.capacity_at(false, None, None).unwrap();
    assert_eq!(capacity.root.fresh_feed_backing_bytes, fresh_bytes);
    assert!(capacity.returns.feed.bytes().unwrap() > 0);
    assert_eq!(rows.return_metadata().unwrap().order().count(), 2);
    assert_eq!(actual.return_feed_plan.edges().count(), 1);
    assert_eq!(rows.return_metadata().unwrap().feeds().count(), 1);
    assert_eq!(
        rows.returns
            .capacity_at(false, &actual.return_buses, Some(&actual.return_feed_plan)),
        Err(AudioStorageError::SourceMismatch)
    );
}

#[test]
fn actual_context_aliases_close_without_duplicate_pcm_or_reservation_enlargement() {
    let f = Fixture::new();
    let aliases: Vec<_> = (0..100).map(|_| f.previous.clone_on_control()).collect();
    let mut original = f.original(4096);
    original.root(&aliases, AudioStorageRole::Caller).unwrap();
    original.vec(&aliases).unwrap();
    for alias in &aliases {
        crate::shared_analytic::resource_storage::visit_table_contents(alias, &mut original)
            .unwrap();
    }
    let mut rows = ArrangementClips::default();
    let (retained, peak) = normal_bounds(&rows, &f, &original, limits());
    let start = std::time::Instant::now();
    rows.compile_original_sampler_on_control(
        &f.editor,
        &f.preparation,
        &f.bank,
        Some(&f.previous),
        &original,
        &f.transforms,
        AudioStorageLedgerReservation {
            max_retained_bytes: retained,
            max_construction_peak_bytes: peak,
            ..limits()
        },
    )
    .unwrap();
    eprintln!(
        "actual100 context aliases: retained={retained} peak={peak} elapsed={:?}",
        start.elapsed()
    );
    assert!(rows.root.project().is_some());
    assert_eq!(aliases.len(), 100);
}

#[test]
fn actual_compiler_error_and_panic_restore_same_context_without_legacy_producer() {
    for panic in [false, true] {
        let f = Fixture::new();
        let original = f.original(4096);
        let before = original.summary().unwrap();
        let mut rows = ArrangementClips::default();
        if panic {
            rows.tracks.panic_after = Some(0);
        } else {
            rows.tracks.fail_after = Some(0);
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rows.compile_original_sampler_on_control(
                &f.editor,
                &f.preparation,
                &f.bank,
                Some(&f.previous),
                &original,
                &f.transforms,
                limits(),
            )
        }));
        if panic {
            let payload = outcome.unwrap_err();
            assert_eq!(
                payload.downcast_ref::<String>().map(String::as_str),
                Some(
                    "assertion `left != right` failed: paired compiler Track move panic\n  left: Some(0)\n right: Some(0)"
                )
            );
            assert!(f.transforms.failed_incomplete_on_control());
        } else {
            assert_eq!(outcome.unwrap(), Err(AudioStorageError::UnsupportedGraph));
            assert!(!f.transforms.failed_incomplete_on_control());
        }
        assert_eq!(original.summary().unwrap(), before);
        assert!(rows.root.context_at(None).is_some());
        let context = rows.root.context_at(None).unwrap().as_ref() as *const _;
        let mut unrelated = crate::session_shared_preparation::SessionSharedConstruction::default();
        let shared_before = unrelated.observe().unwrap();
        let clones_before_restore = crate::owned_shared::clone_count();
        let mut restore = Ok(());
        let allocations = allocation_counter::measure(|| {
            restore = rows
                .root
                .restore(&mut rows.tracks, &mut rows.returns, &mut unrelated);
        });
        restore.unwrap();
        assert_eq!(allocations.count_total, 0);
        assert_eq!(crate::owned_shared::clone_count(), clones_before_restore);
        assert_eq!(
            rows.root.context_at(None).unwrap().as_ref() as *const _,
            context
        );
        assert_eq!(unrelated.observe().unwrap(), shared_before);
        rows.restore_root(&mut unrelated).unwrap();
        assert_eq!(
            rows.root.context_at(None).unwrap().as_ref() as *const _,
            context
        );
        let mut parent = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        parent.arrangement_clips = rows;
        assert_eq!(parent.phase(), AudioPreparationPhase::Vacant);
        parent.cleanup_on_control();
        assert_eq!(parent.phase(), AudioPreparationPhase::Recoverable);
        assert!(parent.arrangement_clips.root_context().is_none());
        parent.arrangement_clips.clear_disposed().unwrap();
        assert!(!parent.arrangement_clips.has_next());
        assert_eq!(original.summary().unwrap(), before);
    }
}

#[test]
fn actual_existing_cancellation_poll_seam_preserves_each_closed_operation_frontier() {
    for polls in [1, 2] {
        let f = Fixture::new();
        f.transforms.cancel_after_polls_for_test(polls);
        let original = f.original(4096);
        let before = original.summary().unwrap();
        let mut rows = ArrangementClips::default();
        crate::owned_shared::reset_birth_count();
        crate::owned_shared::reset_clone_count();
        let outcome = rows.compile_original_sampler_on_control(
            &f.editor,
            &f.preparation,
            &f.bank,
            Some(&f.previous),
            &original,
            &f.transforms,
            limits(),
        );
        assert_eq!(original.summary().unwrap(), before);
        assert!(rows.root.project().is_none());
        assert!(!f.transforms.failed_incomplete_on_control());
        if polls == 1 {
            assert_eq!(outcome, Err(AudioStorageError::SourceMismatch));
            assert!(rows.root.context_at(None).is_none());
            assert_eq!(crate::owned_shared::birth_count(), 0);
            assert_eq!(crate::owned_shared::clone_count(), 0);
        } else {
            assert_eq!(outcome, Err(AudioStorageError::UnsupportedGraph));
            assert!(rows.root.context_at(None).is_some());
            assert!(crate::owned_shared::birth_count() > 0);
        }
        f.transforms.compile_transaction(|| Ok(())).unwrap();
    }
}

#[test]
fn root_owned_context_only_cancellation_is_discovered_and_retired_by_actual_parent() {
    let f = Fixture::new();
    f.transforms.cancel_after_polls_for_test(2);
    let original = f.original(4096);
    let before = original.summary().unwrap();
    let mut rows = ArrangementClips::default();
    assert!(
        rows.compile_original_sampler_on_control(
            &f.editor,
            &f.preparation,
            &f.bank,
            Some(&f.previous),
            &original,
            &f.transforms,
            limits()
        )
        .is_err()
    );
    assert!(rows.root.project().is_none());
    assert!(rows.root_context().is_some());
    assert!(rows.has_next());
    assert!(rows.clear_disposed().is_err());
    let mut parent = crate::AudioPreparationCustody::try_new_on_control().unwrap();
    parent.arrangement_clips = rows;
    parent.cleanup_on_control();
    assert_eq!(parent.phase(), AudioPreparationPhase::Recoverable);
    assert!(parent.arrangement_clips.root_context().is_none());
    parent.cleanup_on_control();
    parent.arrangement_clips.clear_disposed().unwrap();
    parent.arrangement_clips.clear_disposed().unwrap();
    assert_eq!(original.summary().unwrap(), before);
}

#[test]
fn root_owned_successful_project_restores_same_table_and_real_parent_retires_once() {
    let f = Fixture::new();
    let original = f.original(4096);
    let before = original.summary().unwrap();
    let mut rows = ArrangementClips::default();
    rows.compile_original_sampler_on_control(
        &f.editor,
        &f.preparation,
        &f.bank,
        Some(&f.previous),
        &original,
        &f.transforms,
        limits(),
    )
    .unwrap();
    let context = rows.root_context().unwrap().as_ref() as *const _;
    let mut unrelated = crate::session_shared_preparation::SessionSharedConstruction::default();
    rows.restore_root(&mut unrelated).unwrap();
    assert_eq!(rows.root_context().unwrap().as_ref() as *const _, context);
    rows.restore_root(&mut unrelated).unwrap();
    let order = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let children = order.clone();
    rows.tracks.probe = Some(crate::audio_preparation::DropProbe(Some(Box::new(
        move || {
            assert_eq!(
                children.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                0
            );
        },
    ))));
    let context_drop = order.clone();
    rows.root.context_probe = Some(crate::audio_preparation::DropProbe(Some(Box::new(
        move || {
            assert_eq!(
                context_drop.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                1
            );
        },
    ))));
    let mut parent = crate::AudioPreparationCustody::try_new_on_control().unwrap();
    parent.arrangement_clips = rows;
    parent.cleanup_on_control();
    assert_eq!(parent.phase(), AudioPreparationPhase::Recoverable);
    assert_eq!(order.load(std::sync::atomic::Ordering::SeqCst), 2);
    parent.cleanup_on_control();
    assert_eq!(order.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(parent.arrangement_clips.root_context().is_none());
    assert_eq!(original.summary().unwrap(), before);
}

#[test]
fn root_owned_retirement_unwind_is_pending_and_actual_parent_preserves_first_payload() {
    let f = Fixture::new();
    f.transforms.cancel_after_polls_for_test(2);
    let original = f.original(4096);
    let before = original.summary().unwrap();
    let mut rows = ArrangementClips::default();
    assert!(
        rows.compile_original_sampler_on_control(
            &f.editor,
            &f.preparation,
            &f.bank,
            Some(&f.previous),
            &original,
            &f.transforms,
            limits()
        )
        .is_err()
    );
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    rows.root.context_probe = Some(crate::audio_preparation::DropProbe(Some(Box::new(
        move || {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            panic!("root-owned context retirement probe");
        },
    ))));
    let mut parent = crate::AudioPreparationCustody::try_new_on_control().unwrap();
    parent.arrangement_clips = rows;
    parent.cleanup_on_control();
    assert_eq!(parent.phase(), AudioPreparationPhase::FailedIncomplete);
    let payload = parent
        .first_panic
        .as_ref()
        .unwrap()
        .downcast_ref::<&'static str>()
        .unwrap()
        .to_owned();
    assert_eq!(payload, "root-owned context retirement probe");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(parent.arrangement_clips.root_context().is_none());
    assert!(parent.arrangement_clips.has_next());
    assert!(
        parent
            .arrangement_clips
            .capacity_at(false, None, None)
            .is_err()
    );
    assert!(parent.arrangement_clips.clear_disposed().is_err());
    parent.cleanup_on_control();
    assert_eq!(
        parent
            .first_panic
            .as_ref()
            .unwrap()
            .downcast_ref::<&'static str>()
            .unwrap(),
        &payload
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        parent
            .arrangement_clips
            .root
            .dispose_metadata_on_control()
            .is_err()
    );
    assert!(
        parent
            .arrangement_clips
            .root
            .begin_compiler_context()
            .is_err()
    );
    assert_eq!(original.summary().unwrap(), before);
}

pub(in crate::audio_preparation::arrangement_clips) fn compiled_root_owned_rows(
    mode: &str,
) -> ArrangementClips {
    let f = Fixture::new();
    if mode == "context-only" {
        f.transforms.cancel_after_polls_for_test(2);
    }
    let original = f.original(4096);
    let mut rows = ArrangementClips::default();
    if mode == "track-error" {
        rows.tracks.fail_after = Some(0);
    }
    if mode == "selector-panic" {
        rows.tracks.panic_after = Some(0);
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rows.compile_original_sampler_on_control(
            &f.editor,
            &f.preparation,
            &f.bank,
            Some(&f.previous),
            &original,
            &f.transforms,
            limits(),
        )
    }));
    match mode {
        "context-only" | "track-error" => assert!(result.unwrap().is_err()),
        "selector-panic" => assert!(result.is_err()),
        _ => result.unwrap().unwrap(),
    }
    // External bank/history/previous owners disappear normally; only actual
    // persistent compiler destinations and their real resource leases remain.
    rows
}

mod parent_bound;
