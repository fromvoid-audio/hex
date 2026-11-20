//! External test-only access to a real post-admission read failure.
use hex_engine::{MediaPages, StreamSourceKey, StreamedSourceMetadata};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project};
use hex_stream::{
    PageKey, SourceDefinition, StreamConfig, StreamFailure, StreamFailureKind, StreamRuntime,
    test_support::PageGate,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Wav(PathBuf);
impl Wav {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "hex-stream-public-fault-{}-{stamp}-{}.wav",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36_u32 + 8192 * 8).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000_u32 * 8).to_le_bytes());
        bytes.extend_from_slice(&8_u16.to_le_bytes());
        bytes.extend_from_slice(&32_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(8192_u32 * 8).to_le_bytes());
        for index in 0_i32..8192 {
            bytes.extend_from_slice(&(index << 16).to_le_bytes());
            bytes.extend_from_slice(&(-index << 16).to_le_bytes());
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut file, &bytes).unwrap();
        file.sync_all().unwrap();
        Self(path)
    }
    fn definition(&self) -> SourceDefinition {
        let limits = VerifiedWavReaderLimits::new(
            WavReaderLimits::new(1024 * 1024, 8192, 32, 8192),
            1024 * 1024,
        );
        let fingerprint = *VerifiedWavReader::open(&self.0, limits)
            .unwrap()
            .fingerprint();
        let mut model = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
        let Created::Asset(asset) = model
            .execute(Command::AddAsset {
                path: self.0.clone(),
                sample_rate: 48_000,
                frames: 8192,
            })
            .unwrap()
        else {
            panic!("asset");
        };
        let metadata = StreamedSourceMetadata::new(
            StreamSourceKey::new(101, asset),
            48_000,
            48_000,
            8192,
            8192,
            hex_project::IDENTITY_CONVERSION,
            fingerprint,
        )
        .unwrap();
        SourceDefinition::new(self.0.clone(), metadata, limits)
    }
}
impl Drop for Wav {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn measured<T: Copy>(work: impl FnOnce() -> T) -> T {
    let value = std::cell::Cell::new(None);
    let count = allocation_counter::measure(|| value.set(Some(work())));
    assert_eq!(
        (
            count.count_total,
            count.bytes_total,
            count.count_current,
            count.bytes_current
        ),
        (0, 0, 0, 0)
    );
    value.into_inner().unwrap()
}
fn expect_page(runtime: &mut StreamRuntime, source: StreamSourceKey, page_index: u64) {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut failures = [StreamFailure {
        key: source.into(),
        page_index,
        kind: StreamFailureKind::Cancelled,
    }];
    while !measured(|| runtime.page(source, page_index).is_some()) {
        measured(|| runtime.pump_ready(1));
        assert_eq!(
            measured(|| runtime.take_failures(&mut failures)),
            0,
            "unused gate fault must not leak into another request: {failures:?}"
        );
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let page = runtime.page(source, page_index).unwrap();
    assert_eq!(page.page_index, page_index);
    assert_eq!(page.frames.len(), 4096);
    for (index, frame) in page.frames.iter().enumerate() {
        let sample = (index + usize::try_from(page_index).unwrap() * 4096) as f32 / 32768.0;
        assert_eq!(*frame, [sample, -sample]);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Complete,
    ReadError,
    Cancel,
    UnusedPlan,
}
#[test]
fn external_gate_preserves_real_read_failure_retry_cancel_and_pool_custody() {
    for outcome in [
        Outcome::Complete,
        Outcome::ReadError,
        Outcome::Cancel,
        Outcome::UnusedPlan,
    ] {
        let wav = Wav::new();
        let definition = wav.definition();
        let source = definition.key;
        let requested_page = if outcome == Outcome::UnusedPlan { 0 } else { 1 };
        let key = PageKey {
            source,
            page_index: requested_page,
        };
        let (gate, entered, release) = PageGate::new(key);
        let gate = if matches!(outcome, Outcome::ReadError | Outcome::UnusedPlan) {
            gate.with_read_failure_after(1)
        } else {
            gate
        };
        let mut runtime = StreamRuntime::new_with_page_gate(
            StreamConfig::with_pool_pages(4),
            vec![definition],
            gate,
        )
        .unwrap();
        runtime.wait_until_ready(Duration::from_secs(1)).unwrap();
        measured(|| runtime.request_page(source, requested_page));
        runtime.wake_worker();
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(measured(|| runtime.pump_ready(1)), 0);
        assert!(!measured(|| runtime.page(source, 1).is_some()));
        if outcome != Outcome::Cancel {
            release.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(1);
            if outcome == Outcome::ReadError {
                let mut failures = [StreamFailure {
                    key: source.into(),
                    page_index: 0,
                    kind: StreamFailureKind::Cancelled,
                }];
                while measured(|| runtime.take_failures(&mut failures)) == 0 {
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                }
                assert_eq!(
                    failures[0],
                    StreamFailure {
                        key: source.into(),
                        page_index: 1,
                        kind: StreamFailureKind::DecodeFailed
                    }
                );
                assert_eq!(measured(|| runtime.pump_ready(1)), 0);
                assert!(!measured(|| runtime.page(source, 1).is_some()));
                measured(|| runtime.request_page(source, 1));
                runtime.wake_worker();
            }
            expect_page(&mut runtime, source, requested_page);
            if outcome == Outcome::UnusedPlan {
                // This first request reads one raw page and does not consume
                // the fault. Its plan must not leak into another page request.
                measured(|| runtime.request_page(source, 1));
                runtime.wake_worker();
                expect_page(&mut runtime, source, 1);
            }
        }
        let report = runtime.shutdown().unwrap();
        assert!(report.worker_joined && report.accounting_complete);
        assert_eq!(report.page_buffers, 4);
        assert_eq!(
            report.cached_pages_returned,
            if outcome == Outcome::UnusedPlan {
                2
            } else {
                usize::from(outcome != Outcome::Cancel)
            }
        );
        assert_eq!(
            report.worker_pages_reclaimed
                + report.queued_pages_reclaimed
                + report.held_return_pages_reclaimed,
            4
        );
    }
}

#[test]
fn pitched_page_read_failure_never_publishes_and_retry_succeeds() {
    let wav = Wav::new();
    let raw = wav.definition();
    let pitched = raw
        .metadata
        .with_plain_pitch(700, hex_project::WarpResampler::V2)
        .unwrap();
    let key = PageKey {
        source: pitched.key(),
        page_index: 1,
    };
    let definition = SourceDefinition::new(raw.path.clone(), pitched, raw.reader_limits);
    let (gate, entered, release) = PageGate::new(key);
    let mut runtime = StreamRuntime::new_with_page_gate(
        StreamConfig::with_pool_pages(4),
        vec![definition],
        gate.with_read_failure_after(1),
    )
    .unwrap();
    runtime.wait_until_ready(Duration::from_secs(1)).unwrap();
    measured(|| runtime.request_page(key.source, key.page_index));
    runtime.wake_worker();
    entered.recv_timeout(Duration::from_secs(1)).unwrap();
    release.send(()).unwrap();
    let mut failures = [StreamFailure {
        key: key.source.into(),
        page_index: 0,
        kind: StreamFailureKind::Cancelled,
    }];
    let deadline = Instant::now() + Duration::from_secs(1);
    while measured(|| runtime.take_failures(&mut failures)) == 0 {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(
        failures[0],
        StreamFailure {
            key: key.source.into(),
            page_index: 1,
            kind: StreamFailureKind::DecodeFailed
        }
    );
    assert_eq!(measured(|| runtime.pump_ready(1)), 0);
    assert!(runtime.page(key.source, key.page_index).is_none());
    measured(|| runtime.request_page(key.source, key.page_index));
    runtime.wake_worker();
    let deadline = Instant::now() + Duration::from_secs(1);
    while runtime.page(key.source, key.page_index).is_none() {
        measured(|| runtime.pump_ready(1));
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let report = runtime.shutdown().unwrap();
    assert!(report.worker_joined && report.accounting_complete);
}

#[test]
fn pitched_page_cancel_while_read_is_gated_reclaims_worker() {
    let wav = Wav::new();
    let raw = wav.definition();
    let pitched = raw
        .metadata
        .with_plain_pitch(-700, hex_project::WarpResampler::V1)
        .unwrap();
    let key = PageKey {
        source: pitched.key(),
        page_index: 1,
    };
    let definition = SourceDefinition::new(raw.path.clone(), pitched, raw.reader_limits);
    let (gate, entered, _release) = PageGate::new(key);
    let mut runtime =
        StreamRuntime::new_with_page_gate(StreamConfig::with_pool_pages(4), vec![definition], gate)
            .unwrap();
    runtime.wait_until_ready(Duration::from_secs(1)).unwrap();
    measured(|| runtime.request_page(key.source, key.page_index));
    runtime.wake_worker();
    entered.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(runtime.page(key.source, key.page_index).is_none());
    let report = runtime.shutdown().unwrap();
    assert!(report.worker_joined && report.accounting_complete);
}
