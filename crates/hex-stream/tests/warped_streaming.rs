//! End-to-end verified streaming tests for Beats, Complex, and ComplexPro warp modes:
//! audio equivalence against resident `hex_warp::render_with_resampler` across loop
//! wraps, checkpoint caching, backward/forward seeks, and >16.7M-frame clips.
use hex_engine::{
    STREAM_PAGE_FRAMES, StreamSourceKey, StreamedSourceMetadata,
    audio_transform::StreamedGranularRoot,
};
use hex_media::{
    ResampleKernel, ResamplePlan, ResampleVersion, VerifiedWavReader, VerifiedWavReaderLimits,
    WavReaderLimits,
};
use hex_project::{Command, Created, Editor, IDENTITY_CONVERSION, Project};
use hex_stream::{
    GranularBankAllowance, GranularPageProgress, GranularSourcePreparationLimits,
    GranularWorkerBank, GranularWorkerExitReason, GranularWorkerKey, GranularWorkerQueues,
    GranularWorkerRequest, GranularWorkerResult, GranularWorkerSeek, VerifiedGranularSource,
    run_granular_worker,
};
use hex_warp::{
    GainPoint, Input, MapSegment, Mode, Request, Resampler, SourceRegion, Tail, TimeMap,
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RATE: u32 = 16_000;
const SOURCE_FRAMES: usize = 4_096;
static SEQ: AtomicU64 = AtomicU64::new(0);

struct FixtureWav {
    path: PathBuf,
    pcm: Vec<[f32; 2]>,
}

impl FixtureWav {
    fn create(frames: usize) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hex-stream-warped-{}-{stamp}-{seq}.wav",
            std::process::id()
        ));
        let mut pcm = Vec::with_capacity(frames);
        let data_bytes = frames * 4;
        let riff_size = 36 + data_bytes;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create wav");
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(riff_size as u32).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16_u32.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&2_u16.to_le_bytes()).unwrap();
        file.write_all(&RATE.to_le_bytes()).unwrap();
        file.write_all(&(RATE * 4).to_le_bytes()).unwrap();
        file.write_all(&4_u16.to_le_bytes()).unwrap();
        file.write_all(&16_u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&(data_bytes as u32).to_le_bytes()).unwrap();
        for i in 0..frames {
            let t = i as f64 / f64::from(RATE);
            let tone = 0.28 * (std::f64::consts::TAU * 130.0 * t).sin()
                + 0.14 * (std::f64::consts::TAU * 390.0 * t).cos();
            let beat = i % 1024;
            let hit = if beat < 48 {
                let env = (-f64::from(beat as u32) / 12.0).exp();
                0.55 * env * if beat % 2 == 0 { 1.0 } else { -0.75 }
            } else {
                0.0
            };
            let l_i16 = ((tone + hit).clamp(-0.99, 0.99) * 32767.0).round() as i16;
            let r_i16 = ((tone * -0.75 + hit * 0.8).clamp(-0.99, 0.99) * 32767.0).round() as i16;
            file.write_all(&l_i16.to_le_bytes()).unwrap();
            file.write_all(&r_i16.to_le_bytes()).unwrap();
            pcm.push([f32::from(l_i16) / 32768.0, f32::from(r_i16) / 32768.0]);
        }
        file.sync_all().unwrap();
        Self { path, pcm }
    }
}

impl Drop for FixtureWav {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn open_reader_and_plan(
    wav: &FixtureWav,
) -> (VerifiedWavReader, ResamplePlan, StreamedSourceMetadata) {
    let limits = VerifiedWavReaderLimits::new(
        WavReaderLimits::new(1024 * 1024, wav.pcm.len() as u64, 32, 32_768),
        1024 * 1024,
    );
    let reader = VerifiedWavReader::open(&wav.path, limits).unwrap();
    let kernel = ResampleKernel::with_version(RATE, RATE, ResampleVersion::V1).unwrap();
    let plan = ResamplePlan::with_kernel(
        kernel,
        wav.pcm.len() as u64,
        wav.pcm.len() as u64,
        16_384,
        STREAM_PAGE_FRAMES,
    )
    .unwrap();
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 4).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: wav.path.clone(),
            sample_rate: RATE,
            frames: wav.pcm.len() as u64,
        })
        .unwrap()
    else {
        panic!("asset");
    };
    let raw = StreamedSourceMetadata::new(
        StreamSourceKey::new(11, asset),
        RATE,
        RATE,
        wav.pcm.len() as u64,
        wav.pcm.len() as u64,
        IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .unwrap();
    (reader, plan, raw)
}

