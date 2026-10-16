use super::*;
use crate::prepared::candidate_tests::{anchored, candidate, finish, recover, serial};
fn reservation(
    owner: &mut hex_engine::AudioCandidateOwner,
) -> crate::AudioCandidatePublicationReservation {
    let ledger = hex_engine::AudioStorageLedgerReservation {
        data_rows: 4096,
        handle_rows: 2048,
        root_rows: 1,
        max_retained_bytes: 96 * 1024 * 1024,
        max_construction_peak_bytes: 96 * 1024 * 1024,
    };
    let summary = owner.seal_constructor_on_control(ledger).unwrap();
    crate::AudioCandidatePublicationReservation {
        runtime_slots: 3,
        archive_slots: 3,
        member_rows: summary.member_rows.checked_mul(3).unwrap(),
        transfer_rows: summary.transfer_rows.checked_mul(3).unwrap(),
        receipt_rows: 4,
        data_rows: ledger.data_rows,
        handle_rows: ledger.handle_rows,
        max_retained_data_bytes: ledger.max_retained_bytes,
        max_construction_peak_data_bytes: ledger.max_construction_peak_bytes,
    }
}
#[test]
fn actual_queue_rejection_and_return_pressure_recover_original_owners_without_install_ack() {
    let _serial = serial();
    let Some(mut first) = candidate() else { return };
    let reservation = reservation(&mut first);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(reservation)
        .unwrap();
    let first_id = first.identity();
    let first_ticket = control
        .stage_eligible_audio_candidate(first, vec![], PublicationPolicy::InheritRuntime, 41)
        .ok()
        .unwrap();
    assert_eq!(control.revision, 1);
    assert!(control.publication_pending.load(Acquire));
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    let second = candidate().unwrap();
    let second_id = second.identity();
    let domains = Vec::with_capacity(7);
    let ptr = domains.as_ptr();
    let cap = domains.capacity();
    let refused = control
        .stage_eligible_audio_candidate(second, domains, PublicationPolicy::InheritRuntime, 42)
        .err()
        .unwrap();
    assert_eq!(refused.owner.identity(), second_id);
    assert_eq!(
        refused.owner.phase(),
        hex_engine::AudioCandidateDisposalPhase::Owned
    );
    assert_eq!(
        (refused.domains.as_ptr(), refused.domains.capacity()),
        (ptr, cap)
    );
    assert_eq!(control.revision, 1);
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert!(!control.publication_pending.load(Acquire));
    let second_ticket = control
        .stage_eligible_audio_candidate(
            refused.owner,
            refused.domains,
            PublicationPolicy::InheritRuntime,
            42,
        )
        .ok()
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let third = candidate().unwrap();
    let third_id = third.identity();
    let third_ticket = control
        .stage_eligible_audio_candidate(third, vec![], PublicationPolicy::InheritRuntime, 43)
        .ok()
        .unwrap();
    // The real return ring is full: the actual rejected owner must remain in
    // callback held custody, with no callback-side destruction or allocation.
    #[cfg(feature = "allocation-probe")]
    let measured = allocation_counter::measure(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
    });
    #[cfg(not(feature = "allocation-probe"))]
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    data.owner_mut().unwrap().run(|callback, _| {
        assert!(matches!(
            callback.held_retirement,
            Some(ReturnedPublication::Eligible(_))
        ));
        assert!(callback.active.is_none());
        let mut output = [[1.0; 2]; 8];
        callback.render(&mut output).unwrap();
        assert_eq!(output, [[0.0; 2]; 8]);
    });
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert_eq!(control.reclaim(), 2);
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    for (ticket, identity) in [
        (&first_ticket, first_id),
        (&second_ticket, second_id),
        (&third_ticket, third_id),
    ] {
        let result = recover(&mut control, ticket);
        assert_eq!(result.disposed.identity(), identity);
        assert_eq!(
            result.return_kind,
            crate::AudioCandidateReturnKind::NeverInstalled
        );
        assert!(result.inputs.owning_handle_count() > 0);
        assert!(control.recover_audio_candidate(ticket).is_none());
        drop(result);
    }
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
#[test]
fn terminal_drain_returns_actual_eligible_owner_after_control_first_drop() {
    let _serial = serial();
    let Some(mut owner) = candidate() else { return };
    let reservation = reservation(&mut owner);
    let (mut control, data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(reservation)
        .unwrap();
    let _abandoned_ticket = control
        .stage_eligible_audio_candidate(
            owner,
            Vec::with_capacity(2),
            PublicationPolicy::InheritRuntime,
            99,
        )
        .ok()
        .unwrap();
    let root = control.native_candidate_root();
    drop(control);
    drop(data);
    drop(errors);
    // Real guard release + terminal consumer draining, then the same persistent
    // lifecycle execution destroys the original bank without a Control/UI poll.
    finish(&mut retirement);
    assert!(root.test_eligible_complete());
}

#[cfg(feature = "allocation-probe")]
#[test]
fn actual_eligible_active_slot_installs_then_acks_renders_commands_and_returns_after_real_handle() {
    let _serial = serial();
    let Some(mut first) = candidate() else { return };
    let reservation = reservation(&mut first);
    let first_id = first.identity();
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(reservation)
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.test_admit_candidate = true);
    let first_ticket = control
        .stage_eligible_audio_candidate(first, vec![], PublicationPolicy::InheritRuntime, 71)
        .ok()
        .unwrap();
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    let measure = allocation_counter::measure(|| {
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
    });
    assert_eq!(
        (
            measure.count_total,
            measure.bytes_total,
            measure.count_current,
            measure.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert!(!control.publication_pending.load(Acquire));
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 4];
    data.owner_mut().unwrap().run(|callback, _| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(output, [[0.25; 2]; 4]);
    assert!(control.status.playing.load(Acquire));
    let second = candidate().unwrap();
    let second_id = second.identity();
    let second_ticket = control
        .stage_eligible_audio_candidate(second, vec![], PublicationPolicy::InheritRuntime, 72)
        .ok()
        .unwrap();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    let measure = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
    });
    assert_eq!(
        (
            measure.count_total,
            measure.bytes_total,
            measure.count_current,
            measure.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(control.status.applied_revision.load(Acquire), 2);
    assert_eq!(output, [[0.25; 2]; 4]); // same actual inherited runtime path
    let previous = recover(&mut control, &first_ticket);
    assert_eq!(previous.disposed.identity(), first_id);
    assert_eq!(
        previous.return_kind,
        crate::AudioCandidateReturnKind::Replaced
    );
    drop(previous);
    for _ in 0..COMMANDS {
        control.command(PlaybackCommand::Play).unwrap();
    }
    control.stop();
    let measure = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
    });
    assert_eq!(
        (
            measure.count_total,
            measure.bytes_total,
            measure.count_current,
            measure.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(output, [[0.0; 2]; 4]);
    // Both guards alone cannot mint the completed native handle frontier.
    drop(data);
    drop(errors);
    assert!(!retirement.advance().unwrap());
    assert!(control.recover_audio_candidate(&second_ticket).is_none());
    let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    struct Backend(Arc<std::sync::atomic::AtomicUsize>);
    impl Drop for Backend {
        fn drop(&mut self) {
            self.0.fetch_add(1, Relaxed);
        }
    }
    let backend = Some(Backend(drops.clone()));
    let (mut retirement, thread) = std::thread::spawn(move || {
        let mut retirement = retirement;
        let mut backend = backend;
        let mut pending = false;
        let mut disposed = false;
        assert!(
            !crate::lifecycle::test_dispose_backend(
                &mut backend,
                &mut pending,
                &mut disposed,
                &mut retirement,
                |_| Ok(())
            )
            .unwrap()
        );
        assert!(backend.is_none() && disposed && !pending);
        (retirement, std::thread::current().id())
    })
    .join()
    .unwrap();
    assert_ne!(thread, std::thread::current().id());
    assert_eq!(drops.load(Relaxed), 1);
    let final_bank = recover(&mut control, &second_ticket);
    assert_eq!(final_bank.disposed.identity(), second_id);
    assert_eq!(
        final_bank.return_kind,
        crate::AudioCandidateReturnKind::FinalActive
    );
    drop(final_bank);
    drop(control);
    finish(&mut retirement);
}
