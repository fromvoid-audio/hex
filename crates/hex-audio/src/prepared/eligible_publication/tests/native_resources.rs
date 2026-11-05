use super::*;
#[test]
fn native_flag_without_actual_carriers_and_resource_pressure_refuse_before_consuming_inputs() {
    let _serial = serial();
    let Some(mut owner) = candidate() else { return };
    let identity = owner.identity();
    let mut limits = reservation(&mut owner);
    let (mut naked, callback) = channel(48000).unwrap();
    naked.enable_native_custody().unwrap();
    assert!(naked.configure_audio_candidate_publication(limits).is_err());
    let tokens = Vec::with_capacity(4);
    let ptr = tokens.as_ptr();
    let cap = tokens.capacity();
    let refused = naked
        .stage_eligible_audio_candidate(owner, tokens, PublicationPolicy::InheritRuntime, 1)
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
    drop(naked);
    drop(callback);
    let (mut control, data, errors, mut retirement) = anchored();
    limits.handle_rows = 32; // finite caller capacity: no new process/global cap
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let refused = control
        .stage_eligible_audio_candidate(
            refused.owner,
            refused.domains,
            PublicationPolicy::InheritRuntime,
            1,
        )
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
    assert_eq!(control.revision, 0);
    assert_eq!(control.audio_candidate_custody().occupied_slots, 0);
    let pair = crate::prepared::candidate_tests::pair();
    let refused_pair = control.register_audio_candidate_domain(pair).err().unwrap();
    let lease = refused_pair.domain.0.catalog_lease().unwrap();
    assert!(refused_pair.domain.1.matches_catalog(&lease));
    drop(lease);
    assert!(
        refused_pair
            .domain
            .0
            .shutdown(refused_pair.domain.1)
            .unwrap()
            .accounting_complete
    );
    assert_eq!(observed_usage(&control).occupied_domains, 0);
    drop(refused);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
#[test]
fn both_final_guard_orders_and_completed_backend_gate_actual_eligible_return() {
    let _serial = serial();
    for errors_first in [false, true] {
        let Some(mut owner) = candidate() else { return };
        let identity = owner.identity();
        let limits = reservation(&mut owner);
        let (mut control, mut data, errors, retirement) = anchored();
        control
            .configure_audio_candidate_publication(limits)
            .unwrap();
        let ticket = control
            .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 1)
            .ok()
            .unwrap();
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        assert_eq!(control.status.applied_revision.load(Acquire), 1);
        let mut retirement = std::thread::spawn(move || {
            let mut retirement = retirement;
            let mut backend = Some(());
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
            assert!(backend.is_none() && disposed);
            retirement
        })
        .join()
        .unwrap();
        if errors_first {
            drop(errors);
            assert!(!retirement.advance().unwrap());
            drop(data);
        } else {
            drop(data);
            assert!(!retirement.advance().unwrap());
            drop(errors);
        }
        assert!(control.recover_audio_candidate(&ticket).is_none());
        let mut retirement = std::thread::spawn(move || {
            let mut r = retirement;
            let _ = r.advance().unwrap();
            r
        })
        .join()
        .unwrap();
        let ready = recover(&mut control, &ticket);
        assert_eq!(ready.disposed.identity(), identity);
        assert_eq!(
            ready.return_kind,
            crate::AudioCandidateReturnKind::FinalActive
        );
        assert_eq!(control.audio_candidate_custody().occupied_slots, 0);
        assert!(
            control
                .audio_candidate_custody()
                .runtime_and_shared_overhead_unknown
        );
        drop(ready);
        drop(control);
        finish(&mut retirement);
    }
}
#[test]
fn actual_eligible_owner_remains_pinned_after_carrier_destructor_panic_without_retry_or_ready() {
    let _serial = serial();
    let Some(mut owner) = candidate() else { return };
    let identity = owner.identity();
    let limits = reservation(&mut owner);
    let (mut control, mut data, mut errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 1)
        .ok()
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    // Control-side fault sensitivity only, after valid admission. This test-only
    // carrier probe is not a claimed production payload/cost or callback catch.
    errors.owner_mut().unwrap().test_drop_probe(move || {
        count.fetch_add(1, Relaxed);
        std::panic::panic_any("eligible carrier drop");
    });
    drop(data);
    drop(errors);
    let (mut retirement, first) = std::thread::spawn(move || {
        let mut backend = Some(());
        let mut pending = false;
        let mut disposed = false;
        let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::lifecycle::test_dispose_backend(
                &mut backend,
                &mut pending,
                &mut disposed,
                &mut retirement,
                |_| Ok(()),
            )
        }))
        .err()
        .unwrap();
        (retirement, first)
    })
    .join()
    .unwrap();
    assert_eq!(retirement.test_extracted_identity(), Some(identity));
    assert!(retirement.advance().is_err());
    assert_eq!(calls.load(Relaxed), 1);
    assert!(first.downcast_ref::<&str>().is_some());
    assert!(control.recover_audio_candidate(&ticket).is_none());
    assert_eq!(control.audio_candidate_custody().occupied_slots, 1);
    let original = candidate().unwrap();
    let original_id = original.identity();
    let refused = control
        .stage_eligible_audio_candidate(original, vec![], PublicationPolicy::InheritRuntime, 2)
        .err()
        .unwrap();
    assert_eq!(refused.owner.identity(), original_id);
    // Explicit fixture quarantine: never manufacture a successful disposal or
    // allow the singleton to take a new unleased cleanup job for this pinned Root.
    retirement.root().announce_retirement();
    drop(control);
    drop(refused);
    drop(first);
    std::mem::forget(retirement);
}