fn render_page(
    source: &mut VerifiedGranularSource,
    page_index: u64,
    frames: Box<[[f32; 2]]>,
    segmented: bool,
) -> (Box<[[f32; 2]]>, usize) {
    if segmented {
        source
            .begin_page_replaying(page_index, frames, 65_536, || false)
            .unwrap();
    } else {
        source
            .begin_page(page_index, frames, 65_536, || false)
            .unwrap();
    }
    loop {
        if source.step(|| false).unwrap() == GranularPageProgress::Complete {
            break;
        }
    }
    let completed = source.take_complete().expect("completed page");
    assert_eq!(completed.page_index, page_index);
    (completed.frames, completed.valid_frames)
}

#[test]
fn beats_complex_and_complex_pro_stream_match_resident_across_loops_and_seeks() {
    let wav = FixtureWav::create(SOURCE_FRAMES);
    type StreamCase<'a> = (Mode, i16, u64, u64, bool, Resampler, &'a [u64]);
    let cases: &[StreamCase<'_>] = &[
        (
            Mode::Beats {
                division_frames: None,
                tail: Tail::Forward,
                decay: 100,
            },
            0,
            10_000,
            8_000,
            true,
            Resampler::V1,
            &[],
        ),
        (
            Mode::Beats {
                division_frames: None,
                tail: Tail::Alternating,
                decay: 70,
            },
            150,
            10_000,
            7_500,
            true,
            Resampler::V2,
            &[],
        ),
        (
            Mode::Beats {
                division_frames: Some(1024),
                tail: Tail::Off,
                decay: 100,
            },
            0,
            9_000,
            7_000,
            true,
            Resampler::V1,
            &[],
        ),
        (Mode::Complex, 300, 10_000, 8_000, true, Resampler::V1, &[]),
        (
            Mode::ComplexPro {
                formants: 75,
                envelope: 128,
            },
            -200,
            10_000,
            8_000,
            true,
            Resampler::V2,
            &[],
        ),
    ];

    for &(mode, pitch, out_len, src_len, looping, law, attacks) in cases {
        let (reader, plan, raw) = open_reader_and_plan(&wav);
        let gain = vec![
            GainPoint {
                source_frame: 0,
                gain_q16: 65_536,
            },
            GainPoint {
                source_frame: SOURCE_FRAMES as u64,
                gain_q16: 49_152,
            },
        ];
        let region = SourceRegion::new(
            SOURCE_FRAMES as u64,
            0,
            SOURCE_FRAMES as u64,
            0,
            looping,
            gain.clone(),
        )
        .unwrap();
        let map = TimeMap::new_streamed(
            vec![MapSegment {
                output_start: 0,
                output_end: out_len,
                source_start_q32: 0,
                source_end_q32: i128::from(src_len) << 32,
            }],
            out_len,
        )
        .unwrap();
        let expected = hex_warp::render_with_resampler(
            &Request {
                attacks,
                input: Input {
                    frames: &wav.pcm,
                    sample_rate: RATE,
                    region_start: 0,
                    region_length: SOURCE_FRAMES as u64,
                    phase: 0,
                    looping,
                    envelope: &gain,
                },
                map: &map,
                output_frames: out_len as usize,
                pitch_cents: pitch,
                mode,
            },
            law,
            &mut || false,
        )
        .unwrap()
        .frames;

        let root =
            StreamedGranularRoot::new_warped(&raw, region, map, mode, pitch, law, attacks.to_vec())
                .unwrap();
        let limits = GranularSourcePreparationLimits {
            source_scratch_frames: 16_384,
            checkpoint_entries: 4,
            checkpoint_budget: 8 * 1024 * 1024,
            max_processing_bytes: 24 * 1024 * 1024,
            max_construction_peak_bytes: 128 * 1024 * 1024,
        };
        let mut source = VerifiedGranularSource::prepare_from_reader_on_control(
            raw,
            root,
            reader,
            plan,
            limits,
            || false,
        )
        .unwrap();
        assert!(source.reserved_processing_bytes() <= hex_warp::MAX_SCRATCH_BYTES);

        let mut buf = vec![[0.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
        // Forward sequential across pages 0, 1, 2 (spans loop wraps in 4096-frame source).
        for page_index in [0_u64, 1, 2] {
            let (returned, valid) = render_page(&mut source, page_index, buf, false);
            buf = returned;
            let start = page_index as usize * STREAM_PAGE_FRAMES;
            assert_eq!(valid, (out_len as usize - start).min(STREAM_PAGE_FRAMES));
            assert_eq!(
                &buf[..valid],
                &expected[start..start + valid],
                "sequential page {page_index} mode={mode:?}"
            );
        }
        // Backward seek to page 0 (restores pinned initial checkpoint) then forward seek to page 2!
        let (returned, valid) = render_page(&mut source, 0, buf, false);
        buf = returned;
        assert_eq!(
            &buf[..valid],
            &expected[..valid],
            "seek back page 0 {mode:?}"
        );
        let (returned, valid) = render_page(&mut source, 2, buf, true);
        assert_eq!(
            &returned[..valid],
            &expected[2 * STREAM_PAGE_FRAMES..2 * STREAM_PAGE_FRAMES + valid],
            "seek forward page 2 {mode:?}"
        );
    }
}

#[test]
fn long_warped_clips_above_max_output_frames_stream_through_worker_bank() {
    let wav = FixtureWav::create(SOURCE_FRAMES);
    let long_out = 20_000_000_u64;
    let long_src = 16_000_000_u64;
    assert!(long_out as usize > hex_warp::MAX_OUTPUT_FRAMES);

    let modes = [
        Mode::Beats {
            division_frames: None,
            tail: Tail::Forward,
            decay: 100,
        },
        Mode::Complex,
        Mode::ComplexPro {
            formants: 70,
            envelope: 128,
        },
    ];
    let mut sources = Vec::new();
    let mut keys = Vec::new();
    let mut expected_prefixes = Vec::new();
    for mode in modes {
        let (reader, plan, raw) = open_reader_and_plan(&wav);
        let region = SourceRegion::new(
            SOURCE_FRAMES as u64,
            0,
            SOURCE_FRAMES as u64,
            0,
            true,
            vec![],
        )
        .unwrap();
        let map = TimeMap::new_streamed(
            vec![MapSegment {
                output_start: 0,
                output_end: long_out,
                source_start_q32: 0,
                source_end_q32: i128::from(long_src) << 32,
            }],
            long_out,
        )
        .unwrap();
        let prefix_map = TimeMap::new(
            vec![MapSegment {
                output_start: 0,
                output_end: 10_000,
                source_start_q32: 0,
                source_end_q32: 8_000_i128 << 32,
            }],
            10_000,
        )
        .unwrap();
        let expected = hex_warp::render_with_resampler(
            &Request {
                attacks: &[],
                input: Input {
                    frames: &wav.pcm,
                    sample_rate: RATE,
                    region_start: 0,
                    region_length: SOURCE_FRAMES as u64,
                    phase: 0,
                    looping: true,
                    envelope: &[],
                },
                map: &prefix_map,
                output_frames: 10_000,
                pitch_cents: 0,
                mode,
            },
            Resampler::V1,
            &mut || false,
        )
        .unwrap()
        .frames;
        let root =
            StreamedGranularRoot::new_warped(&raw, region, map, mode, 0, Resampler::V1, vec![])
                .unwrap();
        assert_eq!(root.metadata().playback_frames(), long_out);
        let source = VerifiedGranularSource::prepare_from_reader_on_control(
            raw,
            root,
            reader,
            plan,
            GranularSourcePreparationLimits {
                source_scratch_frames: 16_384,
                checkpoint_entries: 4,
                checkpoint_budget: 4 * 1024 * 1024,
                max_processing_bytes: 16 * 1024 * 1024,
                max_construction_peak_bytes: 128 * 1024 * 1024,
            },
            || false,
        )
        .unwrap();
        keys.push(source.metadata().key());
        sources.push(source);
        expected_prefixes.push(expected);
    }

    let bank = GranularWorkerBank::try_new_on_control(
        sources,
        GranularBankAllowance {
            max_sources: 4,
            max_data_bytes: 64 * 1024 * 1024,
        },
    )
    .unwrap();
    let (mut req_tx, req_rx) = rtrb::RingBuffer::new(4);
    let (res_tx, mut res_rx) = rtrb::RingBuffer::new(4);
    let stop = Arc::new(AtomicBool::new(false));
    let epoch = Arc::new(AtomicU64::new(1));
    let worker_stop = Arc::clone(&stop);
    let worker_epoch = Arc::clone(&epoch);
    let join = thread::spawn(move || {
        run_granular_worker(
            bank,
            GranularWorkerQueues {
                requests: req_rx,
                results: res_tx,
            },
            &worker_stop,
            &worker_epoch,
        )
    });

    let mut buf = vec![[0.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
    for (idx, (&key, expected)) in keys.iter().zip(expected_prefixes.iter()).enumerate() {
        for page_index in [0_u64, 1, 0] {
            assert!(
                req_tx
                    .push(GranularWorkerRequest {
                        key: GranularWorkerKey {
                            source: key,
                            epoch: 1,
                            page_index,
                            seek: GranularWorkerSeek::Segmented {
                                segment_frames: 65_536,
                            },
                        },
                        frames: buf,
                    })
                    .is_ok(),
                "submit"
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            let outcome = loop {
                if let Ok(o) = res_rx.pop() {
                    break o;
                }
                assert!(Instant::now() < deadline, "worker timed out for mode {idx}");
                thread::sleep(Duration::from_millis(1));
            };
            let GranularWorkerResult::Ready(page) = outcome.result else {
                panic!("worker failed for mode {idx} page {page_index}");
            };
            assert_eq!(page.valid_frames, STREAM_PAGE_FRAMES);
            let start = page_index as usize * STREAM_PAGE_FRAMES;
            assert_eq!(
                &page.frames[..STREAM_PAGE_FRAMES],
                &expected[start..start + STREAM_PAGE_FRAMES],
                "worker mismatch mode={idx} page={page_index}"
            );
            buf = page.frames;
        }
    }
    stop.store(true, Ordering::Release);
    join.thread().unpark();
    let exit = join.join().unwrap();
    assert_eq!(exit.reason, GranularWorkerExitReason::Stopped);
}

#[test]
fn beats_checkpoint_cache_record_and_refresh_respects_tight_budget() {
    use hex_stream::{
        CheckpointCacheError, CheckpointRecord, VerifiedGranularBuffers,
        VerifiedGranularCheckpointCache, VerifiedGranularCursor, VerifiedReferenceChannel,
    };
    use hex_warp::{
        Error, MAX_SCRATCH_BYTES, SequentialBeats, SequentialBeatsConfig, SequentialWarp,
    };

    let wav = FixtureWav::create(SOURCE_FRAMES);
    let (mut reader, plan, raw) = open_reader_and_plan(&wav);
    let out_len = 12_288_u64;
    let map = TimeMap::new_streamed(
        vec![MapSegment {
            output_start: 0,
            output_end: out_len,
            source_start_q32: 0,
            source_end_q32: i128::from((SOURCE_FRAMES as u64) * 2) << 32,
        }],
        out_len,
    )
    .unwrap();
    let region = SourceRegion::new(
        SOURCE_FRAMES as u64,
        0,
        SOURCE_FRAMES as u64,
        0,
        true,
        Vec::new(),
    )
    .unwrap();
    let mode = Mode::Beats {
        division_frames: None,
        tail: Tail::Forward,
        decay: 55,
    };
    let reference = VerifiedReferenceChannel::for_warp_region(raw, region).unwrap();
    let beats = SequentialBeats::new(
        SequentialBeatsConfig {
            map,
            sample_rate: RATE,
            mode,
            pitch_cents: 0,
            resampler: Resampler::V1,
            region_length: SOURCE_FRAMES as u64,
            looping: true,
            attacks: Vec::new(),
        },
        &mut || false,
    )
    .unwrap();
    let mut cursor =
        VerifiedGranularCursor::new_warp(SequentialWarp::from_beats(beats), reference).unwrap();

    // Measure the exact initial single-checkpoint cache footprint and pick a
    // tight budget allowing the initial entry plus one moderately grown slot,
    // while leaving less than 2 full slots so LRU refresh is forced under
    // dynamic onset/anchor growth.
    let probe_cache = VerifiedGranularCheckpointCache::new(&cursor, 3, 2_000_000).unwrap();
    let initial_cache_bytes = probe_cache.backing_bytes().unwrap();
    let single_cp_bytes = cursor
        .warp_checkpoint(MAX_SCRATCH_BYTES)
        .unwrap()
        .backing_bytes()
        .unwrap();
    // Budget fits initial cache + 1 slot + small growth slack, NOT 2 full extra slots.
    let tight_budget = initial_cache_bytes + single_cp_bytes + 4_096;
    let mut cache = VerifiedGranularCheckpointCache::new(&cursor, 3, tight_budget).unwrap();
    assert!(cache.backing_bytes().unwrap() <= tight_budget);

    let mut source_scratch = vec![[0.0; 2]; 16_384];
    let mut cache_scratch = vec![[0.0; 2]; 16_384];
    let mut grain_scratch = vec![[0.0; 2]; cursor.max_source_frames()];
    let mut out = vec![[0.0; 2]; 512];

    let mut saw_added = false;
    let mut saw_replaced_or_refused = false;
    for step in 0..16 {
        let mut drained = 0;
        while drained < out.len() {
            let chunk = cursor.available().min(out.len() - drained);
            if chunk > 0 {
                cursor
                    .drain(&mut out[drained..drained + chunk])
                    .expect("drain");
                drained += chunk;
                continue;
            }
            cursor
                .advance(
                    &mut reader,
                    &plan,
                    VerifiedGranularBuffers {
                        source: &mut source_scratch,
                        cache: &mut cache_scratch,
                        grain: &mut grain_scratch,
                    },
                    || false,
                )
                .expect("advance beats cursor");
        }
        let before_bytes = cache.backing_bytes().unwrap();
        assert!(before_bytes <= tight_budget);
        match cache.record(&cursor) {
            Ok(CheckpointRecord::Added) => saw_added = true,
            Ok(CheckpointRecord::Replaced { .. }) => saw_replaced_or_refused = true,
            Ok(CheckpointRecord::Existing) => {}
            Err(CheckpointCacheError::Warp(Error::ScratchLimit)) => {
                // Tight budget refused growth cleanly without corrupting or
                // underflowing existing cache accounting.
                assert_eq!(cache.backing_bytes().unwrap(), before_bytes);
                saw_replaced_or_refused = true;
            }
            Err(other) => panic!("unexpected record error at step {step}: {other:?}"),
        }
        assert!(
            cache.backing_bytes().unwrap() <= tight_budget,
            "cache backing bytes {} exceeded tight budget {} at step {step}",
            cache.backing_bytes().unwrap(),
            tight_budget
        );
    }
    assert!(saw_added, "expected at least one checkpoint added");
    assert!(
        saw_replaced_or_refused,
        "expected tight budget to exercise LRU refresh or bounded refusal"
    );

    // Seeking back to 0 must still succeed cleanly using the intact origin slot.
    let seek = cache
        .begin_seek(&mut cursor, 0, 4096, &mut reader, &plan, || false)
        .expect("seek to origin");
    assert_eq!(
        seek,
        hex_stream::GranularSeekProgress::Reached { position: 0 }
    );
}
