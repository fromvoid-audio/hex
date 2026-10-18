use super::*;
use crate::audio_preparation::{
    AudioArrangementInput, AudioPreparationCustody, AudioPreparationPhase,
};
use hex_project::{Command, Created, Editor};
use std::sync::atomic::{AtomicUsize, Ordering};
fn supported(name: &str) -> bool {
    crate::audio_preparation::session_source_tests::supported(name)
}
fn fixture(
    count: usize,
    feeds: bool,
) -> (
    hex_project::Project,
    OwnedAudioInputs,
    crate::AudioSourceToken,
) {
    let (p, mut b) = crate::audio_preparation::tests::fixture(1);
    b.reservation.handle_obligations = 64; // explicit caller reservation, not new global policy
    let asset = p.assets().next().unwrap().id();
    let token = b.source_token(asset).unwrap();
    let mut e = Editor::new(p, 32).unwrap();
    let mut ids = Vec::new();
    for i in 0..count {
        let Created::ReturnTrack(id) = e
            .execute(Command::AddReturnTrack {
                name: format!("return {i}"),
            })
            .unwrap()
        else {
            panic!("return")
        };
        ids.push(id);
    }
    let mut s = e.project().snapshot();
    for (i, bus) in s.return_tracks.iter_mut().enumerate() {
        bus.gain = 0.5 + (i % 3) as f32 * 0.25;
        bus.muted = i % 2 != 0;
    }
    if feeds {
        for (i, source) in ids.iter().enumerate() {
            for destination in &ids[i + 1..] {
                s = s.with_return_feed(*source, *destination, 0.25).unwrap();
            }
        }
        s.return_feeds.reverse();
    }
    (hex_project::Project::from_snapshot(s).unwrap(), b, token)
}
fn begin(
    c: &mut AudioPreparationCustody,
    p: &hex_project::Project,
    b: OwnedAudioInputs,
) -> Result<crate::AudioPreparationToken, Box<crate::AudioArrangementRefusal>> {
    c.begin_input_on_control(AudioArrangementInput::new(p, 11, b).with_audio_return_inventory())
}
#[test]
fn actual_return_buses_and_feed_plan_match_legacy_order_edges_and_propagation() {
    if !supported("arrangement_returns_oracle") {
        return;
    }
    for (count, edges, backward) in [
        (0, false, false),
        (3, false, false),
        (3, true, false),
        (16, true, false),
        (3, true, true),
    ] {
        let (mut p, b, old) = fixture(count, edges && !backward);
        if backward {
            let ids = p.return_tracks().iter().map(|r| r.id()).collect::<Vec<_>>();
            p = hex_project::Project::from_snapshot(
                p.snapshot()
                    .with_return_feed(ids[2], ids[0], 0.25)
                    .unwrap()
                    .with_return_feed(ids[0], ids[1], 0.5)
                    .unwrap(),
            )
            .unwrap();
        }
        let pointer = b.sources.as_ptr();
        let capacity = b.sources.capacity();
        let expected = crate::return_feed_plan::ReturnFeedPlan::prepare(&p).unwrap();
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        let t = begin(&mut c, &p, b).ok().unwrap();
        assert_eq!(
            c.phase(),
            AudioPreparationPhase::ArrangementAndSessionSources
        );
        let r = &c.arrangement_clips.returns;
        let plan = r.feed.plan().unwrap();
        assert_eq!(plan.order(), expected.order());
        let actual_edges = r.feed.feeds().collect::<Vec<_>>();
        let expected_edges = expected.test_edges().collect::<Vec<_>>();
        assert_eq!(actual_edges, expected_edges);
        let view = c.arrangement_return_metadata().unwrap();
        assert_eq!(
            view.buses().collect::<Vec<_>>(),
            p.return_tracks()
                .iter()
                .map(|b| (b.id(), b.gain(), b.muted()))
                .collect::<Vec<_>>()
        );
        assert_eq!(view.order().collect::<Vec<_>>(), expected.order());
        assert_eq!(view.feeds().collect::<Vec<_>>(), expected_edges);
        for (i, row) in r.rows[..r.active].iter().enumerate() {
            let bus = row.bus.as_ref().unwrap();
            assert_eq!(bus.id, p.return_tracks()[i].id());
            assert!(bus.device_chain.is_empty());
            assert_eq!(Arc::strong_count(&bus.device_chain), 1);
            assert_ne!(row.handle, 0);
            if i > 0 {
                assert!(!Arc::ptr_eq(
                    &bus.device_chain,
                    &r.rows[0].bus.as_ref().unwrap().device_chain
                ));
            }
        }
        if edges {
            let mut x = vec![[[0.; 2]; 256]; count];
            for (i, row) in x.iter_mut().enumerate() {
                row.fill([i as f32 + 1., 0.5]);
            }
            let mut y = x.clone();
            for source in plan.order() {
                plan.propagate(*source, &mut x, 16);
                expected.propagate(*source, &mut y, 16);
            }
            assert_eq!(x, y);
        }
        let report = c.capacity_report().arrangement_clips.returns;
        assert_eq!(report, r.capacity(true).unwrap());
        assert_eq!(report.independent_handle_obligations, count);
        assert_eq!(report.reserved_handle_destinations, count);
        assert_eq!(report.chain_payload_bytes, 0);
        if count > 0 {
            assert!(report.chain_declared_overhead_bytes > 0);
        }
        if !edges {
            assert!(view.order().next().is_none());
            assert_eq!(report.feed.feed_bytes, 0);
        }
        assert!(c.cancel(&t));
        c.cleanup_on_control();
        assert!(c.arrangement_return_metadata().is_none());
        let b = c.recover_on_control(&t).unwrap();
        assert_eq!(b.sources.as_ptr(), pointer);
        assert_eq!(b.sources.capacity(), capacity);
        assert!(b.source_view(&old).is_ok());
    }
    eprintln!(
        "hex preparation evidence: arrangement_returns_oracle; supported-controls=completed; empty-disconnected-DAG-max16-backward=5"
    );
}
#[test]
fn every_return_and_installed_plan_failure_or_panic_keeps_actual_persistent_owners() {
    if !supported("arrangement_returns_partial") {
        return;
    }
    for phase in 0..=3 {
        let (p, b, old) = fixture(3, true);
        let pointer = b.sources.as_ptr();
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        c.arrangement_clips.returns.fail_after = Some(phase);
        let t = begin(&mut c, &p, b).ok().unwrap();
        assert_eq!(c.phase(), AudioPreparationPhase::Cleaning);
        assert_eq!(
            c.arrangement_clips
                .returns
                .rows
                .iter()
                .filter(|r| r.bus.is_some())
                .count(),
            (phase + 1).min(3)
        );
        assert_eq!(c.arrangement_clips.returns.feed.installed(), phase == 3);
        assert!(c.arrangement_return_metadata().is_none());
        c.cleanup_on_control();
        let b = c.recover_on_control(&t).unwrap();
        assert_eq!(b.sources.as_ptr(), pointer);
        assert!(b.source_view(&old).is_ok());
        c.arrangement_clips.returns.fail_after = None;
        let t = begin(&mut c, &p, b).ok().unwrap();
        assert!(c.arrangement_return_metadata().is_some());
        assert!(c.cancel(&t));
        c.cleanup_on_control();
        assert!(c.recover_on_control(&t).is_some());
        let (p, b, _) = fixture(3, true);
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        c.arrangement_clips.returns.panic_after = Some(phase);
        let t = begin(&mut c, &p, b).ok().unwrap();
        assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
        assert!(c.first_panic.is_some());
        let before = c.arrangement_clips.returns.capacity(false).unwrap();
        c.cleanup_on_control();
        assert_eq!(c.arrangement_clips.returns.capacity(false).unwrap(), before);
        assert!(c.recover_on_control(&t).is_none());
    }
    eprintln!(
        "hex preparation evidence: arrangement_returns_partial; supported-controls=completed; row-plus-installed-errors4-panics4"
    );
}
#[test]
fn borrowed_feed_partial_copy_error_and_panic_preserve_original_bank_and_allocations() {
    if !supported("arrangement_returns_feed_failure") {
        return;
    }
    for panic in [false, true] {
        let (p, b, old) = fixture(3, true);
        let pointer = b.sources.as_ptr();
        let capacity = b.sources.capacity();
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        if panic {
            c.arrangement_clips.returns.feed.panic_after = Some(1);
        } else {
            c.arrangement_clips.returns.feed.fail_after = Some(1);
        }
        let refusal = begin(&mut c, &p, b).err().unwrap();
        assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
        assert_eq!(refusal.inputs.sources.capacity(), capacity);
        assert!(refusal.inputs.source_view(&old).is_ok());
        assert!(refusal.capacity.arrangement_clips.returns.feed.feed_bytes > 0);
        if panic {
            assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
            assert!(c.first_panic.is_some());
            assert!(
                c.arrangement_clips
                    .returns
                    .rows
                    .iter()
                    .all(|r| r.chain.is_some())
            );
            c.cleanup_on_control();
            assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
        } else {
            assert_eq!(c.phase(), AudioPreparationPhase::Vacant);
            c.arrangement_clips.returns.feed.fail_after = None;
            let t = begin(&mut c, &p, refusal.inputs).ok().unwrap();
            assert!(c.cancel(&t));
            c.cleanup_on_control();
            assert!(c.recover_on_control(&t).is_some());
        }
    }
    eprintln!(
        "hex preparation evidence: arrangement_returns_feed_failure; supported-controls=completed; borrowed-error-panic2"
    );
}
#[test]
fn actual_chain_destructor_panic_retains_feed_tracks_bank_payload_and_pending_tombstone() {
    if !supported("arrangement_returns_disposal_panic") {
        return;
    }
    let (p, b, _) = fixture(3, true);
    let pointer = b.sources.as_ptr();
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    let t = begin(&mut c, &p, b).ok().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let observe = Arc::clone(&count);
    let thread = std::thread::current().id();
    c.arrangement_clips.returns.probe = Some(DropProbe(Some(Box::new(move || {
        assert_eq!(std::thread::current().id(), thread);
        observe.fetch_add(1, Ordering::Relaxed);
        panic!("actual Return chain disposal probe");
    }))));
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    assert_eq!(c.phase(), AudioPreparationPhase::FailedIncomplete);
    assert!(c.first_panic.is_some());
    assert_eq!(
        c.arrangement_clips.returns.rows[0].disposal,
        Disposal::Pending
    );
    assert!(c.arrangement_clips.returns.feed.installed());
    assert!(c.arrangement_clips.tracks.metadata(0, &[]).is_some());
    assert_eq!(c.bank.as_ref().unwrap().sources.as_ptr(), pointer);
    c.cleanup_on_control();
    assert!(c.recover_on_control(&t).is_none());
    assert_eq!(count.load(Ordering::Relaxed), 1);
    eprintln!(
        "hex preparation evidence: arrangement_returns_disposal_panic; supported-controls=completed; destructor-count1"
    );
}
#[test]
fn actual_return_nested_spares_growth_reservations_and_clone_conservation() {
    if !supported("arrangement_returns_capacity") {
        return;
    }
    let (p, b, _) = fixture(3, true);
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    crate::owned_shared::reset_clone_count();
    let t = begin(&mut c, &p, b).ok().unwrap();
    let clones = crate::owned_shared::clone_count();
    assert!(clones > 0);
    let first = c.capacity_report();
    let allocations = c.arrangement_clips.returns.feed.test_allocations();
    assert!(!allocations.is_empty());
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    let b = c.recover_on_control(&t).unwrap();
    assert_eq!(
        c.arrangement_clips.returns.feed.test_allocations(),
        allocations
    );
    let t = begin(&mut c, &p, b).ok().unwrap();
    assert_eq!(
        c.arrangement_clips.returns.feed.test_allocations(),
        allocations
    );
    assert_eq!(
        c.capacity_report()
            .arrangement_clips
            .returns
            .previous_growth_backing_bytes,
        0
    );
    assert_eq!(
        c.capacity_report().arrangement_clips.returns.bytes(),
        first.arrangement_clips.returns.bytes()
    );
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    let b = c.recover_on_control(&t).unwrap();
    let (p0, _, _) = fixture(0, false);
    let t = begin(&mut c, &p0, b).ok().unwrap();
    let report = c.capacity_report().arrangement_clips.returns;
    assert!(report.feed.feed_bytes > 0 && report.feed.outgoing_header_bytes > 0);
    assert_eq!(report.independent_handle_obligations, 0);
    assert_eq!(report.chain_declared_overhead_bytes, 0);
    let view = c.arrangement_return_metadata().unwrap();
    assert!(view.buses().next().is_none());
    assert!(view.feeds().next().is_none());
    assert!(view.order().next().is_none());
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    drop(c.recover_on_control(&t).unwrap());
    let (p, b, _) = fixture(16, true);
    let t = begin(&mut c, &p, b).ok().unwrap();
    let report = c.capacity_report();
    assert!(
        report
            .arrangement_clips
            .returns
            .previous_growth_backing_bytes
            > 0
    );
    assert_eq!(
        report.construction_peak_bytes,
        report.retained_stage_bytes
            + report.previous_slot_backing_bytes
            + report.session.previous_backing_bytes
            + report.structure.previous_growth_backing_bytes
            + report.tempo.previous_growth_backing_bytes
            + report.assembly.previous_growth_backing_bytes
            + report.arrangement_clips.previous_growth_backing_bytes
    );
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    assert!(c.recover_on_control(&t).is_some());
    for handles in [true, false] {
        let (p, mut b, old) = fixture(3, true);
        let pointer = b.sources.as_ptr();
        let capacity = b.sources.capacity();
        if handles {
            b.reservation.handle_obligations = 9;
        } else {
            b.reservation.max_prepared_data_bytes = first.retained_stage_bytes - 1;
        }
        let reservation = b.reservation;
        let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
        let refusal = begin(&mut c, &p, b).err().unwrap();
        assert_eq!(
            refusal.reason,
            if handles {
                AudioStorageError::Capacity
            } else {
                AudioStorageError::ReservationExceeded
            }
        );
        assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
        assert_eq!(refusal.inputs.sources.capacity(), capacity);
        assert_eq!(refusal.inputs.reservation, reservation);
        assert!(refusal.inputs.source_view(&old).is_ok());
    }
    let (p, b, _) = fixture(3, true);
    let mut c = AudioPreparationCustody::try_new_on_control().unwrap();
    crate::owned_shared::reset_clone_count();
    let t = c
        .begin_input_on_control(AudioArrangementInput::new(&p, 11, b).with_audio_track_inventory())
        .ok()
        .unwrap();
    assert_eq!(crate::owned_shared::clone_count(), clones);
    assert!(c.cancel(&t));
    c.cleanup_on_control();
    assert!(c.recover_on_control(&t).is_some());
    assert_eq!(
        ArrangementReturnCapacity {
            row_bytes: usize::MAX,
            chain_payload_bytes: 1,
            ..Default::default()
        }
        .bytes(),
        Err(AudioStorageError::Overflow)
    );
    eprintln!(
        "hex preparation evidence: arrangement_returns_capacity; supported-controls=completed; staging-clones={clones}"
    );
}

