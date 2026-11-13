use super::*;
#[cfg(feature = "allocation-probe")]
mod actions;
#[cfg(feature = "allocation-probe")]
mod current_sampler;
#[cfg(feature = "allocation-probe")]
mod domain_facts;
#[cfg(feature = "allocation-probe")]
mod fanout;
#[cfg(feature = "allocation-probe")]
mod native_resources;
#[cfg(feature = "allocation-probe")]
mod pages;
#[cfg(all(feature = "allocation-probe", feature = "stream-test-support"))]
mod read_failure;
#[cfg(feature = "allocation-probe")]
mod readiness;
#[cfg(feature = "allocation-probe")]
mod reverb;
#[cfg(feature = "allocation-probe")]
mod scalar;
#[cfg(feature = "allocation-probe")]
pub(in crate::prepared) mod stream_fixture;
#[cfg(feature = "allocation-probe")]
mod utility_fixture;
use crate::prepared::candidate_tests::{anchored, candidate, finish, recover, serial};

/// Bound lifecycle waits so a broken handoff fails promptly; normal runs take milliseconds.
const LIFECYCLE_BOUND: std::time::Duration = std::time::Duration::from_millis(500);

/// The nonblocking usage query is None while a lifecycle pass holds the bank
/// (#4029). Wait for an actual observation; never infer zero from None.
fn observed_usage(control: &Control) -> crate::AudioCandidateDomainUsage {
    let deadline = std::time::Instant::now() + LIFECYCLE_BOUND;
    loop {
        if let Some(usage) = control.audio_candidate_domain_usage() {
            return usage;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "domain usage unavailable: {}",
            control.candidate_root.test_status()
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn usage_observation_waits_out_lifecycle_bank_contention() {
    let _serial = serial();
    let Some(mut owner) = candidate() else { return };
    let limits = reservation(&mut owner);
    let (mut control, data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let root = control.candidate_root.clone();
    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        root.with_domain_bank_held(|| {
            held.send(()).unwrap();
            released.recv().unwrap();
        })
    });
    holding.recv().unwrap();
    // The exact #4029 failure: a raw unwrap here observed contention as None.
    assert!(control.audio_candidate_domain_usage().is_none());
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        release.send(()).unwrap();
    });
    let expected = observed_usage(&control);
    holder.join().unwrap();
    releaser.join().unwrap();
    assert_eq!(control.audio_candidate_domain_usage(), Some(expected));
    drop(owner);
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}
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
    assert!(control.status.publication_in_flight());
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
    assert_eq!(
        control.cancel_audio_candidate(&first_ticket),
        crate::AudioCandidateCancellationResult::Cancelled
    );
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert!(!control.status.publication_in_flight());
    let second_ticket = control
        .stage_eligible_audio_candidate(
            refused.owner,
            refused.domains,
            PublicationPolicy::InheritRuntime,
            42,
        )
        .ok()
        .unwrap();
    assert_eq!(
        control.cancel_audio_candidate(&second_ticket),
        crate::AudioCandidateCancellationResult::Cancelled
    );
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let third = candidate().unwrap();
    let third_id = third.identity();
    let third_ticket = control
        .stage_eligible_audio_candidate(third, vec![], PublicationPolicy::InheritRuntime, 43)
        .ok()
        .unwrap();
    assert_eq!(
        control.cancel_audio_candidate(&third_ticket),
        crate::AudioCandidateCancellationResult::Cancelled
    );
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
    assert!(!control.status.publication_pending.load(Acquire));
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

