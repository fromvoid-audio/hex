use super::*;
use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
fn playback() -> Playback {
    let project = hex_project::Project::new(48000).unwrap();
    let prepared = hex_engine::PreparedRevision::prepare(
        &project,
        &std::collections::BTreeMap::new(),
        hex_engine::SharedSineTable::new_on_control(),
        None,
    )
    .unwrap();
    Playback::from_prepared(Arc::new(prepared))
}
#[cfg(feature = "allocation-probe")]
#[test]
fn actual_resident_archive_pairs_two_fields_and_retains_real_storage_until_root_drop() {
    let bytes = ResidentArchive::requested_bytes().unwrap();
    let mut archive = None;
    let measured = allocation_counter::measure(|| archive = Some(ResidentArchive::new().unwrap()));
    assert_eq!(
        (measured.count_total, measured.bytes_total),
        (2, bytes as u64)
    );
    let archive = archive.unwrap();
    assert_eq!(archive.test_facts(16).unwrap(), (bytes, 2));
    let fields = archive.pair().unwrap();
    assert_eq!(archive.test_facts(16).unwrap(), (bytes, 6));
    let measured = allocation_counter::measure(|| assert!(archive.pair().is_err()));
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert!(archive.test_facts(5).is_err());
    let omitted = archive.value.clone();
    assert!(archive.test_facts(16).is_err());
    drop(omitted);
    assert_eq!(archive.test_facts(16).unwrap(), (bytes, 6));
    let measured = allocation_counter::measure(|| drop(fields));
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert_eq!(archive.test_facts(16).unwrap(), (bytes, 2));
    assert!(archive.pair().is_err());
    let measured = allocation_counter::measure(|| drop(archive));
    assert_eq!(
        (measured.count_current, measured.bytes_current),
        (-2, -(bytes as i64))
    );
    eprintln!(
        "SUPPORTED actual Root Resident archive2alloc/{bytes}B; independent DATA+metadata handles2→6→2; fielddrops0alloc/free, actualRootdrop2free; omittedalias/5rows/reuse refuse; not full native admission"
    );
}
#[cfg(feature = "allocation-probe")]
#[test]
fn actual_resident_interrupted_field_release_stays_rooted_and_never_reusable() {
    let archive = ResidentArchive::new().unwrap();
    let mut fields = archive.pair().unwrap();
    let ResidentHandle::Qualified { lease, .. } = &mut fields[0] else {
        panic!("qualified")
    };
    lease.begin_actual_field_drop_on_control().unwrap();
    assert!(matches!(
        archive.test_facts(16),
        Err(AudioStorageError::UnknownStorage)
    ));
    drop(fields); // Drop cannot retry an already-started destructive step.
    assert!(matches!(
        archive.test_facts(16),
        Err(AudioStorageError::UnknownStorage)
    ));
    assert!(archive.pair().is_err());
    assert!(archive.legacy_budget_increment(false).is_none());
}
#[cfg(feature = "allocation-probe")]
#[test]
fn actual_resident_slot_foreign_parent_runtime_and_cancellation_children_never_disappear() {
    let a = ResidentArchive::new().unwrap();
    let b = ResidentArchive::new().unwrap();
    {
        let other = b.value.slot.try_lock().unwrap();
        assert!(matches!(
            a.fields(&other),
            Err(AudioStorageError::SourceMismatch)
        ));
    }
    let fields = a.pair().unwrap();
    {
        let mut slot = fields[0].slot.try_lock().unwrap();
        slot.request = Some(Request {
            token: ResidentPublicationToken(1),
            playback: playback(),
            policy: PublicationPolicy::InheritRuntime,
            context_generation: 2,
        });
    }
    assert!(a.test_facts(32).is_err()); // real legacy runtime, never raw-Arc adoption
    let request = { fields[0].slot.try_lock().unwrap().request.take() };
    drop(request); // actual negative fixture cleanup AFTER exclusion ends
    assert!(a.test_facts(32).is_ok());
    {
        let mut slot = fields[0].slot.try_lock().unwrap();
        slot.response = Some(ResidentPublicationResponse {
            token: ResidentPublicationToken(1),
            result: Err(PublishError {
                reason: "test refusal",
                playback: playback(),
            }),
        });
    }
    assert!(a.test_facts(32).is_err());
    let response = { fields[0].slot.try_lock().unwrap().response.take() };
    drop(response);
    {
        fields[0].slot.try_lock().unwrap().cancellation =
            Some(Arc::new(FreshRuntimeCancellation {
                revision: 3,
                decision: AtomicU32::new(FRESH_PUBLICATION_PENDING),
            }));
    }
    assert!(a.test_facts(32).is_err()); // actual owning Arc is present, not a fake None
    let cancellation = { fields[0].slot.try_lock().unwrap().cancellation.take() };
    drop(cancellation);
    assert!(a.test_facts(32).is_ok());
    drop(fields);
}
#[cfg(feature = "allocation-probe")]
#[test]
fn actual_resident_scalar_constructor_first_repeat_and_control_recovery_have_no_hidden_allocation()
{
    let mut actual = None;
    let measured = allocation_counter::measure(|| actual = Some(ResidentForwarding::new()));
    let layout = std::alloc::Layout::array::<std::sync::atomic::AtomicUsize>(2)
        .unwrap()
        .extend(std::alloc::Layout::new::<ResidentForwarding>())
        .unwrap()
        .0
        .pad_to_align();
    assert_eq!(
        (measured.count_total, measured.bytes_total),
        (1, layout.size() as u64)
    );
    let actual = actual.unwrap();
    let measured = allocation_counter::measure(|| {
        for n in 0..128 {
            assert!(actual.virgin());
            {
                let mut held = actual.slot.try_lock().unwrap();
                assert!(actual.slot.try_lock().is_err());
                held.next = n;
            }
            assert_eq!(actual.slot.lock_recover_on_control().next, n);
        }
    });
    assert_eq!(
        (
            measured.count_total,
            measured.count_current,
            measured.bytes_total,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let measured = allocation_counter::measure(|| drop(actual));
    assert_eq!(
        (measured.count_current, measured.bytes_current),
        (-1, -(layout.size() as i64))
    );
    eprintln!(
        "SUPPORTED actual ResidentForwarding scalar constructor1alloc/{}B; first/repeat/contended try + control recovery0alloc/free without warmup; final1free. Child/handle/fullunion/Mac contention qualification separate",
        layout.size()
    );
}
#[cfg(feature = "allocation-probe")]
#[test]
fn actual_other_thread_resident_exclusion_keeps_all_client_and_worker_tries_nonblocking() {
    let (control, callback) = channel(48000).unwrap();
    let (data, errors, terminals, mut client, mut worker) =
        reserve_native_worker(control, callback, None).unwrap();
    let root = worker.control().unwrap().native_candidate_root();
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let bridge = client
        .owner_ref()
        .unwrap()
        .resident_forwarding
        .legacy_arc_for_test();
    let original = playback();
    let identity = original.prepared_revision().cloned().unwrap();
    let ready = std::sync::Barrier::new(2);
    let release = std::sync::Barrier::new(2);
    let mut refused = None;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _held = bridge.slot.try_lock().unwrap();
            ready.wait();
            release.wait();
        });
        ready.wait();
        let measured = allocation_counter::measure(|| {
            assert!(!bridge.virgin());
            refused = Some(
                client
                    .forward_resident_publication(original, PublicationPolicy::InheritRuntime, 0)
                    .unwrap_err(),
            );
            assert!(client.try_resident_publication_response().is_none());
            assert_eq!(
                client.cancel_fresh_runtime_publication(1),
                FreshRuntimeCancellationResult::Busy
            );
            worker.control().unwrap().poll_resident_publication();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        // All calls returned while the OTHER thread still holds exclusion.
        // No fairness/wait-time guarantee is inferred from this handshake.
        release.wait();
    });
    let refused = refused.unwrap();
    assert_eq!(refused.reason, "resident publication busy");
    assert!(Arc::ptr_eq(
        refused.playback.prepared_revision().unwrap(),
        &identity
    ));
    client
        .forward_resident_publication(refused.playback, PublicationPolicy::InheritRuntime, 0)
        .unwrap();
    worker.control().unwrap().poll_resident_publication();
    assert!(
        client
            .try_resident_publication_response()
            .unwrap()
            .result
            .is_ok()
    );
    drop(bridge);
    client.close();
    worker.close();
    drop(data);
    drop(errors);
    candidate_tests::finish(&mut retirement);
}
#[cfg(feature = "allocation-probe")]
#[test]
fn actual_resident_poison_keeps_client_busy_and_worker_recovery_sticky_without_allocation() {
    let (control, callback) = channel(48000).unwrap();
    let (data, errors, terminals, mut client, mut worker) =
        reserve_native_worker(control, callback, None).unwrap();
    let root = worker.control().unwrap().native_candidate_root();
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let bridge = client
        .owner_ref()
        .unwrap()
        .resident_forwarding
        .legacy_arc_for_test();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut held = bridge.slot.try_lock().unwrap();
        held.next = 17;
        std::panic::panic_any(377_u64);
    }))
    .unwrap_err();
    assert_eq!(panic.downcast_ref::<u64>(), Some(&377));
    let original = playback();
    let identity = original.prepared_revision().cloned().unwrap();
    let mut refused = None;
    let measured = allocation_counter::measure(|| {
        assert!(!bridge.virgin());
        refused = Some(
            client
                .forward_resident_publication(original, PublicationPolicy::InheritRuntime, 0)
                .unwrap_err(),
        );
        assert!(client.try_resident_publication_response().is_none());
        assert_eq!(
            client.cancel_fresh_runtime_publication(1),
            FreshRuntimeCancellationResult::Busy
        );
        worker.owner_ref().unwrap().update_resident_cancellation();
        worker.control().unwrap().poll_resident_publication();
        assert_eq!(bridge.slot.lock_recover_on_control().next, 17);
        assert!(bridge.slot.try_lock().is_err());
    });
    assert_eq!(
        (
            measured.count_total,
            measured.count_current,
            measured.bytes_total,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(Arc::ptr_eq(
        refused.unwrap().playback.prepared_revision().unwrap(),
        &identity
    ));
    drop(bridge);
    client.close();
    worker.close();
    drop(data);
    drop(errors);
    candidate_tests::finish(&mut retirement);
}
#[test]
fn one_request_preserves_refusal_owner_and_separates_request_submission_and_apply() {
    let _serial = candidate_tests::serial();
    let (control, callback) = channel(48000).unwrap();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut worker) =
        reserve_native_worker(control, callback, None).unwrap();
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let ticket = client
        .forward_resident_publication(playback(), PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    let pending = playback();
    let identity = pending.prepared_revision().cloned().unwrap();
    let refused = client
        .forward_resident_publication(pending, PublicationPolicy::InheritRuntime, 7)
        .unwrap_err();
    assert!(Arc::ptr_eq(
        refused.playback.prepared_revision().unwrap(),
        &identity
    ));
    assert_eq!(client.status().unwrap().submitted_revision.load(Acquire), 0);
    assert!(client.try_resident_publication_response().is_none());
    worker.control().unwrap().poll_resident_publication();
    let response = client.try_resident_publication_response().unwrap();
    assert_eq!(response.token, ticket);
    assert_eq!(response.result.unwrap(), 1);
    assert_eq!(client.status().unwrap().submitted_revision.load(Acquire), 1);
    assert_eq!(client.status().unwrap().applied_revision.load(Acquire), 0);
    // A second request may enter the one cell after result collection, but
    // actual queue/pending pressure still returns its exact original owner.
    let refused_ticket = client
        .forward_resident_publication(refused.playback, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    worker.control().unwrap().poll_resident_publication();
    let response = client.try_resident_publication_response().unwrap();
    assert_eq!(response.token, refused_ticket);
    assert!(Arc::ptr_eq(
        response
            .result
            .unwrap_err()
            .playback
            .prepared_revision()
            .unwrap(),
        &identity
    ));
    let render = || {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut [[0.0; 2]; 8]).unwrap();
        });
    };
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(render);
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    {
        let mut render = render;
        render();
    }
    assert_eq!(client.status().unwrap().applied_revision.load(Acquire), 1);
    assert!(client.try_resident_publication_response().is_none());
    let fence = client.capture_fresh_runtime_fence().unwrap();
    assert_eq!(
        worker.control().unwrap().capture_fresh_runtime_fence(),
        Some(fence)
    );
    let request = client
        .forward_resident_publication(
            playback(),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            7,
        )
        .unwrap();
    worker.control().unwrap().poll_resident_publication();
    let response = client.try_resident_publication_response().unwrap();
    assert_eq!(response.token, request);
    let submitted = response.result.unwrap();
    assert_eq!(
        client.cancel_fresh_runtime_publication(submitted + 1),
        FreshRuntimeCancellationResult::UnknownRevision
    );
    assert_eq!(
        client.cancel_fresh_runtime_publication(submitted),
        FreshRuntimeCancellationResult::Cancelled
    );
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert_eq!(client.status().unwrap().applied_revision.load(Acquire), 1);
    assert_eq!(
        client.status().unwrap().rejected_revision(),
        Some((submitted, PlaybackHandoffRejection::FreshRuntimeCancelled))
    );
    worker.control().unwrap().reclaim();
    let fence = client.capture_fresh_runtime_fence().unwrap();
    client
        .forward_resident_publication(
            playback(),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            7,
        )
        .unwrap();
    worker.control().unwrap().poll_resident_publication();
    let successor = client
        .try_resident_publication_response()
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        client.cancel_fresh_runtime_publication(submitted),
        FreshRuntimeCancellationResult::UnknownRevision
    );
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert_eq!(
        client.status().unwrap().applied_revision.load(Acquire),
        successor
    );
    assert_eq!(
        client.cancel_fresh_runtime_publication(successor),
        FreshRuntimeCancellationResult::Complete
    );
    worker.control().unwrap().reclaim();
    client.close();
    worker.close();
    drop(data);
    drop(errors);
    candidate_tests::finish(&mut retirement);
}
#[test]
fn contended_client_access_is_nonblocking_and_keeps_original_playback() {
    let (control, callback) = channel(48000).unwrap();
    let (data, errors, terminals, mut client, mut worker) =
        reserve_native_worker(control, callback, None).unwrap();
    let root = worker.control().unwrap().native_candidate_root();
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    // Hold the worker's real cell guard; client methods must try, not wait.
    let held = worker
        .owner_ref()
        .unwrap()
        .resident_forwarding
        .as_ref()
        .unwrap()
        .slot
        .lock_recover_on_control();
    let original = playback();
    let identity = original.prepared_revision().cloned().unwrap();
    let refused = client
        .forward_resident_publication(original, PublicationPolicy::InheritRuntime, 0)
        .unwrap_err();
    assert_eq!(refused.reason, "resident publication busy");
    assert!(Arc::ptr_eq(
        refused.playback.prepared_revision().unwrap(),
        &identity
    ));
    assert!(client.try_resident_publication_response().is_none());
    assert_eq!(
        client.cancel_fresh_runtime_publication(1),
        FreshRuntimeCancellationResult::Busy
    );
    drop(held);
    client.close();
    worker.close();
    drop(data);
    drop(errors);
    candidate_tests::finish(&mut retirement);
}

#[test]
fn actual_resident_field_roles_foreign_and_legacy_refuse_without_slot_inspection() {
    let archive = ResidentArchive::new().unwrap();
    let [client, worker] = archive.pair().unwrap();
    let foreign = ResidentArchive::new().unwrap();
    let [other_client, other_worker] = foreign.pair().unwrap();
    let legacy = ResidentHandle::Legacy(ResidentForwarding::new());
    let matching = allocation_counter::measure(|| {
        assert!(archive.matches_actual_field(&client, 0));
        assert!(archive.matches_actual_field(&worker, 1));
        assert!(!archive.matches_actual_field(&client, 1));
        assert!(!archive.matches_actual_field(&worker, 0));
        assert!(!archive.matches_actual_field(&other_client, 0));
        assert!(!archive.matches_actual_field(&other_worker, 1));
        assert!(!archive.matches_actual_field(&legacy, 0));
        assert!(!archive.matches_actual_field(&client, usize::MAX));
    });
    assert_eq!((matching.count_total, matching.count_current), (0, 0));
    // Origin matching does NOT try-lock, inspect, clear, or qualify Slot content.
    let guard = archive.value.slot.try_lock().unwrap();
    assert!(archive.matches_actual_field(&client, 0));
    drop(guard);
}
