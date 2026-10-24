//! Actual post-node PCM from Track, fed Return and Main, independent of GUI focus.
use super::*;
use crate::spectrum_target::SpectrumTargetRequest;
use crate::spectrum_worker::{CaptureOutcome, SpectrumTargetId, spawn_native_spectrum_worker};

#[test]
fn simultaneous_eq_taps_preserve_pcm_and_validate_every_owner_without_callback_allocation() {
    let initial = SpectrumStamp {
        target: SpectrumTargetId(0),
        generation: 0,
        sample_rate_hz: 48_000,
        applied_revision: 0,
    };
    let (capture, mut reader, reaper) = spawn_native_spectrum_worker(initial).unwrap();
    let (mut control, mut callback) = channel_with_spectrum(48_000, Some(capture)).unwrap();
    let (playback, identities) = super::tests::spectrum_bus_graph();
    let (mut oracle, _) = super::tests::spectrum_bus_graph();
    let revision = control
        .publish_with_policy_and_context_generation(
            playback,
            PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    let requests = identities.map(|identity| SpectrumTargetRequest {
        identity,
        project_generation: 7,
        activation_floor: revision,
        sample_rate_hz: 48_000,
    });
    control.spectrum_target.set_targets(&requests).unwrap();
    let targets = *control.spectrum_target.desired_targets();
    let now = Instant::now();
    let mut actual = [[0.0; 2]; 256];
    let mut expected = actual;
    for sequence in 0..16 {
        oracle.render(&mut expected).unwrap();
        let mut render = || {
            callback.begin_block();
            callback.render(&mut actual).unwrap();
            assert_eq!(callback.spectrum_stages.iter().count(), 3);
            assert!(
                callback
                    .spectrum_stages
                    .iter()
                    .all(|(_, stage)| stage.complete())
            );
            assert_eq!(
                callback.finish_spectrum_block(now, true),
                Some(CaptureOutcome::Queued { sequence })
            );
        };
        #[cfg(feature = "allocation-probe")]
        {
            let info = allocation_counter::measure(&mut render);
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
        render();
        assert_eq!(
            actual, expected,
            "analyzers never alter actual processed audio"
        );
        let track = callback.spectrum_stages.get(0).unwrap();
        let send = callback.spectrum_stages.get(1).unwrap();
        let main = callback.spectrum_stages.get(2).unwrap();
        assert_eq!(main.frames, actual);
        assert!(actual.iter().flatten().any(|sample| sample.abs() > 0.001));
        for i in 0..256 {
            for channel in 0..2 {
                assert!(
                    (send.input_frames[i][channel] - track.frames[i][channel] * 0.5).abs()
                        < 0.00001
                );
                assert!(
                    (main.input_frames[i][channel]
                        - track.frames[i][channel]
                        - send.frames[i][channel])
                        .abs()
                        < 0.00001
                );
            }
        }
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let spectra = reader.try_latest_pairs_at(now, &targets);
        if spectra.len() == 3 {
            assert!(spectra.iter().all(|pair| pair.input.is_some()));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "all actual node captures reach independent FFTs"
        );
        std::thread::yield_now();
    }
    // A set change between render and final output validation rejects every
    // old staged result, while preserving the surviving targets' opaque IDs.
    callback.begin_block();
    callback.render(&mut actual).unwrap();
    control
        .spectrum_target
        .set_targets(&[requests[0], requests[2]])
        .unwrap();
    assert_eq!(
        callback.finish_spectrum_block(now, true),
        Some(CaptureOutcome::Skipped { sequence: 16 })
    );
    assert_eq!(
        reader
            .try_latest_pairs_at(now, control.spectrum_target.desired_targets())
            .len(),
        2
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
    drop(callback);
    drop(control);
    reaper.shutdown_and_join().unwrap();
}
