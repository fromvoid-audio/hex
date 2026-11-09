//! Exact stopped source publication keeps the original installed endpoint.
use super::*;

fn paused_fixture() -> OrdinaryFixture {
    let mut f = fixture_with_quantum("processing", 0, 256);
    let playback = compile(&f, true);
    publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    physical_block::<256>(&mut f);
    replacement::wait_for_blocks(&f.control.native_candidate_root(), f.binding, 1);
    f.control.command(PlaybackCommand::Pause).unwrap();
    physical_block::<256>(&mut f);
    f
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn stopped_native_successor_inherits_without_returning_its_original_endpoint() {
    let _serial = serial();
    let mut f = paused_fixture();
    let source = f.control.status.native_effect_source().unwrap();
    let position = f.control.status.position.load(Acquire);
    let state = f.control.native_source_prepared_state(f.binding).unwrap();
    let fence = f.control.request_native_epoch_fence(source).unwrap();
    let policy = PublicationPolicy::InheritUnderStoppedNative { source, fence };
    let pending = compile(&f, true);
    assert!(
        f.control
            .publish_with_admission_and_source_on_control(
                pending,
                policy,
                context(),
                |_| Ok::<_, ()>(())
            )
            .is_err()
    );
    allocation_free(|| {
        f.data
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    assert_eq!(
        f.control.stopped_edit_fence_state(fence),
        StoppedEditFenceState::Ready
    );
    let playback = compile(&f, true);
    let next_context = hex_engine::AudioSourceContext {
        editor_revision: source.context.editor_revision + 1,
        ..source.context
    };
    let next = publish_in_context(&mut f.control, playback, policy, next_context);
    assert_eq!(
        f.control.status.applied_revision.load(Acquire),
        source.audio_revision
    );
    physical_block::<256>(&mut f);
    assert_eq!(f.control.status.applied_revision.load(Acquire), next);
    assert_eq!(
        f.control.status.native_effect_source().unwrap().context,
        next_context
    );
    assert_eq!(f.control.status.position.load(Acquire), position);
    assert_eq!(
        f.control.native_source_endpoint_status(f.binding),
        Some((true, true))
    );
    assert_eq!(
        f.control.native_source_prepared_state(f.binding).unwrap(),
        state
    );
    assert_eq!(
        f.control.stopped_edit_fence_state(fence),
        StoppedEditFenceState::Ready
    );
    f.control.release_stopped_edit_fence(fence);
    let playback = compile(&f, true);
    assert!(
        f.control
            .publish_with_admission_and_source_on_control(playback, policy, context(), |_| Ok::<
                _,
                (),
            >(
                ()
            ))
            .is_err()
    );
    let returned = dispose(f);
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
        if retired.status.success() && retired.terminal.is_some())
    );
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn stopped_native_publication_refuses_wrong_source_and_a_stop_after_admission() {
    let _serial = serial();
    let mut f = paused_fixture();
    let source = f.control.status.native_effect_source().unwrap();
    let fence = f.control.request_native_epoch_fence(source).unwrap();
    allocation_free(|| {
        f.data
            .owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block())
    });
    let wrong = PublicationPolicy::InheritUnderStoppedNative {
        source: AppliedAudioSource {
            audio_revision: source.audio_revision + 1,
            ..source
        },
        fence,
    };
    let playback = compile(&f, true);
    assert!(
        f.control
            .publish_with_admission_and_source_on_control(
                playback,
                wrong,
                context(),
                |_| Ok::<_, ()>(())
            )
            .is_err()
    );
    let playback = compile(&f, true);
    publish(
        &mut f.control,
        playback,
        PublicationPolicy::InheritUnderStoppedNative { source, fence },
    );
    // The ready fence rejects later ordinary commands before admission.
    assert!(f.control.command(PlaybackCommand::Play).is_err());
    f.control.stop();
    physical_block::<256>(&mut f);
    assert_eq!(
        f.control.status.applied_revision.load(Acquire),
        source.audio_revision
    );
    assert!(!f.control.status.playing.load(Acquire));
    assert_eq!(
        f.control.stopped_edit_fence_state(fence),
        StoppedEditFenceState::Refused
    );
    f.control.reclaim();
    f.control.release_stopped_edit_fence(fence);
    let returned = dispose(f);
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
        if retired.status.success() && retired.terminal.is_some())
    );
}
