//! Verified reply notification and an optimized cold-page pacing workload.
use super::*;
use crate::worker::tests::Fixture;
use hex_engine::{STREAM_PAGE_FRAMES, StreamedSourceMetadata};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project};
use std::io::{Seek, SeekFrom, Write};
use std::time::Instant;

fn source(wav: &Fixture, frames: u64) -> SourceDefinition {
    let limits = VerifiedWavReaderLimits::new(
        WavReaderLimits::new(32 * 1024 * 1024, frames, 128, 32_768),
        1024 * 1024,
    );
    let verified = VerifiedWavReader::open(&wav.0, limits).unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: wav.0.clone(),
            sample_rate: 48_000,
            frames,
        })
        .unwrap()
    else {
        panic!("expected asset");
    };
    let metadata = StreamedSourceMetadata::new(
        StreamSourceKey::new(1, asset),
        48_000,
        48_000,
        frames,
        frames,
        hex_project::IDENTITY_CONVERSION,
        *verified.fingerprint(),
    )
    .unwrap();
    SourceDefinition::new(wav.0.clone(), metadata, limits)
        .with_verified_manifest(verified.manifest().clone())
}

#[test]
fn offline_control_is_notified_of_ready_pages_and_failures() {
    let wav = Fixture::pcm16_frames(12_001);
    let definition = source(&wav, 12_001);
    let key = definition.key;
    let mut config = StreamConfig::with_pool_pages(2);
    config.wake_offline_consumer = true;
    let mut runtime = StreamRuntime::new(config, vec![definition]).unwrap();
    runtime.wait_until_ready(Duration::from_secs(1)).unwrap();

    for (index, success) in [(1, true), (3, false)] {
        runtime.request_page(key, index);
        runtime.wake_worker();
        let deadline = Instant::now() + Duration::from_millis(750);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("worker should notify control before the timeout");
            runtime.pump_ready(2);
            let mut failure = [StreamFailure {
                key: key.into(),
                page_index: 0,
                kind: StreamFailureKind::Cancelled,
            }];
            if runtime.take_failures(&mut failure) != 0 {
                assert!(!success);
                assert_eq!(failure[0].page_index, index);
                break;
            }
            if let Some(page) = runtime.page(key, index) {
                assert!(success);
                assert_eq!(page.frames.len(), STREAM_PAGE_FRAMES);
                assert_ne!(page.frames[0], [0.0; 2]);
                break;
            }
            thread::park_timeout(remaining);
        }
    }
    assert!(runtime.shutdown().unwrap().accounting_complete);
}

#[test]
#[ignore = "optimized performance workload"]
fn benchmark_offline_page_notifications() {
    const FRAMES: u64 = 48_000 * 60;
    let wav = Fixture::pcm16_frames(STREAM_PAGE_FRAMES as u16);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&wav.0)
        .unwrap();
    let bytes = (FRAMES * 4) as u32;
    file.seek(SeekFrom::Start(4)).unwrap();
    file.write_all(&(bytes + 36).to_le_bytes()).unwrap();
    file.seek(SeekFrom::Start(40)).unwrap();
    file.write_all(&bytes.to_le_bytes()).unwrap();
    file.set_len(u64::from(bytes + 44)).unwrap();
    drop(file);
    let definition = source(&wav, FRAMES);
    for _ in 0..3 {
        for notify in [false, true] {
            let mut config = StreamConfig::with_pool_pages(8);
            config.wake_offline_consumer = notify;
            let mut runtime = StreamRuntime::new(config, vec![definition.clone()]).unwrap();
            runtime.wait_until_ready(Duration::from_secs(3)).unwrap();
            let started = Instant::now();
            let mut read = 0;
            for index in 0..FRAMES.div_ceil(STREAM_PAGE_FRAMES as u64) {
                runtime.request_page(definition.key, index);
                runtime.wake_worker();
                loop {
                    runtime.pump_ready(8);
                    if let Some(page) = runtime.page(definition.key, index) {
                        read += page.frames.len() as u64;
                        break;
                    }
                    assert!(started.elapsed() < Duration::from_secs(10));
                    if notify {
                        thread::park_timeout(Duration::from_millis(1));
                    } else {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
            assert_eq!(read, FRAMES);
            eprintln!(
                "notify={notify} time_ms={:.2}",
                started.elapsed().as_secs_f64() * 1e3
            );
            assert!(runtime.shutdown().unwrap().accounting_complete);
        }
    }
}
