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
    {
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
        let bank = BTreeMap::from([(
            asset,
            hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(metadata)),
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
        let binding = hex_stream::VerifiedStreamRevision::new(
            Arc::clone(&revision),
            control.catalog_lease().expect("catalog lease"),
        )
        .expect("complete graph binding");
        let metadata_owners = Arc::strong_count(binding.revision());
        let mut bindings = 0;
        let borrowed = measure(|| {
            bindings = binding.revision().streamed_sources().count();
        });
        assert_eq!(bindings, 1, "this fixture has one Session-only binding");
        assert_eq!(Arc::strong_count(binding.revision()), metadata_owners);
        assert_no_allocation(borrowed);
        let mut session = SessionPlayback::new(revision).expect("Session runtime");
        session.apply(SessionPlaybackCommand::Play).expect("play");
        let mut expected = [[0.0; 2]; 31];
        for (index, frame) in expected.iter_mut().take(20).enumerate() {
            *frame = match (4090 + index) % 4 {
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

#[test]
fn typed_endpoint_pairing_and_borrowing_introduce_no_allocations_or_frees() {
    let mut runtime = StreamRuntime::new(StreamConfig::with_pool_pages(3), Vec::new()).unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let (control, pages) = runtime.split().ok().unwrap();
    let lease = control.catalog_lease().unwrap();
    let mut inputs = Some((pages, lease));
    let mut endpoint = None;
    let info = measure(|| {
        let (pages, lease) = inputs.take().unwrap();
        endpoint = Some(hex_stream::VerifiedPageEndpoint::new(pages, lease).unwrap());
    });
    assert_no_allocation(info);
    let mut endpoint = endpoint.unwrap();
    let info = measure(|| {
        assert_eq!(endpoint.pages().pump_ready(3), 0);
        assert_eq!(endpoint.pages().stats(), Default::default());
        assert_eq!(endpoint.catalog().footprint().sources, 0);
        let report = endpoint.allocation_footprint();
        assert_eq!(report.catalog.source_slots.capacity, 0);
        assert!(report.retained_bytes().is_some());
        assert!(report.construction_peak_bound_bytes().is_some());
    });
    assert_no_allocation(info);
    let (pages, lease) = endpoint.into_parts();
    drop(lease);
    let report = control.shutdown(pages).unwrap();
    assert!(report.worker_joined && report.accounting_complete);
}

#[test]
fn real_warmed_pages_and_post_node_observation_are_allocation_and_owner_stable() {
    use hex_engine::playback::{Playback, PlaybackCommand};
    use hex_engine::{DeviceTypeId, NativeDeviceClass, PostNodeIdentity, ProcessorChainOwner};
    let wav = FixtureWav::create();
    let limits = VerifiedWavReaderLimits::new(
        WavReaderLimits::new(1024 * 1024, SOURCE_FRAMES as u64, 32, 32768),
        1024 * 1024,
    );
    let mut reader = VerifiedWavReader::open(&wav.0, limits).unwrap();
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "observed".into(),
            path: wav.0.clone(),
            source_sample_rate: RATE,
            source_frames: SOURCE_FRAMES as u64,
            playback_frames: SOURCE_FRAMES as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let mut snapshot = editor.project().snapshot();
    let device_id = snapshot.next_id;
    snapshot.next_id += 1;
    let device_type = DeviceTypeId::from_static("hex.eq8");
    snapshot.tracks[0]
        .devices
        .push(hex_project::DeviceSnapshot {
            id: device_id,
            bypassed: false,
            state: hex_engine::default_native_device_state_for_class(
                NativeDeviceClass::Effect,
                device_type,
            )
            .unwrap(),
        });
    let project = Project::from_snapshot(snapshot).unwrap();
    let metadata = Arc::new(
        StreamedSourceMetadata::new(
            StreamSourceKey::new(117, asset),
            RATE,
            RATE,
            SOURCE_FRAMES as u64,
            SOURCE_FRAMES as u64,
            hex_project::IDENTITY_CONVERSION,
            *reader.fingerprint(),
        )
        .unwrap(),
    );
    let mut runtime = StreamRuntime::new(
        StreamConfig::with_pool_pages(3),
        vec![SourceDefinition::new(
            wav.0.clone(),
            metadata.as_ref().clone(),
            limits,
        )],
    )
    .unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let (control, mut pages) = runtime.split().ok().unwrap();
    pages.request_page(metadata.key(), 0);
    assert!(wait_for_pages(
        &mut pages,
        &[hex_stream::PageKey {
            source: metadata.key(),
            page_index: 0
        }]
    ));
    let table = Arc::new(SineTable::new());
    let bank = BTreeMap::from([(
        asset,
        hex_engine::compile::PreparedAudioSource::Streamed(Arc::clone(&metadata)),
    )]);
    let revision = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &project,
            &bank,
            117,
            Arc::clone(&table),
            None,
        )
        .unwrap(),
    );
    let binding = hex_stream::VerifiedStreamRevision::new(
        Arc::clone(&revision),
        control.catalog_lease().unwrap(),
    )
    .unwrap();
    let pcm = reader.read_frames(0, SOURCE_FRAMES).unwrap();
    let resident = Arc::new(
        PreparedRevision::prepare_with_prepared_audio_sources(
            &project,
            &BTreeMap::from([(
                asset,
                hex_engine::compile::PreparedAudioSource::Resident(
                    hex_engine::Audio::new(RATE, pcm).unwrap(),
                ),
            )]),
            117,
            table,
            None,
        )
        .unwrap(),
    );
    let mut oracle = SessionPlayback::new(resident).unwrap();
    oracle.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = [[0.0; 2]; 512];
    let mut expected_nodes = [[0.0; 2]; 512];
    oracle
        .render_with_post_node_observer(
            &mut expected,
            Some(&mut |_, offset, frames: &[[f32; 2]]| {
                expected_nodes[offset..offset + frames.len()].copy_from_slice(frames)
            }),
        )
        .unwrap();
    let mut session = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    session.apply(SessionPlaybackCommand::Play).unwrap();
    let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
    playback.apply(PlaybackCommand::Play);
    let graph_owners = Arc::strong_count(&revision);
    let source_owners = Arc::strong_count(&metadata);
    let identity = PostNodeIdentity::new(
        ProcessorChainOwner::Track(track.value()),
        device_id,
        device_type,
    );
    for use_playback in [false, true] {
        let mut output = [[9.0; 2]; 512];
        let mut observed = [[0.0; 2]; 512];
        let mut calls = 0;
        let mut wrong = false;
        let mut failed = false;
        let info = measure(|| {
            let mut observer = |id, offset, frames: &[[f32; 2]]| {
                wrong |= id != identity;
                calls += 1;
                observed[offset..offset + frames.len()].copy_from_slice(frames);
            };
            let provider: &mut dyn MediaPages = &mut pages;
            failed = if use_playback {
                playback
                    .render_with_pages_and_post_node_observer(
                        &mut output,
                        provider,
                        Some(&mut observer),
                    )
                    .is_err()
            } else {
                session
                    .render_with_pages_and_post_node_observer(
                        &mut output,
                        provider,
                        Some(&mut observer),
                    )
                    .is_err()
            };
        });
        assert_no_allocation(info);
        assert!(!failed && !wrong);
        assert_eq!(calls, 2);
        assert_eq!(output, expected);
        assert_eq!(observed, expected_nodes);
        assert_eq!(Arc::strong_count(&revision), graph_owners);
        assert_eq!(Arc::strong_count(&metadata), source_owners);
    }
    assert_eq!(pages.stats().misses, 0);
    let mut calls = 0;
    let mut failed = false;
    let mut output = [[9.0; 2]; 32];
    let pause = measure(|| {
        failed |= session.apply(SessionPlaybackCommand::Pause).is_err();
        failed |= session
            .render_with_pages_and_post_node_observer(
                &mut output,
                &mut pages,
                Some(&mut |_, _, _: &[[f32; 2]]| calls += 1),
            )
            .is_err();
        failed |= session
            .render_with_pages_and_post_node_observer(
                &mut [],
                &mut pages,
                Some(&mut |_, _, _: &[[f32; 2]]| calls += 1),
            )
            .is_err();
    });
    assert_no_allocation(pause);
    assert!(!failed);
    assert_eq!(calls, 0);
    assert_eq!(output, [[0.0; 2]; 32]);
    let miss = measure(|| {
        failed |= session.apply(SessionPlaybackCommand::Seek(4096)).is_err();
        failed |= session.apply(SessionPlaybackCommand::Play).is_err();
        failed |= session
            .render_with_pages_and_post_node_observer(
                &mut output,
                &mut pages,
                Some(&mut |_, _, _: &[[f32; 2]]| calls += 1),
            )
            .is_err();
    });
    assert_no_allocation(miss);
    assert!(!failed);
    assert!(pages.stats().misses > 0);
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
    let mut legacy = Playback::from_shared(Arc::clone(revision.arrangement()));
    legacy.apply(PlaybackCommand::Play);
    let before = (legacy.position(), legacy.performance_frame(), pages.stats());
    output.fill([9.0; 2]);
    calls = 0;
    let mut refused = false;
    let refusal = measure(|| {
        refused = legacy
            .render_with_pages_and_post_node_observer(
                &mut output,
                &mut pages,
                Some(&mut |_, _, _: &[[f32; 2]]| calls += 1),
            )
            .is_err();
    });
    assert_no_allocation(refusal);
    assert!(refused);
    assert_eq!(calls, 0);
    assert_eq!(output, [[9.0; 2]; 32]);
    assert_eq!(
        (legacy.position(), legacy.performance_frame(), pages.stats()),
        before
    );
    assert_eq!(Arc::strong_count(&revision), graph_owners);
    assert_eq!(Arc::strong_count(&metadata), source_owners);
    drop(legacy);
    drop(session);
    drop(playback);
    drop(binding);
    assert!(control.shutdown(pages).unwrap().accounting_complete);
}
