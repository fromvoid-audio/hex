//! Real worker cancellation after the verified decoder has made progress.
use super::*;
use crate::worker::{TestReadGate, TestWorkerHooks};
use hex_engine::{MediaPages, StreamedSourceMetadata};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project};

fn pump(runtime: &mut StreamRuntime) -> usize {
    #[cfg(feature = "allocation-probe")]
    {
        let count = std::cell::Cell::new(0);
        let measured = allocation_counter::measure(|| count.set(runtime.pump_ready(1)));
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        count.get()
    }
    #[cfg(not(feature = "allocation-probe"))]
    runtime.pump_ready(1)
}

#[test]
fn shutdown_during_verified_page_read_reclaims_every_page_without_publication() {
    for release_read in [true, false] {
        let wav = crate::worker::tests::Fixture::pcm16();
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(1024 * 1024, 4096, 128, 8192),
            1024 * 1024,
        );
        let mut reader = VerifiedWavReader::open(&wav.0, limits).expect("verified source");
        let fingerprint = *reader.fingerprint();
        let mut prefix_checks = 0;
        let mut prefix = [[0.0; 2]; 1];
        reader
            .read_frames_into_with_cancel(0, &mut prefix, || {
                prefix_checks += 1;
                false
            })
            .expect("calibrate a short verified read");
        let mut complete_checks = 0;
        let mut complete = vec![[0.0; 2]; 4096];
        reader
            .read_frames_into_with_cancel(0, &mut complete, || {
                complete_checks += 1;
                false
            })
            .expect("calibrate a complete verified read");
        assert!(complete_checks > prefix_checks);
        drop(reader);

        let mut editor = Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "in-flight-source.wav".into(),
                sample_rate: 48_000,
                frames: 4096,
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
            4096,
            4096,
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
            read_gate: Some(TestReadGate {
                key: crate::PageKey {
                    source,
                    page_index: 0,
                },
                // Hold inside the real reader after the short read's entire
                // checkpoint sequence, rather than before render_page starts.
                after_checks: prefix_checks + 1,
                entered: read_entered,
                release: release_read_rx,
            }),
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
        runtime.request_page(source, 0);
        resume.send(()).expect("start the real worker read");
        runtime.wake_worker();
        read_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("read progressed beyond the calibrated prefix");
        assert_eq!(pump(&mut runtime), 0);
        assert!(runtime.page(source, 0).is_none());
        assert!(matches!(
            published.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        if release_read {
            resume_read.send(()).expect("release positive control");
            assert_eq!(
                published
                    .recv_timeout(Duration::from_secs(1))
                    .expect("completed page"),
                0
            );
            assert_eq!(pump(&mut runtime), 1);
            let page = runtime.page(source, 0).expect("verified PCM");
            assert_eq!(page.frames.len(), 4096);
            for (index, frame) in page.frames.iter().enumerate() {
                let sample = index as f32 / 32768.0;
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
