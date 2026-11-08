//! Real worker completion, read failure and cancellation after decoder progress.
use super::*;
use crate::worker::{TestReadGate, TestWorkerHooks};
use hex_engine::{MediaPages, StreamedSourceMetadata};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project};
use std::time::Instant;

fn measured<T: Copy>(work: impl FnOnce() -> T) -> T {
    #[cfg(feature = "allocation-probe")]
    {
        let value = std::cell::Cell::new(None);
        let measured = allocation_counter::measure(|| value.set(Some(work())));
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        value.into_inner().expect("measured operation ran")
    }
    #[cfg(not(feature = "allocation-probe"))]
    work()
}
fn pump(runtime: &mut StreamRuntime) -> usize {
    measured(|| runtime.pump_ready(1))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadOutcome {
    Complete,
    DecodeFailed,
    Cancelled,
}

#[test]
fn verified_page_read_outcomes_preserve_pcm_retry_and_complete_pool_custody() {
    for outcome in [
        ReadOutcome::Complete,
        ReadOutcome::DecodeFailed,
        ReadOutcome::Cancelled,
    ] {
        let release_read = outcome != ReadOutcome::Cancelled;
        let wav = crate::worker::tests::Fixture::pcm32_frames(8192);
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(1024 * 1024, 8192, 128, 8192),
            1024 * 1024,
        );
        let mut reader = VerifiedWavReader::open(&wav.0, limits).expect("verified source");
        let fingerprint = *reader.fingerprint();
        let mut prefix_checks = 0;
        let mut prefix = [[0.0; 2]; 1];
        reader
            .read_frames_into_with_cancel(4096, &mut prefix, || {
                prefix_checks += 1;
                false
            })
            .expect("calibrate a short verified read");
        let mut complete_checks = 0;
        let mut complete = vec![[0.0; 2]; 4096];
        reader
            .read_frames_into_with_cancel(4096, &mut complete, || {
                complete_checks += 1;
                false
            })
            .expect("calibrate a complete verified read");
        // The hold must precede the last check, with real decoder work still
        // remaining. A 32-bit page spans four bounded decoder batches.
        assert!(complete_checks > prefix_checks + 1);
        drop(reader);

        let mut editor = Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "in-flight-source.wav".into(),
                sample_rate: 48_000,
                frames: 8192,
            })
            .expect("asset")
        else {
            panic!("expected asset");
        };
        let source = StreamSourceKey::new(107, asset);
        let metadata = StreamedSourceMetadata::new(
            source,
            48_000,
            48_000,
            8192,
            8192,
            hex_project::IDENTITY_CONVERSION,
            fingerprint,
        )
        .expect("metadata");
        let config = StreamConfig::with_pool_pages(4);
        let (entered, entered_rx) = mpsc::sync_channel(1);
        let (resume, release) = mpsc::sync_channel(1);
        let (read_entered, read_entered_rx) = mpsc::sync_channel(1);
        let (resume_read, release_read_rx) = mpsc::sync_channel(1);
        let (publications, published) = mpsc::sync_channel(1);
        let hooks = TestWorkerHooks {
            entered,
            release,
            publications,
            scalar_steps: None,
            read_gate: Some(TestReadGate::new(
                crate::PageKey {
                    source,
                    page_index: 1,
                },
                // Hold after the short read's complete checkpoint sequence,
                // before a later raw page read in the same real decoder call.
                prefix_checks + 1,
                read_entered,
                release_read_rx,
                (outcome == ReadOutcome::DecodeFailed).then_some(1),
            )),
        };
        let mut runtime = StreamRuntime::new_inner(
            config,
            vec![SourceDefinition::new(wav.0.clone(), metadata, limits)],
            Some(hooks),
            #[cfg(feature = "test-support")]
            None,
        )
        .expect("runtime");
        runtime
            .wait_until_ready(Duration::from_secs(1))
            .expect("catalog");
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker ready");
        measured(|| runtime.request_page(source, 1));
        resume.send(()).expect("start the real worker read");
        runtime.wake_worker();
        read_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("read progressed beyond the calibrated prefix");
        assert_eq!(pump(&mut runtime), 0);
        assert!(runtime.page(source, 1).is_none());
        assert!(matches!(
            published.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        if release_read {
            resume_read.send(()).expect("release positive control");
            if outcome == ReadOutcome::DecodeFailed {
                let mut failures = [StreamFailure {
                    key: source.into(),
                    page_index: 1,
                    kind: StreamFailureKind::Cancelled,
                }];
                let deadline = Instant::now() + Duration::from_secs(1);
                while measured(|| runtime.take_failures(&mut failures)) == 0 {
                    assert!(Instant::now() < deadline, "typed read failure");
                    thread::yield_now();
                }
                assert_eq!(
                    failures[0],
                    StreamFailure {
                        key: source.into(),
                        page_index: 1,
                        kind: StreamFailureKind::DecodeFailed
                    }
                );
                assert_eq!(pump(&mut runtime), 0);
                assert!(
                    !measured(|| runtime.page(source, 1).is_some()),
                    "partial read was not published"
                );
                assert!(matches!(
                    published.try_recv(),
                    Err(mpsc::TryRecvError::Empty)
                ));
                // Taking the failure releases its pending key. The reader's
                // one-shot error and in-flight gate must both allow retry.
                measured(|| runtime.request_page(source, 1));
                runtime.wake_worker();
            }
            assert_eq!(
                published
                    .recv_timeout(Duration::from_secs(1))
                    .expect("completed page"),
                0
            );
            assert_eq!(pump(&mut runtime), 1);
            let page = runtime.page(source, 1).expect("verified PCM");
            assert_eq!(page.page_index, 1);
            assert_eq!(page.frames.len(), 4096);
            for (index, frame) in page.frames.iter().enumerate() {
                let sample = (index + 4096) as f32 / 32768.0;
                assert_eq!(*frame, [sample, -sample]);
            }
        }

        let report = runtime.shutdown().expect("stop joins the held reader");
        assert!(report.worker_joined && report.accounting_complete);
        assert_eq!(report.page_buffers, 4);
        assert_eq!(report.cached_pages_returned, usize::from(release_read));
        assert_eq!(
            report.worker_pages_reclaimed
                + report.queued_pages_reclaimed
                + report.held_return_pages_reclaimed,
            4
        );
        assert!(
            matches!(published.try_recv(), Err(mpsc::TryRecvError::Disconnected)),
            "cancelled read published a page"
        );
    }
}