#[test]
fn paired_return_utility_children_preserve_pcm_aliases_moves_and_pending_disposal() {
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole};
    if !supported("paired_return_utility_children") {
        return;
    }
    eprintln!("SUPPORTED actual paired Return Utility children lifecycle and legacy PCM");
    for filtered in [false, true] {
        for bypassed in [false, true] {
            let (project, bank) = utility_return_fixture(filtered, bypassed);
            assert_eq!(
                crate::audio_preparation::validate_constructor_project(&project),
                Err(AudioStorageError::UnsupportedGraph)
            );
            let mut media = std::collections::BTreeMap::new();
            for (id, source) in &bank.sources {
                let SourceKind::Resident(audio) = &source.kind else {
                    panic!("resident");
                };
                media.insert(*id, audio.clone());
            }
            let reference = crate::compile::project(&project, &media).unwrap();
            let mut assembly = ReturnAssembly::default();
            assembly.reserve(&project).unwrap();
            assembly.construct().unwrap();
            let bus = assembly.rows[0].bus.as_ref().unwrap();
            let expected = &reference.return_buses[0];
            assert_eq!(
                (bus.id, bus.gain, bus.muted),
                (expected.id, expected.gain, expected.muted)
            );
            assert_eq!(bus.device_chain.len(), 1);
            assert_eq!(
                bus.device_chain[0].owner,
                hex_lattice::ProcessorChainOwner::Return(bus.id.value())
            );
            assert_eq!(bus.device_chain[0].definition.is_some(), filtered);
            assert_eq!(bus.device_chain[0].bypassed, bypassed);
            let chain_pointer = bus.device_chain.as_ptr();
            let state_pointer = Arc::as_ptr(bus.device_chain[0].configuration.as_ref().unwrap());
            let meter_pointer = Arc::as_ptr(&bus.device_chain[0].meter);
            let mut actual_runtime = crate::EffectRuntimeChain::new_prepared(
                crate::effect_resources::clone_prepared_on_control(&bus.device_chain).unwrap(),
            )
            .unwrap();
            let mut expected_runtime = crate::EffectRuntimeChain::new_prepared(
                crate::effect_resources::clone_prepared_on_control(&expected.device_chain).unwrap(),
            )
            .unwrap();
            let input = std::array::from_fn::<_, 257, _>(|i| {
                [if i == 0 { 1.0 } else { 0.125 }, -(i as f32) / 512.0]
            });
            let mut actual = input;
            let mut expected_pcm = input;
            actual_runtime.begin_block();
            expected_runtime.begin_block();
            actual_runtime.process_block(&mut actual[..63]);
            actual_runtime.process_block(&mut actual[63..]);
            expected_runtime.process_block(&mut expected_pcm);
            actual_runtime.commit_block();
            expected_runtime.commit_block();
            assert_eq!(actual, expected_pcm);
            drop((actual_runtime, expected_runtime, reference, media));
            let budget = AudioStorageLedgerReservation {
                data_rows: 256,
                handle_rows: 256,
                root_rows: 32,
                max_retained_bytes: 1_000_000,
                max_construction_peak_bytes: 1_000_000,
            };
            let collect = |r: &ReturnAssembly,
                           roots: &Vec<ReturnBus>,
                           feed: Option<&crate::return_feed_plan::ReturnFeedPlan>,
                           limits| {
                let mut ledger = AudioRootStorageLedger::try_new_on_control(limits)?;
                ledger.root(r, AudioStorageRole::Current)?;
                ledger.vec(roots)?;
                ledger.mutate(|ledger| r.visit_audio_storage(ledger, roots, feed))?;
                let report = ledger.summary()?;
                report.require_known_storage().unwrap();
                ledger.validate_selected_shared_handles_on_control()?;
                // Selected constructor/alias closure does not open the full
                // retirement/publication gate.
                assert_eq!(
                    report.require_complete(),
                    Err(crate::AudioStorageIncomplete::RetirementReceiptUnknown)
                );
                if report.unresolved_handles != 0 {
                    // Raw visitors are called once inside a ledger transaction.
                    // A duplicate physical handle visit refuses and invalidates
                    // every partial total; it cannot add a synthetic owner or
                    // turn repeated growth scratch into a publishable report.
                    assert_eq!(
                        ledger.mutate(|ledger| r.visit_audio_storage(ledger, roots, feed)),
                        Err(AudioStorageError::SourceMismatch)
                    );
                    assert_eq!(ledger.summary(), Err(AudioStorageError::UnknownStorage));
                }
                Ok::<_, AudioStorageError>(report)
            };
            let mut roots = Vec::new();
            let report = collect(&assembly, &roots, None, budget).unwrap();
            let mut exact = budget;
            exact.max_retained_bytes = report.retained_bytes;
            exact.max_construction_peak_bytes = report.construction_peak_bytes;
            assert_eq!(collect(&assembly, &roots, None, exact).unwrap(), report);
            exact.max_retained_bytes -= 1;
            assert_eq!(
                collect(&assembly, &roots, None, exact),
                Err(AudioStorageError::ReservationExceeded)
            );
            // A real extra outer owner is independently required even though
            // its payload/config/processor allocations are the same bodies.
            let exported = Arc::clone(&assembly.rows[0].bus.as_ref().unwrap().device_chain);
            assert!(collect(&assembly, &roots, None, budget).is_err());
            drop(exported);
            roots.try_reserve_exact(1).unwrap();
            let mut feed = crate::return_feed_plan::ReturnFeedPlan::default();
            assembly.preflight_root_move().unwrap();
            assembly.move_root_row(0, &mut roots);
            assembly.move_root_feed(&mut feed);
            assert_eq!(roots[0].device_chain.as_ptr(), chain_pointer);
            collect(&assembly, &roots, Some(&feed), budget).unwrap();
            assembly.preflight_root_restore(&roots, true).unwrap();
            assembly.restore_root(&mut roots, &mut feed, true);
            let bus = assembly.rows[0].bus.as_ref().unwrap();
            assert_eq!(bus.device_chain.as_ptr(), chain_pointer);
            assert_eq!(
                Arc::as_ptr(bus.device_chain[0].configuration.as_ref().unwrap()),
                state_pointer
            );
            assert_eq!(Arc::as_ptr(&bus.device_chain[0].meter), meter_pointer);
            while assembly.has_next() {
                assembly.dispose_next().unwrap();
                collect(&assembly, &roots, None, budget).unwrap();
            }
            assembly.clear_disposed().unwrap();
            assert!(assembly.rows[0].bus.is_none());
            assert_eq!(assembly.rows[0].utility.handles(), 0);
            // Same actual nonempty constructor, injected final control failure:
            // parent catches once, pending tombstone forbids observe/retry.
            let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
            custody.arrangement_clips.returns.reserve(&project).unwrap();
            custody.arrangement_clips.returns.construct().unwrap();
            custody.arrangement_clips.phase = Phase::Installed;
            custody.arrangement_clips.returns.probe = Some(DropProbe(Some(Box::new(|| {
                panic!("paired Return control disposal probe")
            }))));
            assert!(!custody.dispose_arrangement_clips_on_control());
            assert_eq!(custody.phase, AudioPreparationPhase::FailedIncomplete);
            assert!(custody.first_panic.is_some());
            let failed = &mut custody.arrangement_clips.returns;
            assert_eq!(failed.rows[0].disposal, Disposal::Pending);
            assert_eq!(
                failed.dispose_next(),
                Err(AudioStorageError::UnknownStorage)
            );
            assert!(matches!(
                collect(failed, &roots, None, budget),
                Err(AudioStorageError::UnknownStorage)
            ));
        }
    }
}

