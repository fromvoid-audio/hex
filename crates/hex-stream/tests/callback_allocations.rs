//! Headless allocation probe for the split streamed-media callback endpoint.
#![cfg(feature = "allocation-probe")]

use allocation_counter::{AllocationInfo, measure};
use hex_engine::{
    Clip, LaunchQuantization, MediaPages, PreparedRevision, Project as EngineProject, Renderer,
    STREAM_PAGE_FRAMES, SessionPlayback, SessionPlaybackCommand, SineTable, StreamSourceKey,
    StreamedSourceMetadata, Track,
};
use hex_media::{VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits};
use hex_project::{Command, Created, Editor, Project};
use hex_stream::{SourceDefinition, StreamConfig, StreamRuntime};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const RATE: u32 = 48_000;
const SOURCE_FRAMES: usize = STREAM_PAGE_FRAMES * 2;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct FixtureWav(PathBuf);

impl FixtureWav {
    fn create() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hex-stream-allocation-{}-{stamp}-{sequence}.wav",
            std::process::id()
        ));
        let data_bytes = SOURCE_FRAMES * 4;
        let riff_size = 36 + data_bytes;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create allocation fixture");
        file.write_all(b"RIFF").expect("riff");
        file.write_all(&(riff_size as u32).to_le_bytes())
            .expect("riff size");
        file.write_all(b"WAVEfmt ").expect("wave format");
        file.write_all(&16_u32.to_le_bytes()).expect("format size");
        file.write_all(&1_u16.to_le_bytes()).expect("pcm format");
        file.write_all(&2_u16.to_le_bytes()).expect("channels");
        file.write_all(&RATE.to_le_bytes()).expect("sample rate");
        file.write_all(&(RATE * 4).to_le_bytes())
            .expect("byte rate");
        file.write_all(&4_u16.to_le_bytes()).expect("block align");
        file.write_all(&16_u16.to_le_bytes()).expect("bit depth");
        file.write_all(b"data").expect("data chunk");
        file.write_all(&(data_bytes as u32).to_le_bytes())
            .expect("data size");
        for index in 0..SOURCE_FRAMES {
            let (left, right) = match index % 4 {
                0 => (8_192_i16, -16_384_i16),
                1 => (-8_192, 24_576),
                2 => (16_384, 4_096),
                _ => (-16_384, -8_192),
            };
            file.write_all(&left.to_le_bytes()).expect("left PCM");
            file.write_all(&right.to_le_bytes()).expect("right PCM");
        }
        file.sync_all().expect("flush fixture");
        Self(path)
    }
}

