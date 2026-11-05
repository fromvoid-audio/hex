//! Cancellation and callback ownership probe for conditional fresh publications.

use allocation_counter::measure;
use hex_audio::prepared::{self, FreshRuntimeCancellationResult, PublicationPolicy};
use hex_engine::{Audio, Clip, PlaybackHandoffRejection, Project, Track, playback::Playback};
use std::sync::atomic::Ordering::Acquire;

fn graph(sample: f32) -> Playback {
    let audio = Audio::new(48_000, vec![[sample, -sample]]).expect("audio");
    let clip = Clip::new(audio, 0, 100, true).expect("clip");
    let track = Track::new(vec![clip], 1.0, false).expect("track");
    Playback::new(Project::new(48_000, vec![track]).expect("project"))
}

fn measured_block(callback: &mut prepared::Callback, output: &mut [[f32; 2]]) {
    let mut failed = false;
    let info = measure(|| {
        callback.begin_block();
        failed = callback.render(output).is_err();
    });
    assert!(!failed, "callback returned a render error");
    assert_eq!(info.count_total, 0, "callback allocated");
    assert_eq!(info.bytes_total, 0, "callback allocated bytes");
    assert_eq!(
        info.count_current, 0,
        "callback changed live allocation count"
    );
    assert_eq!(
        info.bytes_current, 0,
        "callback changed live allocation bytes"
    );
}

fn measured_shutdown(
    control: &mut prepared::Control,
    callback: &mut prepared::Callback,
    output: &mut [[f32; 2]],
) {
    control.request_shutdown();
    measured_block(callback, output);
    assert_eq!(
        control.reclaim(),
        1,
        "shutdown returns exactly one owned graph to control"
    );
}

#[test]
fn cancelled_conditional_candidate_retires_without_callback_allocation_or_drop() {
    let (mut control, mut callback) = prepared::channel(48_000).expect("audio channel");
    control.publish(graph(0.25)).expect("initial graph");
    control
        .command_with_ticket(hex_engine::playback::PlaybackCommand::Play)
        .expect("start old graph");
    // Keep the fixture block inside the clip's audible range so the asserted
    // PCM proves the old graph stayed installed through cancellation.
    let mut output = [[0.0; 2]; 32];
    measured_block(&mut callback, &mut output);

    let fence = control
        .capture_fresh_runtime_fence()
        .expect("current empty-session route snapshot");
    let candidate = control
        .publish_with_policy(
            graph(0.91),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .expect("conditional candidate");
    assert_eq!(
        control.cancel_fresh_runtime_publication(candidate),
        FreshRuntimeCancellationResult::Cancelled
    );
    measured_block(&mut callback, &mut output);
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert_eq!(
        control.status.rejected_revision(),
        Some((candidate, PlaybackHandoffRejection::FreshRuntimeCancelled))
    );
    assert_eq!(output, [[0.25, -0.25]; 32], "old PCM remains active");

    let mut reclaimed = 0;
    let reclaim = measure(|| reclaimed = control.reclaim());
    assert_eq!(reclaimed, 1, "candidate returned to control for final drop");
    assert_eq!(reclaim.count_total, 0, "control retirement allocated");
    assert!(
        reclaim.count_current < 0,
        "control retirement performs final drop"
    );
    assert!(reclaim.bytes_current < 0);

    measured_shutdown(&mut control, &mut callback, &mut output);
}

#[test]
fn accepted_stale_backpressured_and_shutdown_candidates_remain_callback_bounded() {
    let mut output = [[0.0; 2]; 64];

    // Accepted candidate: the callback installs the prepared graph.
    let (mut control, mut callback) = prepared::channel(48_000).expect("audio channel");
    let fence = control
        .capture_fresh_runtime_fence()
        .expect("empty route snapshot");
    let accepted = control
        .publish_with_policy(
            graph(0.25),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .expect("fresh candidate");
    measured_block(&mut callback, &mut output);
    assert_eq!(control.status.applied_revision.load(Acquire), accepted);
    assert_eq!(
        control.cancel_fresh_runtime_publication(accepted),
        FreshRuntimeCancellationResult::Complete
    );
    measured_shutdown(&mut control, &mut callback, &mut output);

    // Stale fence: the old runtime remains active and the candidate is retired.
    let (mut control, mut callback) = prepared::channel(48_000).expect("audio channel");
    control.publish(graph(0.25)).expect("initial graph");
    measured_block(&mut callback, &mut output);
    let fence = control
        .capture_fresh_runtime_fence()
        .expect("active route snapshot");
    control
        .command_with_ticket(hex_engine::playback::PlaybackCommand::Seek(17))
        .expect("admit command after fence");
    let rejected = control
        .publish_with_policy(
            graph(0.91),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .expect("stale candidate");
    measured_block(&mut callback, &mut output);
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert_eq!(
        control.status.rejected_revision(),
        Some((rejected, PlaybackHandoffRejection::RuntimeFenceChanged))
    );
    assert_eq!(control.reclaim(), 1);
    measured_shutdown(&mut control, &mut callback, &mut output);

    // Full retirement ring: cancellation is remembered while the candidate is
    // queued, then its ownership returns after control frees bounded capacity.
    let (mut control, mut callback) = prepared::channel(48_000).expect("audio channel");
    for sample in [0.25, 0.5, 0.75] {
        control
            .publish(graph(sample))
            .expect("fill runtime history");
        measured_block(&mut callback, &mut output);
    }
    let fence = control
        .capture_fresh_runtime_fence()
        .expect("backpressured route snapshot");
    let cancelled = control
        .publish_with_policy(
            graph(0.91),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .expect("queued candidate");
    assert_eq!(
        control.cancel_fresh_runtime_publication(cancelled),
        FreshRuntimeCancellationResult::Cancelled
    );
    measured_block(&mut callback, &mut output);
    assert_eq!(control.status.applied_revision.load(Acquire), 3);
    assert_eq!(control.status.rejected_revision(), None);
    assert_eq!(control.reclaim(), 2);
    measured_block(&mut callback, &mut output);
    assert_eq!(
        control.status.rejected_revision(),
        Some((cancelled, PlaybackHandoffRejection::FreshRuntimeCancelled))
    );
    assert_eq!(control.reclaim(), 1);
    measured_shutdown(&mut control, &mut callback, &mut output);

    // Shutdown itself drains a queued conditional graph through the same
    // retirement owner and publishes its terminal result without dropping it.
    let (mut control, mut callback) = prepared::channel(48_000).expect("audio channel");
    let fence = control
        .capture_fresh_runtime_fence()
        .expect("empty route snapshot");
    let shutting_down = control
        .publish_with_policy(
            graph(0.25),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .expect("queued shutdown candidate");
    assert_eq!(
        control.cancel_fresh_runtime_publication(shutting_down),
        FreshRuntimeCancellationResult::Cancelled
    );
    measured_shutdown(&mut control, &mut callback, &mut output);
    assert_eq!(
        control.status.rejected_revision(),
        Some((
            shutting_down,
            PlaybackHandoffRejection::FreshRuntimeCancelled
        ))
    );

    // A shutdown drain without a UI cancellation is not mislabeled as an
    // explicit user cancellation.
    let (mut control, mut callback) = prepared::channel(48_000).expect("audio channel");
    let fence = control
        .capture_fresh_runtime_fence()
        .expect("empty route snapshot");
    control
        .publish_with_policy(
            graph(0.25),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .expect("queued shutdown candidate");
    measured_shutdown(&mut control, &mut callback, &mut output);
    assert_eq!(control.status.rejected_revision(), None);
}