#[test]
fn cancellation_is_exact_to_pending_incarnation_and_fresh_policy_refuses_original_owner() {
    let _serial = serial();
    let Some(mut first) = candidate() else { return };
    let reservation = reservation(&mut first);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(reservation)
        .unwrap();
    let identity = first.identity();
    let ticket = control
        .stage_eligible_audio_candidate(first, vec![], PublicationPolicy::InheritRuntime, 1)
        .ok()
        .unwrap();
    assert_eq!(
        control.cancel_audio_candidate(&ticket),
        crate::AudioCandidateCancellationResult::Cancelled
    );
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert_eq!(
        control.status.rejected_revision(),
        Some((1, PlaybackHandoffRejection::FreshRuntimeCancelled))
    );
    let recovered = recover(&mut control, &ticket);
    assert_eq!(recovered.disposed.identity(), identity);
    assert_eq!(
        recovered.return_kind,
        crate::AudioCandidateReturnKind::NeverInstalled
    );
    drop(recovered);
    let owner = candidate().unwrap();
    let identity = owner.identity();
    let domains = Vec::with_capacity(5);
    let ptr = domains.as_ptr();
    let cap = domains.capacity();
    let refused = control
        .stage_eligible_audio_candidate(owner, domains, PublicationPolicy::InstallFreshRuntime, 2)
        .err()
        .unwrap();
    assert_eq!(refused.owner.identity(), identity);
    assert_eq!(
        refused.owner.phase(),
        hex_engine::AudioCandidateDisposalPhase::Owned
    );
    assert_eq!(
        (refused.domains.as_ptr(), refused.domains.capacity()),
        (ptr, cap)
    );
    assert_eq!(control.revision, 1);
    let next = control
        .stage_eligible_audio_candidate(
            refused.owner,
            refused.domains,
            PublicationPolicy::InheritRuntime,
            2,
        )
        .ok()
        .unwrap();
    assert_eq!(
        control.cancel_audio_candidate(&ticket),
        crate::AudioCandidateCancellationResult::Unavailable
    );
    assert!(control.recover_audio_candidate(&ticket).is_none());
    // Cancel this distinct next incarnation too, leaving no test-only active
    // owner that would require a fabricated completed backend handle.
    assert_eq!(
        control.cancel_audio_candidate(&next),
        crate::AudioCandidateCancellationResult::Cancelled
    );
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    drop(recover(&mut control, &next));
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}

#[cfg(feature = "allocation-probe")]
#[test]
fn real_utility_transfer_pressure_retry_restores_cancellation_without_changing_active_ack() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let (a_project, track) = utility_fixture::project_with_loop(64);
    let mut edit = hex_project::Editor::new(a_project.clone(), 4).unwrap();
    edit.execute(hex_project::Command::RemoveTrack(track))
        .unwrap();
    let b_project = edit.project();
    let mut a = utility_fixture::owner(&a_project);
    let b = utility_fixture::owner(b_project);
    let b_id = b.identity();
    let reservation = reservation(&mut a);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(reservation)
        .unwrap();
    let _a_ticket = control
        .stage_eligible_audio_candidate(a, vec![], PublicationPolicy::InheritRuntime, 8)
        .ok()
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let pole = (-std::f64::consts::TAU * 5.0 / 48_000.0).exp();
    let mut frame_index = 0;
    let mut render = |data: &mut crate::native_callback_owner::Guard<
        crate::native_callback_owner::NativeCallbackOwner,
    >| {
        let mut pcm = [[0.0; 2]; 8];
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut pcm).unwrap();
            });
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        for frame in pcm {
            // Independent closed form for constant input through the 5 Hz DC filter.
            let scale = pole.powi(frame_index);
            assert!(
                (f64::from(frame[0]) - 0.25 * scale).abs() < 1e-6
                    && (f64::from(frame[1]) + f64::from(0.1_f32) * scale).abs() < 1e-6,
                "donor PCM changed: {frame:?}"
            );
            frame_index += 1;
        }
    };
    render(&mut data);
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert_eq!(control.status.position.load(Acquire), 8);
    let observation = control
        .status
        .transport_observation()
        .expect("eligible observation");
    assert_eq!(
        (observation.audio_revision, observation.control_generation),
        (1, 8)
    );
    assert_eq!(
        (
            observation.arrangement_frame,
            observation.performance_frame,
            observation.playing
        ),
        (8, 8, true)
    );
    let b_ticket = control
        .stage_eligible_audio_candidate(b, vec![], PublicationPolicy::InheritRuntime, 8)
        .ok()
        .unwrap();
    data.owner_mut().unwrap().run(|c, _| {
        // Real Group::plan checks the actual moved Utility chain Boxes against
        // its destination allowance. No synthetic Retry/activity/owner flag.
        c.test_eligible_transfer_allowance =
            Some(hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES);
        c.begin_block();
        assert!(c.deferred_eligible.is_some());
        assert!(matches!(c.active, Some(ActivePublication::Eligible(_))));
    });
    render(&mut data);
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert_eq!(control.status.position.load(Acquire), 16);
    assert_eq!(
        control
            .status
            .transport_observation()
            .unwrap()
            .arrangement_frame,
        16
    );
    assert!(control.status.publication_pending.load(Acquire));
    assert_eq!(
        control.cancel_audio_candidate(&b_ticket),
        crate::AudioCandidateCancellationResult::Cancelled
    );
    render(&mut data);
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert_eq!(control.status.position.load(Acquire), 24);
    assert_eq!(
        control.status.rejected_revision(),
        Some((2, PlaybackHandoffRejection::FreshRuntimeCancelled))
    );
    let bank = recover(&mut control, &b_ticket);
    assert_eq!(bank.disposed.identity(), b_id);
    assert_eq!(
        bank.return_kind,
        crate::AudioCandidateReturnKind::NeverInstalled
    );
    drop(bank);
    // Actual backend-handle completion, never a fabricated terminal token.
    drop(control);
    drop(data);
    drop(errors);
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    let mut retirement = std::thread::spawn(move || {
        let _ = crate::lifecycle::test_dispose_backend(
            &mut backend,
            &mut pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(()),
        )
        .unwrap();
        assert!(backend.is_none() && disposed);
        retirement
    })
    .join()
    .unwrap();
    finish(&mut retirement);
}

