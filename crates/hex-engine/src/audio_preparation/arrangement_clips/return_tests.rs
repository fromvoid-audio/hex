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
            assert_eq!(Arc::strong_count(&bus.output_meter), 1);
            assert_ne!(row.handle, 0);
            assert_ne!(row.meter_handle, 0);
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
        assert_eq!(report.independent_handle_obligations, count * 2);
        assert_eq!(report.reserved_handle_destinations, count * 2);
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
