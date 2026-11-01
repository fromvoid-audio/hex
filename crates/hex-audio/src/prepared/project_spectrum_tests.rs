//! Actual callback bridge, with both analyzers active and an independent PCM run.
use super::*;
use crate::spectrum_worker::{CaptureOutcome, SpectrumTargetId, spawn_native_spectrum_worker};
use hex_engine::ProjectSignal;
use std::time::Duration;

/// Hang bound for worker delivery; ordinary runs finish in milliseconds.
const DELIVERY_BOUND: Duration = Duration::from_secs(60);

#[test]
fn actual_callback_project_and_eq_capture_preserve_pcm_and_final_context_validation() {
    let initial = SpectrumStamp {
        target: SpectrumTargetId(0),
        generation: 0,
        sample_rate_hz: 48_000,
        applied_revision: 0,
    };
    let (capture, mut reader, reaper) = spawn_native_spectrum_worker(initial).unwrap();
    let (mut control, mut callback) = channel_with_spectrum(48_000, Some(capture)).unwrap();
    let (playback, identity) = super::tests::spectrum_eq8_graph_with_params(true, 0, -12.0, 1.0);
    let (mut reference, _) = super::tests::spectrum_eq8_graph_with_params(true, 0, -12.0, 1.0);
    let revision = control
        .publish_with_policy_and_context_generation(
            playback,
            PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    control
        .spectrum_target
        .select(identity, 7, revision, 48_000)
        .unwrap();
    control
        .status
        .project_spectrum
        .select(&[1], 7, revision, 48_000)
        .unwrap();
    let mut expected = [[0.0; 2]; 256];
    reference.render(&mut expected).unwrap();
    let mut actual = [[0.0; 2]; 256];
    let now = Instant::now();
    let mut run = || {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
        assert_eq!(actual, expected);
        assert!(actual.iter().flatten().any(|sample| *sample != 0.0));
        let project = callback
            .spectrum_capture
            .as_ref()
            .unwrap()
            .project
            .as_ref()
            .unwrap();
        assert_eq!(project.staged(ProjectSignal::Master).unwrap(), actual);
        // This fixture has one unity-fader track and an unsent Return. The
        // actual direct stem therefore equals Main, independently of EQ input.
        assert_eq!(project.staged(ProjectSignal::Track(1)).unwrap(), actual);
        assert_eq!(callback.spectrum_stages.get(0).unwrap().frames, actual);
        let (node, project) = callback.finish_analysis_block(now, true);
        assert_eq!(node, Some(CaptureOutcome::Queued { sequence: 0 }));
        assert_eq!(project, Some(CaptureOutcome::Queued { sequence: 0 }));
    };
    #[cfg(feature = "allocation-probe")]
    {
        let info = allocation_counter::measure(&mut run);
        assert_eq!(
            (
                info.count_total,
                info.bytes_total,
                info.count_current,
                info.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    run();
    // Exercise the complete native factory -> worker -> retained reader value
    // path on the ordinary test-thread stack. Large inline publications used to
    // overflow during factory construction before any of this PCM ran.
    // Each block carries its actual capture time, as the device callback does.
    // The worker refuses captures older than the selection's stale window, so a
    // fixed test-start stamp lost every capture whenever this thread was
    // descheduled for longer than that window (#4108). Force that stall here.
    let selection = control.status.project_spectrum.read().unwrap();
    std::thread::sleep(selection.stale_after() + Duration::from_millis(20));
    let mut captured = now;
    for sequence in 1..16 {
        reference.render(&mut expected).unwrap();
        callback.begin_block();
        callback.render(&mut actual).unwrap();
        assert_eq!(actual, expected);
        captured = Instant::now();
        assert_eq!(
            callback.finish_analysis_block(captured, true).1,
            Some(CaptureOutcome::Queued { sequence })
        );
    }
    // Like a running callback, keep delivering fresh blocks until the worker
    // analyses one inside its stale window; only a real hang reaches the bound.
    let mut next = 16;
    let deadline = Instant::now() + DELIVERY_BOUND;
    let published = loop {
        if let Some(result) = reader.try_latest_project_at(captured, selection) {
            break result;
        }
        assert!(
            Instant::now() < deadline,
            "native project result was not delivered"
        );
        if captured.elapsed() >= selection.stale_after() / 2 {
            reference.render(&mut expected).unwrap();
            callback.begin_block();
            callback.render(&mut actual).unwrap();
            assert_eq!(actual, expected);
            captured = Instant::now();
            // A stalled worker may legitimately exert backpressure.
            let sequence = match callback.finish_analysis_block(captured, true).1 {
                Some(
                    CaptureOutcome::Queued { sequence }
                    | CaptureOutcome::QueueFull { sequence }
                    | CaptureOutcome::Skipped { sequence },
                ) => sequence,
                other => panic!("unexpected project capture outcome {other:?}"),
            };
            assert_eq!(sequence, next);
            next += 1;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(published.frames().len(), 2);
    assert_eq!(published.selection, selection);
    assert!(
        published
            .frames()
            .iter()
            .all(|frame| frame.bins_dbfs.iter().all(|value| value.is_finite()))
    );
    drop(published);
    // A UI context change after rendering invalidates its staged project block.
    callback.begin_block();
    callback.render(&mut actual).unwrap();
    control.status.project_spectrum.clear().unwrap();
    let (_, project) = callback.finish_analysis_block(now, true);
    assert_eq!(project, Some(CaptureOutcome::Skipped { sequence: next }));
    assert!(
        reader
            .try_latest_project_at(now, control.status.project_spectrum.read().unwrap())
            .is_none()
    );
    // Unknown identities never acquire invented zero layers or refuse audio.
    control
        .status
        .project_spectrum
        .select(&[999], 7, revision, 48_000)
        .unwrap();
    callback.begin_block();
    callback.render(&mut actual).unwrap();
    let (_, project) = callback.finish_analysis_block(now, true);
    assert_eq!(
        project,
        Some(CaptureOutcome::Skipped { sequence: next + 1 })
    );
    // No track selection still captures the actual master.
    control
        .status
        .project_spectrum
        .select(&[], 7, revision, 48_000)
        .unwrap();
    callback.begin_block();
    callback.render(&mut actual).unwrap();
    let (_, project) = callback.finish_analysis_block(now, true);
    assert_eq!(project, Some(CaptureOutcome::Queued { sequence: next + 2 }));
    callback.begin_block();
    callback.render(&mut actual).unwrap();
    let (_, project) = callback.finish_analysis_block(now, false);
    assert_eq!(
        project,
        Some(CaptureOutcome::Skipped { sequence: next + 3 })
    );
    callback.begin_block();
    callback.render(&mut actual).unwrap();
    control.request_shutdown();
    assert!(
        matches!(
            callback.finish_analysis_block(now, true).1,
            Some(CaptureOutcome::Skipped { .. })
        ),
        "closing output cannot refresh a prior staged project result"
    );
    callback.begin_block();
    control.reclaim();
    drop(callback);
    drop(control);
    drop(reader);
    reaper.shutdown_and_join().unwrap();
}
