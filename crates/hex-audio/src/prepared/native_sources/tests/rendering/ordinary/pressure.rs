//! Actual ordinary graph attachments under publication/return backpressure.
use super::*;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_stopped_unused_endpoint_returns_before_a_fresh_source_can_install() {
    let _serial = serial();
    let mut f = fixture("processing");
    let original = f.binding;
    // Preparation has installed its actual endpoint, but no canonical graph
    // uses it yet. Stop/cancellation can win at this application handoff.
    f.control.stop();
    allocation_free(|| {
        f.data
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    assert!(!f.control.status.playing.load(Acquire));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !f.control.retire_native_source_preparation(original) {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let old = loop {
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
        f.control.reclaim();
        if let Some(returned) = f.control.recover_native_source_preparation(original) {
            break returned;
        }
        assert!(
            Instant::now() < deadline,
            "actual unused endpoint and source return"
        );
        std::thread::yield_now();
    };
    assert!(
        matches!(old.disposal, native::PreparationDisposal::Reaped(retired) if retired.status.success())
    );
    f.binding = source_for_fixture(
        &mut f.control,
        ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, 2).unwrap(),
        "processing",
    );
    assert_ne!(f.binding, original);
    f.control.publish_native_source_endpoint(f.binding).unwrap();
    let mut installed = false;
    allocation_free(|| {
        f.data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            installed = callback.has_native_source_endpoints();
        })
    });
    // Dispose real owners even on the pre-fix failure; do not strand the
    // singleton native lifecycle lane or pretend a queued endpoint was ACKed.
    dispose(f);
    assert!(
        installed,
        "a returned unused revoked cohort permanently refused a fresh original source"
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_native_graph_references_survive_full_returns_and_queued_shutdown() {
    let _serial = serial();
    let mut f = fixture("processing");
    // Keep the real graphs paused: no DSP call is necessary to test custody,
    // and priority Stop must not depend on worker completion or queue space.
    for index in 0..3 {
        let playback = compile(&f, true);
        let revision = publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
        assert_eq!(block(&mut f), [[0.0; 2]; 8]);
        assert_eq!(f.control.status.applied_revision.load(Acquire), revision);
        assert_eq!(
            f.control.unreturned_resident_graphs_on_control(),
            Some(index + 1)
        );
    }
    f.data.owner_mut().unwrap().run(|callback, _| {
        assert!(callback.retired.is_full());
        assert!(callback.has_native_source_endpoints());
    });
    let playback = compile(&f, true);
    let queued = publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    let refused = compile(&f, true);
    let original = Arc::clone(refused.prepared_revision().unwrap());
    match f.control.publish_with_admission_and_source_on_control(
        refused,
        PublicationPolicy::InheritRuntime,
        context(),
        |_| Ok::<_, ()>(()),
    ) {
        Err(AdmittedPublishError::Publication(refused)) => {
            assert_eq!(refused.reason, "playback publication already pending");
            assert!(Arc::ptr_eq(
                refused.playback.prepared_revision().unwrap(),
                &original
            ));
        }
        _ => panic!("a full publication carrier must return the original graph"),
    }
    assert_eq!(f.control.unreturned_resident_graphs_on_control(), Some(4));
    f.control.stop();
    allocation_free(|| {
        f.data
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    assert_eq!(f.control.status.pending_stop_marker.load(Acquire), 0);
    assert!(!f.control.status.playing.load(Acquire));
    assert_ne!(f.control.status.applied_revision.load(Acquire), queued);
    let deadline = Instant::now() + Duration::from_secs(3);
    while !f.control.retire_native_source_preparation(f.binding) {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(
        f.control
            .quiesce_native_source_preparation(f.binding, f.binding.graph)
            .is_err()
    );
    assert!(
        f.control
            .recover_native_source_preparation(f.binding)
            .is_none()
    );
    // Reclaim only the predecessors. The active and never-installed queued
    // graphs still retain their own references to the same original source.
    assert_eq!(f.control.reclaim(), 2);
    assert_eq!(f.control.unreturned_resident_graphs_on_control(), Some(2));
    assert!(
        f.control
            .recover_native_source_preparation(f.binding)
            .is_none()
    );
    // Close with the successor still queued: actual final backend authority
    // and ordinary resource destruction must release both original carriers.
    let returned = dispose(f);
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
        if retired.status.success() && retired.terminal.is_some_and(|terminal|
            (terminal.blocks, terminal.frames) == (0, 0)))
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn actual_ordinary_native_successor_cancellation_returns_its_graph_without_installing() {
    let _serial = serial();
    let mut f = fixture("processing");
    let playback = compile(&f, true);
    let original = publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    assert_eq!(block(&mut f), [[0.0; 2]; 8]);
    let source = f.control.status.native_effect_source().unwrap();
    f.control.stop();
    let fence = f.control.request_native_epoch_fence(source).unwrap();
    allocation_free(|| {
        f.data
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    f.control
        .return_stopped_ordinary_native_epoch_runtime(source, fence)
        .unwrap();
    for _ in 0..2 {
        allocation_free(|| {
            f.data
                .owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block())
        });
    }
    renew_cohort(&mut f);
    let playback = compile(&f, true);
    let cancelled = publish(
        &mut f.control,
        playback,
        PublicationPolicy::InstallUnderNativeEpoch(fence),
    );
    f.control.release_stopped_edit_fence(fence);
    assert_eq!(block(&mut f), [[0.0; 2]; 8]);
    assert_eq!(f.control.status.applied_revision.load(Acquire), original);
    assert_eq!(
        f.control.status.rejected_revision(),
        Some((
            cancelled,
            hex_engine::PlaybackHandoffRejection::RuntimeFenceChanged
        ))
    );
    assert!(f.control.status.native_effect_source().is_none());
    assert_eq!(f.control.unreturned_resident_graphs_on_control(), Some(1));
    f.control.reclaim();
    assert_eq!(f.control.unreturned_resident_graphs_on_control(), Some(0));
    assert!(
        f.control
            .recover_native_source_preparation(f.binding)
            .is_none()
    );
    let returned = dispose(f);
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
        if retired.status.success() && retired.terminal.is_some_and(|terminal|
            (terminal.blocks, terminal.frames) == (0, 0)))
    );
}