impl Drop for FixtureWav {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn assert_no_allocation(info: AllocationInfo) {
    assert_eq!(info.count_total, 0, "endpoint allocated");
    assert_eq!(info.bytes_total, 0, "endpoint allocated bytes");
    assert_eq!(info.count_current, 0, "endpoint changed live allocations");
    assert_eq!(info.bytes_current, 0, "endpoint changed live bytes");
}

fn wait_for_pages(
    pages: &mut hex_stream::StreamCallbackPages,
    keys: &[hex_stream::PageKey],
) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        pages.pump_ready(4);
        if pages.pin_pages(keys) {
            pages.unpin_all();
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    false
}

#[test]
fn warmed_render_page_lookup_and_pending_request_coalescing_are_allocation_free() {
    let wav = FixtureWav::create();
    let limits = VerifiedWavReaderLimits::new(
        WavReaderLimits::new(1024 * 1024, SOURCE_FRAMES as u64, 32, 32_768),
        1024 * 1024,
    );
    let verified = VerifiedWavReader::open(&wav.0, limits).expect("verified fixture");
    let mut editor = Editor::new(Project::new(RATE).expect("project"), 4).expect("editor");
    let asset = match editor
        .execute(Command::AddAsset {
            path: wav.0.clone(),
            sample_rate: RATE,
            frames: SOURCE_FRAMES as u64,
        })
        .expect("asset")
    {
        Created::Asset(asset) => asset,
        other => panic!("unexpected asset result: {other:?}"),
    };
    let key = StreamSourceKey::new(91, asset);
    let metadata = StreamedSourceMetadata::new(
        key,
        RATE,
        RATE,
        SOURCE_FRAMES as u64,
        SOURCE_FRAMES as u64,
        0,
        *verified.fingerprint(),
    )
    .expect("stream metadata");
    let definition = SourceDefinition::new(wav.0.clone(), metadata.clone(), limits);
    let runtime =
        StreamRuntime::new(StreamConfig::with_pool_pages(4), vec![definition]).expect("runtime");
    let (mut control, mut pages) = runtime.split().ok().expect("split runtime");
    control
        .wait_until_ready(Duration::from_secs(2))
        .expect("catalog ready");

    let page_keys = [
        hex_stream::PageKey {
            source: key,
            page_index: 0,
        },
        hex_stream::PageKey {
            source: key,
            page_index: 1,
        },
    ];
    for page in page_keys {
        pages.request_page(page.source, page.page_index);
    }
    assert!(
        wait_for_pages(&mut pages, &page_keys),
        "worker did not prepare both pages"
    );

    {
        let clip = Clip::streamed_region(Arc::new(metadata.clone()), 0, 40, 4_090, 20, false)
            .expect("streamed clip");
        let project = EngineProject::new(
            RATE,
            vec![Track::new(vec![clip], 1.0, false).expect("track")],
        )
        .expect("engine project");
        let mut renderer = Renderer::new(&project);
        renderer.play();

        let mut expected = [[0.0; 2]; 31];
        for (index, frame) in expected.iter_mut().enumerate() {
            if index >= 20 {
                continue;
            }
            let source_index = 4_090 + index;
            let (left, right) = match source_index % 4 {
                0 => (0.25, -0.5),
                1 => (-0.25, 0.75),
                2 => (0.5, 0.125),
                _ => (-0.5, -0.25),
            };
            *frame = [left, right];
        }
        let mut actual = [[0.0; 2]; 31];
        let mut render_failed = false;
        let info = measure(|| {
            pages.pump_ready(4);
            render_failed = renderer.render_with_pages(&mut actual, &mut pages).is_err();
        });
        assert!(!render_failed, "warmed streamed render failed");
        assert_no_allocation(info);
        assert_eq!(actual, expected);
        assert!(pages.stats().misses == 0);

        let mut hit = false;
        let page_info = measure(|| hit = pages.page(key, 0).is_some());
        assert!(hit);
        assert_no_allocation(page_info);

        let pending_page = u64::MAX;
        pages.request_page(key, pending_page);
        let before = pages.stats().coalesced_requests;
        let mut coalesce_allocation = false;
        let coalesce_info = measure(|| {
            pages.request_page(key, pending_page);
            coalesce_allocation = pages.stats().coalesced_requests == before + 1;
        });
        assert!(coalesce_allocation);
        assert_no_allocation(coalesce_info);
    }
    // Session uses the same warmed worker endpoint and source pages. Graph
    // preparation, commands' fixtures, oracles and reclamation stay outside
    // the measured callback scope; launching and rendering are measured.
    for loop_range in [
        None,
        Some(hex_project::SessionAudioLoop::new(4095, 4100).unwrap()),
    ] {
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "audio".into(),
            })
            .expect("track")
        else {
            panic!("track result")
        };
        let Created::Scene(scene) = editor
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .expect("scene")
        else {
            panic!("scene result")
        };
        let Created::SessionClip(clip) = editor
            .execute(Command::CreateSessionAudioClip {
                track,
                scene,
                name: "one shot".into(),
                asset,
                source_start: 4090,
                source_end: 4110,
            })
            .expect("session clip")
        else {
            panic!("clip result")
        };
        editor
            .execute(Command::SetSessionAudioClipLoop { clip, loop_range })
            .unwrap();
        let bank = BTreeMap::from([(
            asset,
            hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(metadata.clone())),
        )]);
        let revision = Arc::new(
            PreparedRevision::prepare_with_prepared_audio_sources(
                editor.project(),
                &bank,
                91,
                Arc::new(SineTable::new()),
                None,
            )
            .expect("prepared Session"),
        );
        let mut session = SessionPlayback::new(revision).expect("Session runtime");
        session.apply(SessionPlaybackCommand::Play).expect("play");
        let mut expected = [[0.0; 2]; 31];
        for (index, frame) in expected.iter_mut().enumerate() {
            let source = match loop_range {
                Some(_) if index >= 10 => 4095 + (index - 10) % 5,
                Some(_) => 4090 + index,
                None if index < 20 => 4090 + index,
                None => continue,
            };
            *frame = match source % 4 {
                0 => [0.25, -0.5],
                1 => [-0.25, 0.75],
                2 => [0.5, 0.125],
                _ => [-0.5, -0.25],
            };
        }
        let mut actual = [[99.0; 2]; 31];
        let mut failed = false;
        let info = measure(|| {
            failed |= session
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .is_err();
            pages.pump_ready(4);
            failed |= session.render_with_pages(&mut actual, &mut pages).is_err();
        });
        assert!(!failed, "streamed Session launch/render failed");
        assert_no_allocation(info);
        assert_eq!(actual, expected);
        assert_eq!(session.performance_frame(), 31);
        assert_eq!(pages.stats().misses, 0);
    }
    let report = control.shutdown(pages).expect("measured runtime shutdown");
    assert!(report.accounting_complete);
}