#[cfg(feature = "allocation-probe")]
#[test]
fn actual_verified_domain_reuse_context_refusal_and_readiness_rejection_hold_pair_until_reports() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let (project, _) = utility_fixture::project();
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::new();
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let mut a = stream_fixture::owner(&project, metadata.clone());
    let reservation = reservation(&mut a);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control.set_audio_diagnostics(AudioDiagnosticsFlags::ALL);
    control
        .configure_audio_candidate_publication(reservation)
        .unwrap();
    let domain = control
        .register_audio_candidate_domain(stream_fixture::pair(definition))
        .ok()
        .unwrap();
    let wrong = control
        .register_audio_candidate_domain(stream_fixture::pair(wav.definition(asset, 12)))
        .ok()
        .unwrap();
    assert_ne!(domain, wrong);
    assert_eq!(observed_usage(&control).occupied_domains, 2);
    let identity = a.identity();
    let wrong_tokens = vec![wrong];
    let ptr = wrong_tokens.as_ptr();
    let cap = wrong_tokens.capacity();
    let refused = control
        .stage_eligible_audio_candidate(a, wrong_tokens, PublicationPolicy::InheritRuntime, 11)
        .err()
        .unwrap();
    assert_eq!(refused.owner.identity(), identity);
    assert_eq!(
        refused.owner.phase(),
        hex_engine::AudioCandidateDisposalPhase::Owned
    );
    assert_eq!(
        (refused.domains.as_ptr(), refused.domains.capacity()),
        (ptr, cap)
    );
    let a_ticket = control
        .stage_eligible_audio_candidate(
            refused.owner,
            vec![domain],
            PublicationPolicy::InheritRuntime,
            11,
        )
        .ok()
        .unwrap();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert_eq!(
        control.status.rejected_revision(),
        Some((1, PlaybackHandoffRejection::StreamedPagesNotReady))
    );
    let diagnostic_events: Vec<_> =
        std::iter::from_fn(|| control.take_audio_diagnostic()).collect();
    assert!(diagnostic_events.iter().any(|event| event.revision == 1
        && matches!(
            event.kind,
            AudioDiagnosticKind::PublicationRejected {
                reason: PlaybackHandoffRejection::StreamedPagesNotReady,
            }
        )));
    assert!(
        !diagnostic_events
            .iter()
            .any(|event| event.kind == AudioDiagnosticKind::PublicationApplied)
    );
    let b = stream_fixture::owner(&project, metadata);
    let b_ticket = control
        .stage_eligible_audio_candidate(b, vec![domain], PublicationPolicy::InheritRuntime, 11)
        .ok()
        .unwrap();
    data.owner_mut().unwrap().run(|c, _| {
        c.begin_block();
        let mut out = [[1.0; 2]; 8];
        c.render(&mut out).unwrap();
        assert_eq!(out, [[0.0; 2]; 8]);
    });
    assert_eq!(
        control.status.rejected_revision(),
        Some((2, PlaybackHandoffRejection::StreamedPagesNotReady))
    );
    assert_eq!(observed_usage(&control).occupied_domains, 2); // same actual pair reused, no N3
    assert!(control.retire_audio_candidate_domain(domain));
    assert!(control.recover_audio_candidate_domain(domain).is_none());
    let a_bank = recover(&mut control, &a_ticket);
    assert_eq!(a_bank.disposed.identity(), identity);
    assert!(control.recover_audio_candidate_domain(domain).is_none()); // B still holds its original use
    let b_bank = recover(&mut control, &b_ticket);
    assert!(control.retire_audio_candidate_domain(wrong));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    for token in [domain, wrong] {
        loop {
            control.reclaim();
            if let Some(report) = control.recover_audio_candidate_domain(token) {
                assert!(report.accounting_complete);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "actual domain join/report did not finish"
            );
            std::thread::yield_now();
        }
        assert!(control.recover_audio_candidate_domain(token).is_none());
    }
    let usage = observed_usage(&control);
    assert_eq!(usage.occupied_domains, 0);
    drop(a_bank);
    drop(b_bank);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}

mod foreign_source;
#[cfg(feature = "allocation-probe")]
mod ordinary;