#[test]
fn closed_root_revokes_already_queued_claim_without_install_or_ack() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let (project, _) = utility_fixture::project();
    let (mut owner, bank) = utility_fixture::owner_with_bank(&project);
    let identity = owner.identity();
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let ticket = control
        .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 1)
        .ok()
        .unwrap();
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    // Revoke a real queued constructor claim, independently of Callback's own
    // shutdown flag. No already-committed callback decision is being revoked.
    control.native_candidate_root().close();
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            assert!(callback.active.is_none());
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
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert_eq!(
        control.status.rejected_revision(),
        Some((1, PlaybackHandoffRejection::ConstructorClaimsUnavailable))
    );
    let ready = recover(&mut control, &ticket);
    assert_eq!(ready.disposed.identity(), identity);
    assert_eq!(ready.inputs.test_bank_identity(), bank);
    assert_eq!(
        ready.return_kind,
        crate::AudioCandidateReturnKind::NeverInstalled
    );
    drop(ready);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}

#[test]
fn all_constructor_channel_handles_need_caller_capacity_before_original_owner_can_move() {
    let _serial = serial();
    let Some(mut owner) = candidate() else { return };
    let identity = owner.identity();
    let mut limits = reservation(&mut owner);
    let summary = owner
        .seal_constructor_on_control(hex_engine::AudioStorageLedgerReservation {
            data_rows: limits.data_rows,
            handle_rows: limits.handle_rows,
            root_rows: 1,
            max_retained_bytes: limits.max_retained_data_bytes,
            max_construction_peak_bytes: limits.max_construction_peak_data_bytes,
        })
        .unwrap();
    // This exact caller allowance covered the original three-ring/root witness,
    // but cannot also cover the actual six auxiliary rings/nine shared scalars.
    limits.handle_rows = limits.runtime_slots * 5 + 26 + summary.shared_resource_handles;
    let (mut control, data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let domains = Vec::with_capacity(2);
    let domain_identity = (domains.as_ptr(), domains.capacity());
    let refused = control
        .stage_eligible_audio_candidate(owner, domains, PublicationPolicy::InheritRuntime, 1)
        .err()
        .unwrap();
    assert_eq!(refused.reason, "constructor handle reservation exceeded");
    assert_eq!(refused.owner.identity(), identity);
    assert_eq!(
        refused.owner.phase(),
        hex_engine::AudioCandidateDisposalPhase::Owned
    );
    assert_eq!(
        (refused.domains.as_ptr(), refused.domains.capacity()),
        domain_identity
    );
    assert_eq!(control.revision, 0);
    assert_eq!(control.audio_candidate_custody().occupied_slots, 0);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);

    let mut owner = refused.owner;
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    // Opt-in closes the independent owning preview lane before UI decode starts.
    assert!(control.sample_preview.request().is_err());
    let pcm = hex_engine::Audio::new(48000, vec![[0.5; 2]; 8]).unwrap();
    let original = pcm.clone();
    let refused_preview = control.sample_preview.publish(1, pcm).unwrap_err();
    assert!(refused_preview.audio.shares_pcm_with(&original));
    drop(refused_preview);
    drop(original);
    let ticket = control
        .stage_eligible_audio_candidate(
            owner,
            refused.domains,
            PublicationPolicy::InheritRuntime,
            1,
        )
        .ok()
        .unwrap();
    assert_eq!(
        control.cancel_audio_candidate(&ticket),
        crate::AudioCandidateCancellationResult::Cancelled
    );
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let recovered = recover(&mut control, &ticket);
    assert_eq!(recovered.disposed.identity(), identity);
    drop(recovered);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
#[test]
fn an_already_used_preview_lane_cannot_be_relabelled_as_closed_constructor_custody() {
    let _serial = serial();
    let Some(mut owner) = candidate() else { return };
    let limits = reservation(&mut owner);
    let (mut control, data, errors, mut retirement) = anchored();
    let mut insufficient = limits;
    insufficient.max_retained_data_bytes = 1;
    assert!(
        control
            .configure_audio_candidate_publication(insufficient)
            .is_err()
    );
    // A failed reservation did not seal the still-Resident preview lane.
    let request = control.sample_preview.request().unwrap();
    let audio = hex_engine::Audio::new(48000, vec![[0.25; 2]; 16]).unwrap();
    control.sample_preview.publish(request, audio).unwrap();
    control.sample_preview.cancel();
    assert!(
        control
            .configure_audio_candidate_publication(limits)
            .is_err()
    );
    assert_eq!(control.revision, 0);
    // Resident preview semantics are unchanged by refused opt-in.
    assert!(control.sample_preview.request().is_ok());
    drop(owner);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}

#[test]
fn sentinel_preview_publication_returns_pcm_before_successful_constructor_seal() {
    let _serial = serial();
    let Some(mut owner) = candidate() else { return };
    let limits = reservation(&mut owner);
    let (mut control, mut data, errors, mut retirement) = anchored();
    let audio = hex_engine::Audio::new(48000, vec![[0.75; 2]; 16]).unwrap();
    let original = audio.clone();
    let refused = control.sample_preview.publish(0, audio).unwrap_err();
    assert!(refused.audio.shares_pcm_with(&original));
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let mut output = [[1.0; 2]; 16];
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
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
    assert_eq!(output, [[0.0; 2]; 16]);
    drop(refused);
    drop(original);
    drop(owner);
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}

#[test]
fn rooted_preparation_retries_actual_publication_pressure_then_installs_and_recovers_same_owner() {
    let _serial = serial();
    let Some(mut first) = candidate() else {
        return;
    };
    let limits = reservation(&mut first);
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let first_ticket = control
        .stage_eligible_audio_candidate(first, Vec::new(), PublicationPolicy::InheritRuntime, 2)
        .unwrap_or_else(|refused| panic!("{}", refused.reason));
    let preparation = control
        .reserve_audio_candidate_preparation(
            crate::AudioCandidatePreparationReservation {
                source_capacity: 1,
                member_rows: limits.member_rows / limits.runtime_slots,
                transfer_rows: limits.transfer_rows / limits.runtime_slots,
                handle_rows: limits.handle_rows / limits.runtime_slots,
                storage: hex_engine::AudioStorageReservation {
                    allocation_items: 256,
                    handle_obligations: 512,
                    max_prepared_data_bytes: limits.max_retained_data_bytes / limits.runtime_slots,
                    max_construction_data_bytes: limits.max_construction_peak_data_bytes
                        / limits.runtime_slots,
                    caller_retained_bytes: 0,
                    caller_construction_bytes: 0,
                },
            },
            Vec::new(),
        )
        .ok()
        .unwrap();
    use hex_project::{Command, Created, Editor};
    let mut editor = Editor::new(hex_project::Project::new(48000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "audio".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("track");
    };
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "actual.wav".into(),
            sample_rate: 48000,
            frames: 8,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 8,
            source_offset: 0,
            source_length: 8,
            looping: false,
        })
        .unwrap();
    let source = hex_engine::Audio::new(48000, vec![[0.75; 2]; 8]).unwrap();
    let mut original = None;
    let mut identity = None;
    assert_eq!(
        control
            .prepare_audio_candidate_on_control(preparation, |cell| {
                cell.copy_resident_source_on_control(asset, &source)?;
                original = Some(cell.test_source_collection_identity());
                cell.prepare_on_control(
                    editor.project(),
                    11,
                    hex_engine::AudioSourceContext {
                        project_generation: 2,
                        editor_revision: 8,
                    },
                )?;
                identity = cell.owner_on_control().map(|owner| owner.identity());
                Ok(())
            })
            .unwrap(),
        crate::AudioCandidatePreparationProgress::Ready
    );
    assert_eq!(
        control
            .stage_audio_candidate_preparation(preparation, 2)
            .err(),
        Some("playback publication already pending")
    );
    assert_eq!(control.status.submitted_revision.load(Acquire), 1);
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    let ticket = control
        .stage_audio_candidate_preparation(preparation, 2)
        .unwrap();
    assert_eq!(control.status.submitted_revision.load(Acquire), 2);
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    let action = crate::prepared::PlaybackCommand::Play;
    control.command(action).unwrap();
    let mut output = [[0.0; 2]; 4];
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        })
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
    assert_eq!(control.status.applied_revision.load(Acquire), 2);
    assert_eq!(output, [[0.75; 2]; 4]);
    // Original origins may still be covered by the live successor; replacement
    // alone is not a disposal/credit barrier for those retained anchors.
    control.request_shutdown();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let mut backend = Some(());
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
    assert!(backend.is_none() && disposed);
    drop(data);
    drop(errors);
    let _ = retirement.advance().unwrap();
    let old = recover(&mut control, &first_ticket);
    drop(old);
    let recovered = recover(&mut control, &ticket);
    assert_eq!(recovered.disposed.identity(), identity.unwrap());
    let bank = recovered.inputs.test_bank_identity();
    assert_eq!((bank.0, bank.1), original.unwrap());
    assert_eq!(
        recovered.return_kind,
        crate::AudioCandidateReturnKind::FinalActive
    );
    drop(recovered);
    drop(control);
    finish(&mut retirement);
}