fn utility_return_fixture(
    filtered: bool,
    bypassed: bool,
) -> (hex_project::Project, OwnedAudioInputs) {
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    let (project, bank, _) = fixture(1, false);
    let mut snapshot = project.snapshot();
    let definition = &hex_devices::utility::UTILITY_DEFINITION;
    let state = definition
        .patch_parameter(
            &definition.default_state().unwrap(),
            hex_devices::utility::parameter::DC_REMOVE,
            ParameterValue::Bool(filtered),
        )
        .unwrap()
        .with_input_gain_millidb(-6000)
        .unwrap()
        .with_output_gain_millidb(3000)
        .unwrap();
    snapshot.return_tracks[0]
        .devices
        .push(hex_project::DeviceSnapshot {
            id: snapshot.next_id,
            bypassed,
            state,
        });
    snapshot.next_id += 1;
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    (project, bank)
}

#[test]
fn paired_return_utility_constructor_frontiers_keep_original_children_before_cleanup() {
    if !supported("paired_return_utility_frontiers") {
        return;
    }
    eprintln!("SUPPORTED actual nonempty paired Return constructor error/panic frontiers");
    for panic in [false, true] {
        let (project, _bank) = utility_return_fixture(true, false);
        let mut assembly = ReturnAssembly::default();
        assembly.reserve(&project).unwrap();
        let original = assembly.rows[0].chain.as_ref().unwrap().as_ptr();
        if panic {
            assembly.panic_after = Some(0);
        } else {
            assembly.fail_after = Some(0);
        }
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| assembly.construct()));
        if panic {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), Err(AudioStorageError::Allocation));
        }
        let bus = assembly.rows[0].bus.as_ref().unwrap();
        assert_eq!(bus.device_chain.as_ptr(), original);
        assert!(
            assembly.rows[0]
                .declaration
                .as_ref()
                .unwrap()
                .parts(&bus.device_chain)
                .is_some()
        );
        assert!(assembly.rows[0].chain.is_none());
        assert!(assembly.rows[0].bare.is_none());
        assert!(assembly.reserve(&project).is_err());
        assert_eq!(assembly.construct(), Err(AudioStorageError::SourceMismatch));
        let limits = crate::AudioStorageLedgerReservation {
            data_rows: 128,
            handle_rows: 128,
            root_rows: 16,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        };
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(limits).unwrap();
        ledger
            .root(&assembly, crate::AudioStorageRole::FailedConstruction)
            .unwrap();
        ledger
            .mutate(|ledger| assembly.visit_audio_storage(ledger, &[], None))
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert!(ledger.summary().unwrap().require_complete().is_err());
        drop(ledger);
        if !panic {
            while assembly.has_next() {
                assembly.dispose_next().unwrap();
            }
            assembly.clear_disposed().unwrap();
        }
        // The panic case is a selected raw constructor fixture, not an actual
        // parent recovery permit. Keep its owner alive through all assertions;
        // the production caller must retain panic/FailedIncomplete and charges.
    }
}
