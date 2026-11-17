use hex_engine::{Audio, StreamSourceKey, StreamedSourceMetadata};
use hex_media::{
    ResampleKernel, ResampleLimits, ResamplePlan, ResampleVersion, VerifiedWavReader,
    VerifiedWavReaderLimits, WavReaderLimits, resample_with_version,
};
use hex_project::{Command, Created, Editor, IDENTITY_CONVERSION, Project, SINC_CONVERSION_V2};
use hex_stream::{StreamFailureKind, scan_reference_channel_on_control};
use std::{
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Wav(PathBuf);
impl Wav {
    fn create(frames: u32, sparse: bool) -> (Self, Vec<[f32; 2]>) {
        let path = std::env::temp_dir().join(format!(
            "hex-reference-{}-{}.wav",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        let bytes = frames * 4;
        file.write_all(b"RIFF").unwrap();
        file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
        file.write_all(b"WAVEfmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&2u16.to_le_bytes()).unwrap();
        file.write_all(&48_000u32.to_le_bytes()).unwrap();
        file.write_all(&192_000u32.to_le_bytes()).unwrap();
        file.write_all(&4u16.to_le_bytes()).unwrap();
        file.write_all(&16u16.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&bytes.to_le_bytes()).unwrap();
        let mut pcm = Vec::new();
        if sparse {
            file.set_len(44 + u64::from(bytes)).unwrap();
            let count = u64::from(frames).min(4096);
            let stride = u64::from(frames) / count;
            for index in 0..count {
                file.seek(SeekFrom::Start(44 + index * stride * 4)).unwrap();
                file.write_all(&1000i16.to_le_bytes()).unwrap();
                file.write_all(&12_000i16.to_le_bytes()).unwrap();
                pcm.push([1000.0 / 32768.0, 12_000.0 / 32768.0]);
            }
        } else {
            for index in 0..frames {
                let left = ((index % 97) as i16 - 48) * 250;
                let right = ((index % 71) as i16 - 35) * 700;
                file.write_all(&left.to_le_bytes()).unwrap();
                file.write_all(&right.to_le_bytes()).unwrap();
                pcm.push([f32::from(left) / 32768.0, f32::from(right) / 32768.0]);
            }
        }
        drop(file);
        (Self(path), pcm)
    }
}
impl Drop for Wav {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn asset(frames: u32) -> hex_project::AssetId {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
    let Created::Asset(id) = editor
        .execute(Command::AddAsset {
            path: "reference.wav".into(),
            sample_rate: 48_000,
            frames: u64::from(frames),
        })
        .unwrap()
    else {
        panic!("asset")
    };
    id
}
fn setup(
    wav: &Wav,
    frames: u32,
    target_rate: u32,
    version: ResampleVersion,
) -> (VerifiedWavReader, ResamplePlan, StreamedSourceMetadata) {
    let limits = VerifiedWavReaderLimits::new(
        WavReaderLimits::new(u64::from(frames) * 4 + 44, u64::from(frames), 128, 8192),
        4 * 1024 * 1024,
    );
    let reader = VerifiedWavReader::open(&wav.0, limits).unwrap();
    let kernel = ResampleKernel::with_version(48_000, target_rate, version).unwrap();
    let plan = ResamplePlan::with_kernel(
        kernel,
        u64::from(frames),
        u64::from(frames),
        u64::from(frames) * 2,
        4096,
    )
    .unwrap();
    let conversion = if target_rate == 48_000 {
        IDENTITY_CONVERSION
    } else {
        SINC_CONVERSION_V2
    };
    let meta = StreamedSourceMetadata::new(
        StreamSourceKey::new(7, asset(frames)),
        48_000,
        target_rate,
        u64::from(frames),
        plan.output_frames(),
        conversion,
        *reader.fingerprint(),
    )
    .unwrap();
    (reader, plan, meta)
}
fn expected_channel(frames: &[[f32; 2]]) -> usize {
    let count = frames.len().min(4096);
    let stride = (frames.len() / count).max(1);
    let mut energy = [0.0f64; 2];
    for index in 0..count {
        let frame = frames[index * stride];
        for channel in 0..2 {
            let x = f64::from(frame[channel]);
            energy[channel] += x * x;
        }
    }
    usize::from(energy[1] > energy[0])
}

#[test]
fn converted_reference_matches_resident_and_refuses_wrong_inputs() {
    let (wav, pcm) = Wav::create(8192, false);
    let (mut reader, plan, meta) = setup(&wav, 8192, 44_100, ResampleVersion::V2);
    let converted = resample_with_version(
        &Audio::new(48_000, pcm).unwrap(),
        44_100,
        ResampleLimits {
            max_source_frames: 8192,
            max_output_frames: 16384,
        },
        ResampleVersion::V2,
    )
    .unwrap();
    let mut source = vec![[0.0; 2]; 8192];
    let mut output = vec![[0.0; 2]; 4096];
    let result = scan_reference_channel_on_control(
        &meta,
        &mut reader,
        &plan,
        &mut source,
        &mut output,
        || false,
    )
    .unwrap();
    assert_eq!(result.metadata(), &meta);
    assert_eq!(result.channel(), expected_channel(converted.frames()));
    let reads_before_refusals = reader.raw_page_reads();
    let derived = meta
        .with_plain_pitch(100, hex_project::WarpResampler::V2)
        .unwrap();
    assert_eq!(
        scan_reference_channel_on_control(
            &derived,
            &mut reader,
            &plan,
            &mut source,
            &mut output,
            || false
        ),
        Err(StreamFailureKind::UnknownSource)
    );
    let wrong = StreamedSourceMetadata::new(
        meta.key(),
        48_000,
        44_100,
        8192,
        plan.output_frames(),
        SINC_CONVERSION_V2,
        [0; 32],
    )
    .unwrap();
    assert_eq!(
        scan_reference_channel_on_control(
            &wrong,
            &mut reader,
            &plan,
            &mut source,
            &mut output,
            || false
        ),
        Err(StreamFailureKind::SourceChanged)
    );
    let wrong_kernel = ResampleKernel::with_version(48_000, 44_100, ResampleVersion::V1).unwrap();
    let wrong_plan = ResamplePlan::with_kernel(wrong_kernel, 8192, 8192, 16384, 4096).unwrap();
    assert_eq!(
        scan_reference_channel_on_control(
            &meta,
            &mut reader,
            &wrong_plan,
            &mut source,
            &mut output,
            || false
        ),
        Err(StreamFailureKind::UnknownSource)
    );
    assert_eq!(
        scan_reference_channel_on_control(&meta, &mut reader, &plan, &mut [], &mut output, || {
            false
        }),
        Err(StreamFailureKind::CapacityExhausted)
    );
    assert_eq!(
        scan_reference_channel_on_control(&meta, &mut reader, &plan, &mut source, &mut [], || {
            false
        }),
        Err(StreamFailureKind::CapacityExhausted)
    );
    assert_eq!(reader.raw_page_reads(), reads_before_refusals);
    let mut calls = 0;
    assert_eq!(
        scan_reference_channel_on_control(
            &meta,
            &mut reader,
            &plan,
            &mut source,
            &mut output,
            || {
                calls += 1;
                calls >= 3
            }
        ),
        Err(StreamFailureKind::Cancelled)
    );
    assert_eq!(
        scan_reference_channel_on_control(
            &meta,
            &mut reader,
            &plan,
            &mut source,
            &mut output,
            || false
        )
        .unwrap(),
        result
    );
    let reads_before_small_scratch = reader.raw_page_reads();
    assert_eq!(
        scan_reference_channel_on_control(
            &meta,
            &mut reader,
            &plan,
            &mut source[..1],
            &mut output,
            || false
        ),
        Err(StreamFailureKind::CapacityExhausted)
    );
    assert_eq!(reader.raw_page_reads(), reads_before_small_scratch);
    let file = OpenOptions::new().write(true).open(&wav.0).unwrap();
    file.set_len(44 + 8192 * 4 - 1).unwrap();
    drop(file);
    assert_eq!(
        scan_reference_channel_on_control(
            &meta,
            &mut reader,
            &plan,
            &mut source,
            &mut output,
            || false
        ),
        Err(StreamFailureKind::SourceChanged)
    );
}

#[test]
fn dense_scan_uses_multiple_small_conversion_windows_in_order() {
    let (wav, _) = Wav::create(17, false);
    let mut file = OpenOptions::new().write(true).open(&wav.0).unwrap();
    file.seek(SeekFrom::Start(44)).unwrap();
    let mut pcm = Vec::new();
    for index in 0..17 {
        let (left, right) = if index % 2 == 0 {
            (20_000i16, 0i16)
        } else {
            (0, 20_000)
        };
        file.write_all(&left.to_le_bytes()).unwrap();
        file.write_all(&right.to_le_bytes()).unwrap();
        pcm.push([f32::from(left) / 32768.0, f32::from(right) / 32768.0]);
    }
    drop(file);
    let (mut reader, plan, meta) = setup(&wav, 17, 48_000, ResampleVersion::V1);
    let mut source = [[0.0; 2]; 4];
    let mut output = [[0.0; 2]; 4];
    let result = scan_reference_channel_on_control(
        &meta,
        &mut reader,
        &plan,
        &mut source,
        &mut output,
        || false,
    )
    .unwrap();
    assert_eq!(result.channel(), expected_channel(&pcm));
    assert_eq!(result.channel(), 0);
}

#[test]
fn sparse_twenty_million_frame_source_uses_only_bounded_samples() {
    let frames = 20_000_000;
    let (wav, samples) = Wav::create(frames, true);
    let (mut reader, plan, meta) = setup(&wav, frames, 48_000, ResampleVersion::V1);
    let mut source = [[0.0; 2]; 16];
    let mut output = [[0.0; 2]; 16];
    let reads_before = reader.raw_page_reads();
    let started = std::time::Instant::now();
    let result = scan_reference_channel_on_control(
        &meta,
        &mut reader,
        &plan,
        &mut source,
        &mut output,
        || false,
    )
    .unwrap();
    eprintln!(
        "reference_scan frames={frames} rate=48000 source_scratch=16 output_scratch=16 samples=4096 elapsed_ms={} verified_page_reads={}",
        started.elapsed().as_millis(),
        reader.raw_page_reads() - reads_before
    );
    assert_eq!(result.channel(), expected_channel(&samples));
    assert_eq!(result.channel(), 1);
    let mut still_owned = [[0.0; 2]; 1];
    assert_eq!(reader.read_frames_into(0, &mut still_owned).unwrap(), 1);
    assert_eq!(still_owned[0], samples[0]);
}

fn granular_map(total: u64) -> hex_warp::TimeMap {
    hex_warp::TimeMap::new_streamed(
        vec![hex_warp::MapSegment {
            output_start: 0,
            output_end: total,
            source_start_q32: 0,
            source_end_q32: i128::from(total) << 32,
        }],
        total,
    )
    .unwrap()
}
fn granular_state(
    total: u64,
    rate: u32,
    mode: hex_warp::Mode,
    law: hex_warp::Resampler,
    channel: usize,
) -> hex_warp::SequentialGranular {
    hex_warp::SequentialGranular::new(
        hex_warp::SequentialGranularConfig {
            map: granular_map(total),
            sample_rate: rate,
            mode,
            pitch_cents: 300,
            resampler: law,
            reference_channel: channel,
        },
        &mut || false,
    )
    .unwrap()
}
fn resident_prefix(
    pcm: &[[f32; 2]],
    rate: u32,
    mode: hex_warp::Mode,
    law: hex_warp::Resampler,
) -> Vec<[f32; 2]> {
    let map = granular_map(1024);
    hex_warp::render_with_resampler(
        &hex_warp::Request {
            attacks: &[],
            input: hex_warp::Input {
                frames: pcm,
                sample_rate: rate,
                region_start: 0,
                region_length: pcm.len() as u64,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            map: &map,
            output_frames: 1024,
            pitch_cents: 300,
            mode,
        },
        law,
        &mut || false,
    )
    .unwrap()
    .frames[..256]
        .to_vec()
}
fn verified_prefix(
    state: &mut hex_warp::SequentialGranular,
    reference: &hex_stream::VerifiedReferenceChannel,
    reader: &mut VerifiedWavReader,
    plan: &ResamplePlan,
) -> Vec<[f32; 2]> {
    let mut raw = vec![[0.0; 2]; 8192];
    let cache_frames = if reference.region().is_some() {
        8192
    } else {
        2048
    };
    let mut cache = vec![[0.0; 2]; cache_frames];
    let mut grain = vec![[0.0; 2]; state.max_source_frames()];
    let mut result = vec![[0.0; 2]; 256];
    while state.position() < 256 {
        if state.available() == 0 {
            hex_stream::advance_verified_granular_on_control(
                state,
                reference,
                reader,
                plan,
                hex_stream::VerifiedGranularBuffers {
                    source: &mut raw,
                    cache: &mut cache,
                    grain: &mut grain,
                },
                || false,
            )
            .unwrap();
        }
        let start = state.position() as usize;
        let n = state.available().min(256 - start);
        state.drain(&mut result[start..start + n]).unwrap();
    }
    result
}
#[test]
fn verified_granular_prefix_matches_resident_for_long_and_converted_sources() {
    for (frames, rate, sparse) in [(20_000_000, 48_000, true), (8192, 44_100, false)] {
        let (wav, pcm) = Wav::create(frames, sparse);
        let (mut reader, plan, meta) = setup(
            &wav,
            frames,
            rate,
            if sparse {
                ResampleVersion::V1
            } else {
                ResampleVersion::V2
            },
        );
        let mut raw = vec![[0.0; 2]; 8192];
        let mut scan = vec![[0.0; 2]; 4096];
        let reference = scan_reference_channel_on_control(
            &meta,
            &mut reader,
            &plan,
            &mut raw,
            &mut scan,
            || false,
        )
        .unwrap();
        let resident = if sparse {
            let mut prefix = vec![[0.0; 2]; 4096];
            prefix[0] = pcm[0];
            prefix
        } else {
            resample_with_version(
                &Audio::new(48_000, pcm).unwrap(),
                rate,
                ResampleLimits {
                    max_source_frames: frames as usize,
                    max_output_frames: frames as usize * 2,
                },
                ResampleVersion::V2,
            )
            .unwrap()
            .frames()
            .to_vec()
        };
        let before = reader.raw_page_reads();
        let started = std::time::Instant::now();
        for mode in [
            hex_warp::Mode::Tones { grain_ms: 8 },
            hex_warp::Mode::Texture {
                grain_ms: 8,
                fluctuation: 35,
                seed: 42,
            },
        ] {
            for law in [hex_warp::Resampler::V1, hex_warp::Resampler::V2] {
                let expected = resident_prefix(&resident, rate, mode, law);
                let mut state =
                    granular_state(plan.output_frames(), rate, mode, law, reference.channel());
                let reads = reader.raw_page_reads();
                let timer = std::time::Instant::now();
                let actual = verified_prefix(&mut state, &reference, &mut reader, &plan);
                let elapsed = timer.elapsed();
                eprintln!(
                    "verified_step frames={frames} project_rate={rate} mode={mode:?} law={law:?} prefix=256 elapsed_us={} verified_reads={}",
                    elapsed.as_micros(),
                    reader.raw_page_reads() - reads
                );
                assert_eq!(actual, expected, "{frames} {mode:?} {law:?}");
            }
        }
        eprintln!(
            "verified_granular frames={frames} source_rate=48000 project_rate={rate} prefix=256 modes=2 laws=2 elapsed_ms={} verified_reads={}",
            started.elapsed().as_millis(),
            reader.raw_page_reads() - before
        );
    }
}

#[test]
fn verified_granular_refuses_mismatch_capacity_cancel_and_changed_media() {
    use hex_stream::{
        VerifiedGranularBuffers, VerifiedGranularStepError, advance_verified_granular_on_control,
    };
    let (wav, _) = Wav::create(8192, false);
    let (mut reader, plan, meta) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let mut raw = vec![[0.0; 2]; 8192];
    let mut cache = vec![[0.0; 2]; 8192];
    let reference =
        scan_reference_channel_on_control(&meta, &mut reader, &plan, &mut raw, &mut cache, || {
            false
        })
        .unwrap();
    let mode = hex_warp::Mode::Tones { grain_ms: 8 };
    let mut state = granular_state(
        plan.output_frames(),
        48_000,
        mode,
        hex_warp::Resampler::V1,
        1 - reference.channel(),
    );
    let mut grain = vec![[0.0; 2]; state.max_source_frames()];
    let before = reader.raw_page_reads();
    assert_eq!(
        advance_verified_granular_on_control(
            &mut state,
            &reference,
            &mut reader,
            &plan,
            VerifiedGranularBuffers {
                source: &mut raw,
                cache: &mut cache,
                grain: &mut grain
            },
            || false
        ),
        Err(VerifiedGranularStepError::Source(
            StreamFailureKind::UnknownSource
        ))
    );
    assert_eq!(reader.raw_page_reads(), before);
    let mut state = granular_state(
        plan.output_frames(),
        48_000,
        mode,
        hex_warp::Resampler::V1,
        reference.channel(),
    );
    assert_eq!(
        advance_verified_granular_on_control(
            &mut state,
            &reference,
            &mut reader,
            &plan,
            VerifiedGranularBuffers {
                source: &mut raw,
                cache: &mut [],
                grain: &mut grain
            },
            || false
        ),
        Err(VerifiedGranularStepError::Source(
            StreamFailureKind::CapacityExhausted
        ))
    );
    let kernel = ResampleKernel::with_version(48_000, 48_000, ResampleVersion::V2).unwrap();
    let wrong_plan = ResamplePlan::with_kernel(kernel, 8192, 8192, 16384, 4096).unwrap();
    assert_eq!(
        advance_verified_granular_on_control(
            &mut state,
            &reference,
            &mut reader,
            &wrong_plan,
            VerifiedGranularBuffers {
                source: &mut raw,
                cache: &mut cache,
                grain: &mut grain
            },
            || false
        ),
        Err(VerifiedGranularStepError::Source(
            StreamFailureKind::UnknownSource
        ))
    );
    assert_eq!(
        advance_verified_granular_on_control(
            &mut state,
            &reference,
            &mut reader,
            &plan,
            VerifiedGranularBuffers {
                source: &mut raw[..1],
                cache: &mut cache,
                grain: &mut grain
            },
            || false
        ),
        Err(VerifiedGranularStepError::Source(
            StreamFailureKind::CapacityExhausted
        ))
    );
    assert_eq!(reader.raw_page_reads(), before);
    let (other_wav, _) = Wav::create(8192, false);
    let mut other_file = OpenOptions::new().write(true).open(&other_wav.0).unwrap();
    other_file.seek(SeekFrom::Start(44)).unwrap();
    other_file.write_all(&[1, 2, 3, 4]).unwrap();
    drop(other_file);
    let (mut other_reader, _, _) = setup(&other_wav, 8192, 48_000, ResampleVersion::V1);
    let other_before = other_reader.raw_page_reads();
    assert_eq!(
        advance_verified_granular_on_control(
            &mut state,
            &reference,
            &mut other_reader,
            &plan,
            VerifiedGranularBuffers {
                source: &mut raw,
                cache: &mut cache,
                grain: &mut grain
            },
            || false
        ),
        Err(VerifiedGranularStepError::Source(
            StreamFailureKind::SourceChanged
        ))
    );
    assert_eq!(other_reader.raw_page_reads(), other_before);
    let cancelled = advance_verified_granular_on_control(
        &mut state,
        &reference,
        &mut reader,
        &plan,
        VerifiedGranularBuffers {
            source: &mut raw,
            cache: &mut cache,
            grain: &mut grain,
        },
        || true,
    );
    assert!(matches!(
        cancelled,
        Err(VerifiedGranularStepError::Warp(hex_warp::Error::Cancelled))
            | Err(VerifiedGranularStepError::Source(
                StreamFailureKind::Cancelled
            ))
    ));
    assert_eq!(state.position(), 0);
    assert!(!state.is_poisoned());
    let mut file = OpenOptions::new().write(true).open(&wav.0).unwrap();
    file.seek(SeekFrom::Start(44)).unwrap();
    file.write_all(&[7, 8, 9, 10]).unwrap();
    drop(file);
    let mut changed = false;
    for _ in 0..16 {
        match advance_verified_granular_on_control(
            &mut state,
            &reference,
            &mut reader,
            &plan,
            VerifiedGranularBuffers {
                source: &mut raw,
                cache: &mut cache,
                grain: &mut grain,
            },
            || false,
        ) {
            Err(VerifiedGranularStepError::Source(StreamFailureKind::SourceChanged)) => {
                changed = true;
                break;
            }
            Ok(true) => {
                let mut output = vec![[0.0; 2]; state.available()];
                state.drain(&mut output).unwrap();
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }
    assert!(changed);
}

fn independent_region_channel(
    pcm: &[[f32; 2]],
    start: usize,
    length: usize,
    phase: usize,
    looping: bool,
    gain: &[hex_warp::GainPoint],
) -> usize {
    let count = length.min(4096);
    let stride = (length / count).max(1);
    let mut energy = [0.0_f64; 2];
    for i in 0..count {
        let position = i * stride + phase;
        let local = if looping { position % length } else { position };
        let index = start + local;
        let next = gain.partition_point(|p| p.source_frame <= index as u64);
        let factor = match (
            next.checked_sub(1).and_then(|j| gain.get(j)),
            gain.get(next),
        ) {
            (Some(a), Some(b)) => {
                let span = b.source_frame - a.source_frame;
                let offset = index as u64 - a.source_frame;
                (u128::from(a.gain_q16) * u128::from(span - offset)
                    + u128::from(b.gain_q16) * u128::from(offset)) as f64
                    / span as f64
                    / 65_536.0
            }
            (Some(a), None) => f64::from(a.gain_q16) / 65_536.0,
            (None, Some(b)) => f64::from(b.gain_q16) / 65_536.0,
            _ => 1.0,
        };
        for channel in 0..2 {
            let v = f64::from(pcm[index][channel]) * factor;
            energy[channel] += v * v;
        }
    }
    usize::from(energy[1] > energy[0])
}

#[test]
fn verified_region_loop_and_gain_match_resident_analysis_and_pcm() {
    use hex_warp::{GainPoint, Input, Mode, Request, Resampler, SourceRegion};
    for rate in [48_000, 44_100] {
        let (wav, pcm) = Wav::create(8192, false);
        let law = if rate == 48_000 {
            ResampleVersion::V1
        } else {
            ResampleVersion::V2
        };
        let (mut reader, plan, metadata) = setup(&wav, 8192, rate, law);
        let resident = if rate == 48_000 {
            pcm
        } else {
            resample_with_version(
                &Audio::new(48_000, pcm).unwrap(),
                rate,
                ResampleLimits {
                    max_source_frames: 8192,
                    max_output_frames: 16384,
                },
                ResampleVersion::V2,
            )
            .unwrap()
            .frames()
            .to_vec()
        };
        let gain = vec![
            GainPoint {
                source_frame: 0,
                gain_q16: 65_536,
            },
            GainPoint {
                source_frame: 2000,
                gain_q16: 20_000,
            },
            GainPoint {
                source_frame: 6000,
                gain_q16: 120_000,
            },
        ];
        for looping in [false, true] {
            let phase = if looping { 4900 } else { 0 };
            let region = SourceRegion::new(
                plan.output_frames(),
                197,
                5000,
                phase,
                looping,
                gain.clone(),
            )
            .unwrap();
            let mut raw = vec![[0.0; 2]; 8192];
            let mut cache = vec![[0.0; 2]; 8192];
            let started = std::time::Instant::now();
            let before = reader.raw_page_reads();
            let reference = hex_stream::scan_verified_granular_region_on_control(
                &metadata,
                region,
                &mut reader,
                &plan,
                &mut raw,
                &mut cache,
                || false,
            )
            .unwrap();
            assert!(reference.region().is_some());
            let mut probe = granular_state(
                plan.output_frames(),
                rate,
                Mode::Tones { grain_ms: 8 },
                Resampler::V1,
                reference.channel(),
            );
            let mut grain = vec![[0.0; 2]; probe.max_source_frames()];
            let reads = reader.raw_page_reads();
            assert_eq!(
                hex_stream::advance_verified_granular_on_control(
                    &mut probe,
                    &reference,
                    &mut reader,
                    &plan,
                    hex_stream::VerifiedGranularBuffers {
                        source: &mut raw,
                        cache: &mut cache[..2048],
                        grain: &mut grain
                    },
                    || false
                ),
                Err(hex_stream::VerifiedGranularStepError::Source(
                    StreamFailureKind::CapacityExhausted
                ))
            );
            assert_eq!(reader.raw_page_reads(), reads);
            assert_eq!(
                reference.channel(),
                independent_region_channel(&resident, 197, 5000, phase as usize, looping, &gain)
            );
            for mode in [
                Mode::Tones { grain_ms: 8 },
                Mode::Texture {
                    grain_ms: 8,
                    fluctuation: 35,
                    seed: 42,
                },
            ] {
                for filter in [Resampler::V1, Resampler::V2] {
                    let map = granular_map(1024);
                    let expected = hex_warp::render_with_resampler(
                        &Request {
                            attacks: &[],
                            input: Input {
                                frames: &resident,
                                sample_rate: rate,
                                region_start: 197,
                                region_length: 5000,
                                phase,
                                looping,
                                envelope: &gain,
                            },
                            map: &map,
                            output_frames: 1024,
                            pitch_cents: 300,
                            mode,
                        },
                        filter,
                        &mut || false,
                    )
                    .unwrap();
                    let mut state = granular_state(
                        plan.output_frames(),
                        rate,
                        mode,
                        filter,
                        reference.channel(),
                    );
                    assert_eq!(
                        verified_prefix(&mut state, &reference, &mut reader, &plan),
                        expected.frames[..256],
                        "{rate} {looping} {mode:?} {filter:?}"
                    );
                }
            }
            eprintln!(
                "verified_region rate={rate} looping={looping} region=5000 phase={phase} prefix=256 modes=2 laws=2 elapsed_ms={} verified_reads={}",
                started.elapsed().as_millis(),
                reader.raw_page_reads() - before
            );
        }
    }
}

#[test]
fn verified_region_rejects_mismatched_source_extent_and_cancel() {
    let (wav, _) = Wav::create(8192, false);
    let (mut reader, plan, metadata) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let mut raw = vec![[0.0; 2]; 8192];
    let mut cache = vec![[0.0; 2]; 8192];
    let before = reader.raw_page_reads();
    let wrong = hex_warp::SourceRegion::new(8191, 0, 100, 0, false, vec![]).unwrap();
    assert_eq!(
        hex_stream::scan_verified_granular_region_on_control(
            &metadata,
            wrong,
            &mut reader,
            &plan,
            &mut raw,
            &mut cache,
            || false
        ),
        Err(hex_stream::VerifiedGranularStepError::Source(
            StreamFailureKind::UnknownSource
        ))
    );
    let region = hex_warp::SourceRegion::new(8192, 0, 100, 0, false, vec![]).unwrap();
    assert_eq!(
        hex_stream::scan_verified_granular_region_on_control(
            &metadata,
            region,
            &mut reader,
            &plan,
            &mut raw,
            &mut cache[..2048],
            || false
        ),
        Err(hex_stream::VerifiedGranularStepError::Source(
            StreamFailureKind::CapacityExhausted
        ))
    );
    let region = hex_warp::SourceRegion::new(8192, 0, 100, 0, false, vec![]).unwrap();
    assert_eq!(
        hex_stream::scan_verified_granular_region_on_control(
            &metadata,
            region,
            &mut reader,
            &plan,
            &mut raw,
            &mut cache,
            || true
        ),
        Err(hex_stream::VerifiedGranularStepError::Source(
            StreamFailureKind::Cancelled
        ))
    );
    assert_eq!(reader.raw_page_reads(), before);
}

fn cursor_read(
    cursor: &mut hex_stream::VerifiedGranularCursor,
    reader: &mut VerifiedWavReader,
    plan: &ResamplePlan,
    raw: &mut [[f32; 2]],
    cache: &mut [[f32; 2]],
    grain: &mut [[f64; 2]],
    count: usize,
) -> Vec<[f32; 2]> {
    let mut output = vec![[0.0; 2]; count];
    let mut at = 0;
    while at < count {
        if cursor.available() == 0 {
            cursor
                .advance(
                    reader,
                    plan,
                    hex_stream::VerifiedGranularBuffers {
                        source: raw,
                        cache,
                        grain,
                    },
                    || false,
                )
                .unwrap();
        }
        let n = cursor.available().min(count - at);
        cursor.drain(&mut output[at..at + n]).unwrap();
        at += n;
    }
    output
}

#[test]
fn verified_granular_seek_replays_exact_looped_pcm_in_bounded_calls() {
    use hex_stream::{
        GranularSeekProgress, VerifiedGranularBuffers, VerifiedGranularCursor,
        scan_verified_granular_region_on_control,
    };
    use hex_warp::{GainPoint, MAX_SCRATCH_BYTES, Mode, Resampler, SourceRegion};
    let (wav, pcm) = Wav::create(8192, false);
    let resident = resample_with_version(
        &Audio::new(48_000, pcm).unwrap(),
        44_100,
        ResampleLimits {
            max_source_frames: 8192,
            max_output_frames: 16384,
        },
        ResampleVersion::V2,
    )
    .unwrap();
    let (mut reader, plan, metadata) = setup(&wav, 8192, 44_100, ResampleVersion::V2);
    let mut raw = vec![[0.0; 2]; 8192];
    let mut cache = vec![[0.0; 2]; 8192];
    let gain = vec![
        GainPoint {
            source_frame: 0,
            gain_q16: 65_536,
        },
        GainPoint {
            source_frame: 6000,
            gain_q16: 16_384,
        },
    ];
    let region =
        SourceRegion::new(plan.output_frames(), 197, 5000, 4900, true, gain.clone()).unwrap();
    let reference = scan_verified_granular_region_on_control(
        &metadata,
        region,
        &mut reader,
        &plan,
        &mut raw,
        &mut cache,
        || false,
    )
    .unwrap();
    let started = std::time::Instant::now();
    let before = reader.raw_page_reads();
    for mode in [
        Mode::Tones { grain_ms: 8 },
        Mode::Texture {
            grain_ms: 8,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        for law in [Resampler::V1, Resampler::V2] {
            let map = granular_map(1024);
            let expected = hex_warp::render_with_resampler(
                &hex_warp::Request {
                    attacks: &[],
                    input: hex_warp::Input {
                        frames: resident.frames(),
                        sample_rate: 44_100,
                        region_start: 197,
                        region_length: 5000,
                        phase: 4900,
                        looping: true,
                        envelope: &gain,
                    },
                    map: &map,
                    output_frames: 1024,
                    pitch_cents: 300,
                    mode,
                },
                law,
                &mut || false,
            )
            .unwrap()
            .frames;
            let state =
                granular_state(plan.output_frames(), 44_100, mode, law, reference.channel());
            let mut cursor = VerifiedGranularCursor::new(state, reference.clone()).unwrap();
            let mut grain = vec![[0.0; 2]; cursor.max_source_frames()];
            let start = cursor.checkpoint(MAX_SCRATCH_BYTES).unwrap();
            let first = cursor_read(
                &mut cursor,
                &mut reader,
                &plan,
                &mut raw,
                &mut cache,
                &mut grain,
                512,
            );
            let middle = cursor.checkpoint(MAX_SCRATCH_BYTES).unwrap();
            let later = cursor_read(
                &mut cursor,
                &mut reader,
                &plan,
                &mut raw,
                &mut cache,
                &mut grain,
                512,
            );
            assert_eq!(first, expected[..512]);
            assert_eq!(later, expected[512..]);
            let mut discard = [[0.0; 2]; 31];
            assert!(matches!(
                cursor.begin_seek(&middle, 520, 8, &mut reader, &plan, || false),
                Ok(GranularSeekProgress::Pending { position: 512 })
            ));
            let kernel = ResampleKernel::with_version(48_000, 44_100, ResampleVersion::V1).unwrap();
            let wrong = ResamplePlan::with_kernel(kernel, 8192, 8192, 16384, 4096).unwrap();
            assert_eq!(
                cursor.continue_seek(
                    &mut reader,
                    &wrong,
                    VerifiedGranularBuffers {
                        source: &mut raw,
                        cache: &mut cache,
                        grain: &mut grain
                    },
                    &mut discard,
                    1,
                    || false
                ),
                Err(hex_stream::VerifiedGranularStepError::Source(
                    StreamFailureKind::UnknownSource
                ))
            );
            assert_eq!(
                cursor.continue_seek(
                    &mut reader,
                    &plan,
                    VerifiedGranularBuffers {
                        source: &mut raw,
                        cache: &mut cache,
                        grain: &mut grain
                    },
                    &mut discard,
                    1,
                    || true
                ),
                Err(hex_stream::VerifiedGranularStepError::Source(
                    StreamFailureKind::Cancelled
                ))
            );
            assert_eq!(cursor.position(), 512);
            let old_position = cursor.position();
            assert!(matches!(
                cursor.begin_seek(&middle, 511, 1, &mut reader, &plan, || false),
                Err(hex_stream::VerifiedGranularStepError::Warp(
                    hex_warp::Error::Invalid(_)
                ))
            ));
            assert_eq!(cursor.position(), old_position);
            let seek_reads = reader.raw_page_reads();
            let seek_started = std::time::Instant::now();
            assert_eq!(
                cursor
                    .begin_seek(&start, 256, 256, &mut reader, &plan, || false)
                    .unwrap(),
                GranularSeekProgress::Pending { position: 0 }
            );
            assert_eq!(cursor.available(), 0);
            assert!(cursor.drain(&mut []).is_err());
            let mut pending = 0;
            loop {
                match cursor
                    .continue_seek(
                        &mut reader,
                        &plan,
                        VerifiedGranularBuffers {
                            source: &mut raw,
                            cache: &mut cache,
                            grain: &mut grain,
                        },
                        &mut discard,
                        1,
                        || false,
                    )
                    .unwrap()
                {
                    GranularSeekProgress::Pending { .. } => pending += 1,
                    GranularSeekProgress::Reached { position } => {
                        assert_eq!(position, 256);
                        break;
                    }
                }
            }
            assert!(pending > 0);
            eprintln!(
                "verified_seek_replay mode={mode:?} law={law:?} target=256 from=0 pending_calls={pending} elapsed_us={} verified_reads={}",
                seek_started.elapsed().as_micros(),
                reader.raw_page_reads() - seek_reads
            );
            assert_eq!(
                cursor_read(
                    &mut cursor,
                    &mut reader,
                    &plan,
                    &mut raw,
                    &mut cache,
                    &mut grain,
                    256
                ),
                first[256..]
            );
            assert_eq!(
                cursor
                    .begin_seek(&middle, 768, 256, &mut reader, &plan, || false)
                    .unwrap(),
                GranularSeekProgress::Pending { position: 512 }
            );
            loop {
                if matches!(
                    cursor
                        .continue_seek(
                            &mut reader,
                            &plan,
                            VerifiedGranularBuffers {
                                source: &mut raw,
                                cache: &mut cache,
                                grain: &mut grain
                            },
                            &mut discard,
                            1,
                            || false
                        )
                        .unwrap(),
                    GranularSeekProgress::Reached { .. }
                ) {
                    break;
                }
            }
            assert_eq!(
                cursor_read(
                    &mut cursor,
                    &mut reader,
                    &plan,
                    &mut raw,
                    &mut cache,
                    &mut grain,
                    256
                ),
                later[256..]
            );
        }
    }
    eprintln!(
        "verified_seek rate=44100 loop=5000 phase=4900 modes=2 laws=2 targets=2 preroll=256 steps=1 elapsed_ms={} verified_reads={}",
        started.elapsed().as_millis(),
        reader.raw_page_reads() - before
    );
}

#[test]
fn verified_granular_seek_rejects_invalid_source_budget_and_foreign_marks() {
    use hex_stream::{
        GranularSeekProgress, VerifiedGranularBuffers, VerifiedGranularCursor,
        VerifiedGranularStepError,
    };
    use hex_warp::{Error, MAX_SCRATCH_BYTES, Mode, Resampler};
    let (wav, _) = Wav::create(8192, false);
    let (mut reader, plan, meta) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let mut raw = vec![[0.0; 2]; 8192];
    let mut cache = vec![[0.0; 2]; 2048];
    let reference =
        scan_reference_channel_on_control(&meta, &mut reader, &plan, &mut raw, &mut cache, || {
            false
        })
        .unwrap();
    let mut dirty = granular_state(
        plan.output_frames(),
        48_000,
        Mode::Tones { grain_ms: 8 },
        Resampler::V1,
        reference.channel(),
    );
    let mut dirty_grain = vec![[0.0; 2]; dirty.max_source_frames()];
    hex_stream::advance_verified_granular_on_control(
        &mut dirty,
        &reference,
        &mut reader,
        &plan,
        VerifiedGranularBuffers {
            source: &mut raw,
            cache: &mut cache,
            grain: &mut dirty_grain,
        },
        || false,
    )
    .unwrap();
    assert!(!dirty.is_pristine());
    assert!(matches!(
        VerifiedGranularCursor::new(dirty, reference.clone()),
        Err(Error::Invalid(_))
    ));
    let mut cursor = VerifiedGranularCursor::new(
        granular_state(
            plan.output_frames(),
            48_000,
            Mode::Tones { grain_ms: 8 },
            Resampler::V1,
            reference.channel(),
        ),
        reference.clone(),
    )
    .unwrap();
    let other = VerifiedGranularCursor::new(
        granular_state(
            plan.output_frames(),
            48_000,
            Mode::Tones { grain_ms: 8 },
            Resampler::V1,
            reference.channel(),
        ),
        reference,
    )
    .unwrap();
    let foreign = other.checkpoint(MAX_SCRATCH_BYTES).unwrap();
    let mark = cursor.checkpoint(MAX_SCRATCH_BYTES).unwrap();
    let mut grain = vec![[0.0; 2]; cursor.max_source_frames()];
    assert!(matches!(
        cursor.begin_seek(&foreign, 10, 10, &mut reader, &plan, || false),
        Err(VerifiedGranularStepError::Warp(Error::Invalid(_)))
    ));
    assert!(matches!(
        cursor.begin_seek(&mark, 100, 99, &mut reader, &plan, || false),
        Err(VerifiedGranularStepError::Warp(Error::Invalid(_)))
    ));
    assert!(matches!(
        cursor.begin_seek(
            &mark,
            10,
            hex_stream::MAX_GRANULAR_PREROLL_FRAMES + 1,
            &mut reader,
            &plan,
            || false
        ),
        Err(VerifiedGranularStepError::Warp(Error::Invalid(_)))
    ));
    assert!(matches!(
        cursor.begin_seek(
            &mark,
            plan.output_frames() + 1,
            plan.output_frames() + 1,
            &mut reader,
            &plan,
            || false
        ),
        Err(VerifiedGranularStepError::Warp(Error::Invalid(_)))
    ));
    assert_eq!(cursor.position(), 0);
    let kernel = ResampleKernel::with_version(48_000, 48_000, ResampleVersion::V2).unwrap();
    let wrong_plan = ResamplePlan::with_kernel(kernel, 8192, 8192, 16384, 4096).unwrap();
    assert_eq!(
        cursor.begin_seek(&mark, 10, 10, &mut reader, &wrong_plan, || false),
        Err(VerifiedGranularStepError::Source(
            StreamFailureKind::UnknownSource
        ))
    );
    assert_eq!(
        cursor
            .begin_seek(&mark, 100, 100, &mut reader, &plan, || false)
            .unwrap(),
        GranularSeekProgress::Pending { position: 0 }
    );
    let mut discard = [[0.0; 2]; 32];
    assert!(matches!(
        cursor.continue_seek(
            &mut reader,
            &plan,
            VerifiedGranularBuffers {
                source: &mut raw,
                cache: &mut cache,
                grain: &mut grain
            },
            &mut discard,
            0,
            || false
        ),
        Err(VerifiedGranularStepError::Warp(Error::Invalid(_)))
    ));
    let cancelled = cursor.continue_seek(
        &mut reader,
        &plan,
        VerifiedGranularBuffers {
            source: &mut raw,
            cache: &mut cache,
            grain: &mut grain,
        },
        &mut discard,
        1,
        || true,
    );
    assert!(matches!(
        cancelled,
        Err(VerifiedGranularStepError::Warp(Error::Cancelled))
            | Err(VerifiedGranularStepError::Source(
                StreamFailureKind::Cancelled
            ))
    ));
    assert_eq!(cursor.seek_target(), Some(100));
    assert!(!cursor.is_poisoned());
    loop {
        if matches!(
            cursor
                .continue_seek(
                    &mut reader,
                    &plan,
                    VerifiedGranularBuffers {
                        source: &mut raw,
                        cache: &mut cache,
                        grain: &mut grain
                    },
                    &mut discard,
                    1,
                    || false
                )
                .unwrap(),
            GranularSeekProgress::Reached { position: 100 }
        ) {
            break;
        }
    }
    assert_eq!(
        cursor
            .begin_seek(&mark, 100, 100, &mut reader, &plan, || false)
            .unwrap(),
        GranularSeekProgress::Pending { position: 0 }
    );
    let mut file = OpenOptions::new().write(true).open(&wav.0).unwrap();
    file.seek(SeekFrom::Start(44)).unwrap();
    file.write_all(&[1, 2, 3, 4]).unwrap();
    drop(file);
    assert_eq!(
        cursor.begin_seek(&mark, 10, 10, &mut reader, &plan, || false),
        Err(VerifiedGranularStepError::Source(
            StreamFailureKind::SourceChanged
        ))
    );
    assert_eq!(cursor.seek_target(), Some(100));
}

#[test]
fn granular_checkpoint_cache_evicts_lru_and_selects_nearest_prior() {
    use hex_stream::{
        CheckpointCacheError, CheckpointRecord, GranularSeekProgress,
        VerifiedGranularCheckpointCache, VerifiedGranularCursor,
    };
    use hex_warp::{Mode, Resampler};
    let (wav, _) = Wav::create(8192, false);
    let (mut reader, plan, meta) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let mut raw = vec![[0.0; 2]; 8192];
    let mut cache = vec![[0.0; 2]; 2048];
    let reference =
        scan_reference_channel_on_control(&meta, &mut reader, &plan, &mut raw, &mut cache, || {
            false
        })
        .unwrap();
    for mode in [
        Mode::Tones { grain_ms: 8 },
        Mode::Texture {
            grain_ms: 8,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        for law in [Resampler::V1, Resampler::V2] {
            let mut cursor = VerifiedGranularCursor::new(
                granular_state(plan.output_frames(), 48_000, mode, law, reference.channel()),
                reference.clone(),
            )
            .unwrap();
            let mut bank = VerifiedGranularCheckpointCache::new(&cursor, 3, 1_000_000).unwrap();
            let mut grain = vec![[0.0; 2]; cursor.max_source_frames()];
            let first = cursor_read(
                &mut cursor,
                &mut reader,
                &plan,
                &mut raw,
                &mut cache,
                &mut grain,
                128,
            );
            assert_eq!(bank.record(&cursor), Ok(CheckpointRecord::Added));
            let second = cursor_read(
                &mut cursor,
                &mut reader,
                &plan,
                &mut raw,
                &mut cache,
                &mut grain,
                128,
            );
            assert_eq!(bank.record(&cursor), Ok(CheckpointRecord::Added));
            let bytes = bank.backing_bytes().unwrap();
            assert_eq!(bank.len(), 3);
            assert!(bytes <= 1_000_000);
            assert_eq!(
                bank.begin_seek(&mut cursor, 128, 0, &mut reader, &plan, || false),
                Ok(GranularSeekProgress::Reached { position: 128 })
            );
            assert_eq!(
                cursor_read(
                    &mut cursor,
                    &mut reader,
                    &plan,
                    &mut raw,
                    &mut cache,
                    &mut grain,
                    128
                ),
                second
            );
            let third = cursor_read(
                &mut cursor,
                &mut reader,
                &plan,
                &mut raw,
                &mut cache,
                &mut grain,
                128,
            );
            assert_eq!(
                bank.record(&cursor),
                Ok(CheckpointRecord::Replaced {
                    evicted_position: 256
                })
            );
            assert_eq!(bank.backing_bytes(), Some(bytes));
            assert_eq!(
                bank.begin_seek(&mut cursor, 256, 0, &mut reader, &plan, || false),
                Err(CheckpointCacheError::NotCovered)
            );
            assert_eq!(cursor.position(), 384);
            assert_eq!(
                bank.begin_seek(&mut cursor, 300, 200, &mut reader, &plan, || false),
                Ok(GranularSeekProgress::Pending { position: 128 })
            );
            let mut discard = [[0.0; 2]; 32];
            loop {
                if matches!(
                    cursor
                        .continue_seek(
                            &mut reader,
                            &plan,
                            hex_stream::VerifiedGranularBuffers {
                                source: &mut raw,
                                cache: &mut cache,
                                grain: &mut grain
                            },
                            &mut discard,
                            1,
                            || false
                        )
                        .unwrap(),
                    GranularSeekProgress::Reached { position: 300 }
                ) {
                    break;
                }
            }
            assert_eq!(
                cursor_read(
                    &mut cursor,
                    &mut reader,
                    &plan,
                    &mut raw,
                    &mut cache,
                    &mut grain,
                    84
                ),
                third[44..]
            );
            assert_eq!(
                bank.begin_seek(&mut cursor, 128, 0, &mut reader, &plan, || false),
                Ok(GranularSeekProgress::Reached { position: 128 })
            );
            assert_eq!(
                cursor_read(
                    &mut cursor,
                    &mut reader,
                    &plan,
                    &mut raw,
                    &mut cache,
                    &mut grain,
                    256
                ),
                [second, third].concat()
            );
            assert_eq!(
                bank.begin_seek(&mut cursor, 0, 0, &mut reader, &plan, || false),
                Ok(GranularSeekProgress::Reached { position: 0 })
            );
            assert_eq!(
                cursor_read(
                    &mut cursor,
                    &mut reader,
                    &plan,
                    &mut raw,
                    &mut cache,
                    &mut grain,
                    128
                ),
                first
            );
        }
    }
}

#[test]
fn granular_checkpoint_cache_refuses_foreign_capacity_pending_and_cancel() {
    use hex_stream::{
        CheckpointCacheError, GranularSeekProgress, VerifiedGranularCheckpointCache,
        VerifiedGranularCursor, VerifiedGranularStepError,
    };
    use hex_warp::{Error, Mode, Resampler};
    let (wav, _) = Wav::create(8192, false);
    let (mut reader, plan, meta) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let mut raw = vec![[0.0; 2]; 8192];
    let mut cache = vec![[0.0; 2]; 2048];
    let reference =
        scan_reference_channel_on_control(&meta, &mut reader, &plan, &mut raw, &mut cache, || {
            false
        })
        .unwrap();
    let make = || {
        VerifiedGranularCursor::new(
            granular_state(
                plan.output_frames(),
                48_000,
                Mode::Tones { grain_ms: 8 },
                Resampler::V1,
                reference.channel(),
            ),
            reference.clone(),
        )
        .unwrap()
    };
    let mut cursor = make();
    let other = make();
    assert!(matches!(
        VerifiedGranularCheckpointCache::new(&cursor, 1, 1_000_000),
        Err(CheckpointCacheError::Warp(Error::Invalid(_)))
    ));
    assert!(matches!(
        VerifiedGranularCheckpointCache::new(&cursor, 3, 0),
        Err(CheckpointCacheError::Warp(Error::ScratchLimit))
    ));
    let mut bank = VerifiedGranularCheckpointCache::new(&cursor, 3, 1_000_000).unwrap();
    let bytes = bank.backing_bytes();
    assert!(matches!(
        bank.record(&other),
        Err(CheckpointCacheError::Warp(Error::Invalid(_)))
    ));
    assert_eq!(bank.len(), 1);
    assert_eq!(bank.backing_bytes(), bytes);
    assert!(matches!(
        bank.begin_seek(&mut cursor, 10, 10, &mut reader, &plan, || true),
        Err(CheckpointCacheError::Seek(
            VerifiedGranularStepError::Source(StreamFailureKind::Cancelled)
        ))
    ));
    assert_eq!(cursor.position(), 0);
    assert_eq!(bank.len(), 1);
    let kernel = ResampleKernel::with_version(48_000, 48_000, ResampleVersion::V2).unwrap();
    let wrong_plan = ResamplePlan::with_kernel(kernel, 8192, 8192, 16384, 4096).unwrap();
    assert_eq!(
        bank.begin_seek(&mut cursor, 10, 10, &mut reader, &wrong_plan, || false),
        Err(CheckpointCacheError::Seek(
            VerifiedGranularStepError::Source(StreamFailureKind::UnknownSource)
        ))
    );
    assert_eq!(bank.len(), 1);
    assert_eq!(
        bank.begin_seek(&mut cursor, 10, 10, &mut reader, &plan, || false),
        Ok(GranularSeekProgress::Pending { position: 0 })
    );
    assert!(matches!(
        bank.record(&cursor),
        Err(CheckpointCacheError::Warp(Error::Invalid(_)))
    ));
}

#[test]
fn granular_checkpoint_cache_refuses_poisoned_cursor_without_eviction() {
    use hex_stream::{
        CheckpointCacheError, VerifiedGranularBuffers, VerifiedGranularCheckpointCache,
        VerifiedGranularCursor,
    };
    use hex_warp::{Error, Mode, Resampler};
    let (wav, _) = Wav::create(8192, false);
    let (mut reader, plan, meta) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let mut raw = vec![[0.0; 2]; 8192];
    let mut cache = vec![[0.0; 2]; 2048];
    let reference =
        scan_reference_channel_on_control(&meta, &mut reader, &plan, &mut raw, &mut cache, || {
            false
        })
        .unwrap();
    let mut observed = false;
    for stop_at in 2..64 {
        let mut cursor = VerifiedGranularCursor::new(
            granular_state(
                plan.output_frames(),
                48_000,
                Mode::Texture {
                    grain_ms: 200,
                    fluctuation: 0,
                    seed: 1,
                },
                Resampler::V1,
                reference.channel(),
            ),
            reference.clone(),
        )
        .unwrap();
        let mut bank = VerifiedGranularCheckpointCache::new(&cursor, 2, 1_000_000).unwrap();
        let bytes = bank.backing_bytes();
        let mut grain = vec![[0.0; 2]; cursor.max_source_frames()];
        let calls = std::cell::Cell::new(0);
        let _ = cursor.advance(
            &mut reader,
            &plan,
            VerifiedGranularBuffers {
                source: &mut raw,
                cache: &mut cache,
                grain: &mut grain,
            },
            || {
                calls.set(calls.get() + 1);
                calls.get() >= stop_at
            },
        );
        if cursor.is_poisoned() {
            assert!(matches!(
                bank.record(&cursor),
                Err(CheckpointCacheError::Warp(Error::Invalid(_)))
            ));
            assert_eq!(bank.len(), 1);
            assert_eq!(bank.backing_bytes(), bytes);
            observed = true;
            break;
        }
    }
    assert!(observed, "expected cancellation inside reconstruction");
}

#[test]
fn granular_checkpoint_cache_reuses_storage_under_byte_budget() {
    use hex_stream::{CheckpointRecord, VerifiedGranularCheckpointCache, VerifiedGranularCursor};
    use hex_warp::{MAX_SCRATCH_BYTES, Mode, Resampler};
    let (wav, _) = Wav::create(8192, false);
    let (mut reader, plan, meta) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let mut raw = vec![[0.0; 2]; 8192];
    let mut cache = vec![[0.0; 2]; 2048];
    let reference =
        scan_reference_channel_on_control(&meta, &mut reader, &plan, &mut raw, &mut cache, || {
            false
        })
        .unwrap();
    let mut cursor = VerifiedGranularCursor::new(
        granular_state(
            plan.output_frames(),
            48_000,
            Mode::Texture {
                grain_ms: 8,
                fluctuation: 0,
                seed: 1,
            },
            Resampler::V1,
            reference.channel(),
        ),
        reference,
    )
    .unwrap();
    let snapshot_bytes = cursor
        .checkpoint(MAX_SCRATCH_BYTES)
        .unwrap()
        .backing_bytes()
        .unwrap();
    let initial_bytes = VerifiedGranularCheckpointCache::new(&cursor, 3, 1_000_000)
        .unwrap()
        .backing_bytes()
        .unwrap();
    let budget = initial_bytes + snapshot_bytes + snapshot_bytes / 2;
    let mut bank = VerifiedGranularCheckpointCache::new(&cursor, 3, budget).unwrap();
    let mut grain = vec![[0.0; 2]; cursor.max_source_frames()];
    cursor_read(
        &mut cursor,
        &mut reader,
        &plan,
        &mut raw,
        &mut cache,
        &mut grain,
        128,
    );
    assert_eq!(bank.record(&cursor), Ok(CheckpointRecord::Added));
    let bytes = bank.backing_bytes().unwrap();
    cursor_read(
        &mut cursor,
        &mut reader,
        &plan,
        &mut raw,
        &mut cache,
        &mut grain,
        128,
    );
    assert_eq!(
        bank.record(&cursor),
        Ok(CheckpointRecord::Replaced {
            evicted_position: 128
        })
    );
    assert_eq!(bank.len(), 2);
    assert_eq!(bank.backing_bytes(), Some(bytes));
    assert!(bytes <= budget);
}

#[test]
#[ignore = "controlled optimized checkpoint cache timing fixture"]
fn granular_checkpoint_cache_profile() {
    use hex_stream::{CheckpointRecord, VerifiedGranularCheckpointCache, VerifiedGranularCursor};
    use hex_warp::{Mode, Resampler};
    let (wav, _) = Wav::create(8192, false);
    let (mut reader, plan, meta) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let mut raw = vec![[0.0; 2]; 8192];
    let mut cache = vec![[0.0; 2]; 2048];
    let reference =
        scan_reference_channel_on_control(&meta, &mut reader, &plan, &mut raw, &mut cache, || {
            false
        })
        .unwrap();
    let mut cursor = VerifiedGranularCursor::new(
        granular_state(
            plan.output_frames(),
            48_000,
            Mode::Texture {
                grain_ms: 200,
                fluctuation: 0,
                seed: 1,
            },
            Resampler::V1,
            reference.channel(),
        ),
        reference,
    )
    .unwrap();
    let mut bank = VerifiedGranularCheckpointCache::new(&cursor, 2, 1_500_000).unwrap();
    let mut grain = vec![[0.0; 2]; cursor.max_source_frames()];
    let mut times = Vec::new();
    for i in 0..26 {
        cursor_read(
            &mut cursor,
            &mut reader,
            &plan,
            &mut raw,
            &mut cache,
            &mut grain,
            32,
        );
        let start = std::time::Instant::now();
        let effect = bank.record(&cursor).unwrap();
        let elapsed = start.elapsed().as_nanos();
        if i > 0 {
            assert!(matches!(effect, CheckpointRecord::Replaced { .. }));
        }
        if i >= 6 {
            times.push(elapsed);
        }
    }
    times.sort_unstable();
    eprintln!(
        "checkpoint_cache rate=48000 grain_ms=200 law=V1 slots=2 retained_bytes={} warmup=5 samples=20 replace_p50_ns={}",
        bank.backing_bytes().unwrap(),
        times[10]
    );
}

#[test]
fn granular_pages_match_resident_and_preserve_buffer_custody() {
    use hex_engine::STREAM_PAGE_FRAMES;
    use hex_stream::{
        GranularPageError, GranularPageProgress, VerifiedGranularBuffers, VerifiedGranularPages,
        scan_verified_granular_region_on_control,
    };
    use hex_warp::{GainPoint, Mode, Resampler, SequentialGranularConfig, SourceRegion};
    let (wav, pcm) = Wav::create(8192, false);
    let (mut reader, plan, meta) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let mut raw = vec![[0.0; 2]; 8192];
    let mut cache = vec![[0.0; 2]; 8192];
    let envelope = vec![
        GainPoint {
            source_frame: 0,
            gain_q16: 65_536,
        },
        GainPoint {
            source_frame: 6000,
            gain_q16: 32_768,
        },
    ];
    let region = SourceRegion::new(8192, 197, 5000, 4900, true, envelope.clone()).unwrap();
    let reference = scan_verified_granular_region_on_control(
        &meta,
        region.clone(),
        &mut reader,
        &plan,
        &mut raw,
        &mut cache,
        || false,
    )
    .unwrap();
    for mode in [
        Mode::Tones { grain_ms: 8 },
        Mode::Texture {
            grain_ms: 8,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        let map = granular_map(8300);
        let expected = hex_warp::render_with_resampler(
            &hex_warp::Request {
                attacks: &[],
                input: hex_warp::Input {
                    frames: &pcm,
                    sample_rate: 48_000,
                    region_start: 197,
                    region_length: 5000,
                    phase: 4900,
                    looping: true,
                    envelope: &envelope,
                },
                map: &map,
                output_frames: 8300,
                pitch_cents: 300,
                mode,
            },
            Resampler::V1,
            &mut || false,
        )
        .unwrap()
        .frames;
        assert!(matches!(
            VerifiedGranularPages::new(
                reference.clone(),
                SequentialGranularConfig {
                    map: map.clone(),
                    sample_rate: 48_000,
                    mode,
                    pitch_cents: 300,
                    resampler: Resampler::V1,
                    reference_channel: 1 - reference.channel()
                },
                3,
                8 * 1024 * 1024,
                &mut || false,
            ),
            Err(GranularPageError::InvalidRequest)
        ));
        let mut pages = VerifiedGranularPages::new(
            reference.clone(),
            SequentialGranularConfig {
                map: map.clone(),
                sample_rate: 48_000,
                mode,
                pitch_cents: 300,
                resampler: Resampler::V1,
                reference_channel: reference.channel(),
            },
            3,
            8 * 1024 * 1024,
            &mut || false,
        )
        .unwrap();
        assert_eq!(
            pages.metadata(),
            &meta
                .with_granular(&region, &map, mode, 300, Resampler::V1)
                .unwrap()
        );
        let mut grain = vec![[0.0; 2]; pages.max_source_frames()];
        let bad = vec![[0.0; 2]; 2].into_boxed_slice();
        let (error, bad) = pages
            .begin_page(0, bad, 0, &mut reader, &plan, || false)
            .unwrap_err();
        assert_eq!(error, GranularPageError::InvalidRequest);
        assert_eq!(bad.len(), 2);
        let page = vec![[0.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
        let ptr = page.as_ptr();
        pages
            .begin_page(0, page, 0, &mut reader, &plan, || false)
            .unwrap();
        assert!(pages.take_complete().is_none());
        let spare = vec![[0.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
        let spare_ptr = spare.as_ptr();
        let (error, spare) = pages
            .begin_page(1, spare, 0, &mut reader, &plan, || false)
            .unwrap_err();
        assert_eq!(error, GranularPageError::InvalidRequest);
        assert_eq!(spare.as_ptr(), spare_ptr);
        assert_eq!(
            pages.step(
                &mut reader,
                &plan,
                VerifiedGranularBuffers {
                    source: &mut raw,
                    cache: &mut cache,
                    grain: &mut grain
                },
                || true
            ),
            Err(GranularPageError::Step(
                hex_stream::VerifiedGranularStepError::Source(StreamFailureKind::Cancelled)
            ))
        );
        assert!(pages.take_complete().is_none());
        let page = pages.reclaim().unwrap();
        assert_eq!(page.as_ptr(), ptr);
        let mut page = page;
        for index in [0, 2, 1, 0] {
            if index == 2 {
                let (error, returned) = pages
                    .begin_page(index, page, 1024, &mut reader, &plan, || false)
                    .unwrap_err();
                assert_eq!(
                    error,
                    GranularPageError::Cache(hex_stream::CheckpointCacheError::NotCovered)
                );
                page = returned;
                pages
                    .begin_page_replaying(index, page, 1024, &mut reader, &plan, || false)
                    .unwrap();
            } else {
                pages
                    .begin_page(
                        index,
                        page,
                        STREAM_PAGE_FRAMES as u64,
                        &mut reader,
                        &plan,
                        || false,
                    )
                    .unwrap();
            }
            assert!(pages.take_complete().is_none());
            let mut steps = 0;
            loop {
                steps += 1;
                assert!(steps < 1000);
                if pages
                    .step(
                        &mut reader,
                        &plan,
                        VerifiedGranularBuffers {
                            source: &mut raw,
                            cache: &mut cache,
                            grain: &mut grain,
                        },
                        || false,
                    )
                    .unwrap()
                    == GranularPageProgress::Complete
                {
                    break;
                }
                assert!(pages.take_complete().is_none());
            }
            let completed = pages.take_complete().unwrap();
            let start = index as usize * STREAM_PAGE_FRAMES;
            let valid = (8300 - start).min(STREAM_PAGE_FRAMES);
            assert_eq!(completed.page_index, index);
            assert_eq!(completed.valid_frames, valid);
            assert_eq!(&completed.frames[..valid], &expected[start..start + valid]);
            assert!(completed.frames[valid..].iter().all(|f| *f == [0.0; 2]));
            assert_eq!(completed.frames.as_ptr(), ptr);
            page = completed.frames;
        }
    }
    let mut pages = VerifiedGranularPages::new(
        reference.clone(),
        SequentialGranularConfig {
            map: granular_map(8300),
            sample_rate: 48_000,
            mode: Mode::Tones { grain_ms: 8 },
            pitch_cents: 300,
            resampler: Resampler::V1,
            reference_channel: reference.channel(),
        },
        3,
        8 * 1024 * 1024,
        &mut || false,
    )
    .unwrap();
    let mut grain = vec![[0.0; 2]; pages.max_source_frames()];
    let page = vec![[0.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
    let ptr = page.as_ptr();
    pages
        .begin_page(0, page, 0, &mut reader, &plan, || false)
        .unwrap();
    assert_eq!(
        pages
            .step(
                &mut reader,
                &plan,
                VerifiedGranularBuffers {
                    source: &mut raw,
                    cache: &mut cache,
                    grain: &mut grain
                },
                || false
            )
            .unwrap(),
        GranularPageProgress::Pending
    );
    OpenOptions::new()
        .write(true)
        .open(&wav.0)
        .unwrap()
        .set_len(100)
        .unwrap();
    let error = loop {
        match pages.step(
            &mut reader,
            &plan,
            VerifiedGranularBuffers {
                source: &mut raw,
                cache: &mut cache,
                grain: &mut grain,
            },
            || false,
        ) {
            Ok(GranularPageProgress::Pending) => continue,
            Ok(GranularPageProgress::Complete) => panic!("changed source published a page"),
            Err(error) => break error,
        }
    };
    assert_eq!(
        error,
        GranularPageError::Step(hex_stream::VerifiedGranularStepError::Source(
            StreamFailureKind::SourceChanged
        ))
    );
    assert!(pages.take_complete().is_none());
    let reclaimed = pages.reclaim().unwrap();
    assert_eq!(reclaimed.as_ptr(), ptr);
}

#[test]
fn distant_granular_page_replay_is_resumable_and_cancelled_without_publication() {
    use hex_engine::STREAM_PAGE_FRAMES;
    use hex_stream::{
        CheckpointCacheError, GranularPageError, GranularPageProgress, VerifiedGranularBuffers,
        VerifiedGranularPages, scan_verified_granular_region_on_control,
    };
    use hex_warp::{Mode, Resampler, SequentialGranularConfig, SourceRegion};
    let (wav, _) = Wav::create(8192, false);
    let (mut reader, plan, meta) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let mut raw = vec![[0.0; 2]; 8192];
    let mut cache = vec![[0.0; 2]; 8192];
    let region = SourceRegion::new(8192, 0, 8192, 0, true, vec![]).unwrap();
    let reference = scan_verified_granular_region_on_control(
        &meta,
        region,
        &mut reader,
        &plan,
        &mut raw,
        &mut cache,
        || false,
    )
    .unwrap();
    let mut pages = VerifiedGranularPages::new(
        reference.clone(),
        SequentialGranularConfig {
            map: granular_map(1_060_000),
            sample_rate: 48_000,
            mode: Mode::Tones { grain_ms: 8 },
            pitch_cents: 0,
            resampler: Resampler::V1,
            reference_channel: reference.channel(),
        },
        2,
        8 * 1024 * 1024,
        &mut || false,
    )
    .unwrap();
    let mut grain = vec![[0.0; 2]; pages.max_source_frames()];
    let page = vec![[0.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
    let pointer = page.as_ptr();
    let (error, page) = pages
        .begin_page(
            257,
            page,
            hex_stream::MAX_GRANULAR_PREROLL_FRAMES,
            &mut reader,
            &plan,
            || false,
        )
        .unwrap_err();
    assert_eq!(
        error,
        GranularPageError::Cache(CheckpointCacheError::NotCovered)
    );
    let (error, page) = pages
        .begin_page_replaying(257, page, 0, &mut reader, &plan, || false)
        .unwrap_err();
    assert!(matches!(
        error,
        GranularPageError::Cache(CheckpointCacheError::Warp(hex_warp::Error::Invalid(_)))
    ));
    pages
        .begin_page_replaying(257, page, 64, &mut reader, &plan, || false)
        .unwrap();
    for _ in 0..12 {
        assert_eq!(
            pages
                .step(
                    &mut reader,
                    &plan,
                    VerifiedGranularBuffers {
                        source: &mut raw,
                        cache: &mut cache,
                        grain: &mut grain
                    },
                    || false
                )
                .unwrap(),
            GranularPageProgress::Pending
        );
        assert!(pages.take_complete().is_none());
    }
    assert_eq!(
        pages.step(
            &mut reader,
            &plan,
            VerifiedGranularBuffers {
                source: &mut raw,
                cache: &mut cache,
                grain: &mut grain
            },
            || true
        ),
        Err(GranularPageError::Step(
            hex_stream::VerifiedGranularStepError::Source(StreamFailureKind::Cancelled)
        ))
    );
    assert!(pages.take_complete().is_none());
    let page = pages.reclaim().unwrap();
    assert_eq!(page.as_ptr(), pointer);
    pages
        .begin_page_replaying(257, page, 64, &mut reader, &plan, || false)
        .unwrap();
    OpenOptions::new()
        .write(true)
        .open(&wav.0)
        .unwrap()
        .set_len(100)
        .unwrap();
    let result = loop {
        match pages.step(
            &mut reader,
            &plan,
            VerifiedGranularBuffers {
                source: &mut raw,
                cache: &mut cache,
                grain: &mut grain,
            },
            || false,
        ) {
            Ok(GranularPageProgress::Pending) => continue,
            other => break other,
        }
    };
    assert_eq!(
        result,
        Err(GranularPageError::Step(
            hex_stream::VerifiedGranularStepError::Source(StreamFailureKind::SourceChanged)
        ))
    );
    assert!(pages.take_complete().is_none());
    let returned = pages.reclaim().unwrap();
    assert_eq!(returned.as_ptr(), pointer);
}

// The two paths share grain analysis and synthesis kernels, but independently
// schedule the complete resident render and checkpointed verified streaming.
fn distant_page_matches_resident(mode: hex_warp::Mode) {
    use hex_engine::STREAM_PAGE_FRAMES;
    use hex_stream::{
        GranularPageProgress, VerifiedGranularBuffers, VerifiedGranularPages,
        scan_verified_granular_region_on_control,
    };
    use hex_warp::{Resampler, SequentialGranularConfig, SourceRegion};

    let (wav, pcm) = Wav::create(8192, false);
    let (mut reader, plan, metadata) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let mut raw = vec![[0.0; 2]; 8192];
    let mut cache = vec![[0.0; 2]; 8192];
    let region = SourceRegion::new(8192, 0, 8192, 0, true, vec![]).unwrap();
    let reference = scan_verified_granular_region_on_control(
        &metadata,
        region,
        &mut reader,
        &plan,
        &mut raw,
        &mut cache,
        || false,
    )
    .unwrap();
    let page_index = 257;
    let target = page_index * STREAM_PAGE_FRAMES;
    let total = target + STREAM_PAGE_FRAMES;
    let map = granular_map(total as u64);
    let resident = hex_warp::render_with_resampler(
        &hex_warp::Request {
            attacks: &[],
            input: hex_warp::Input {
                frames: &pcm,
                sample_rate: 48_000,
                region_start: 0,
                region_length: 8192,
                phase: 0,
                looping: true,
                envelope: &[],
            },
            map: &map,
            output_frames: total,
            pitch_cents: 0,
            mode,
        },
        Resampler::V1,
        &mut || false,
    )
    .unwrap();
    let mut pages = VerifiedGranularPages::new(
        reference.clone(),
        SequentialGranularConfig {
            map,
            sample_rate: 48_000,
            mode,
            pitch_cents: 0,
            resampler: Resampler::V1,
            reference_channel: reference.channel(),
        },
        2,
        8 * 1024 * 1024,
        &mut || false,
    )
    .unwrap();
    let mut grain = vec![[0.0; 2]; pages.max_source_frames()];
    let page = vec![[0.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
    pages
        .begin_page_replaying(
            page_index as u64,
            page,
            hex_stream::MAX_GRANULAR_PREROLL_FRAMES,
            &mut reader,
            &plan,
            || false,
        )
        .unwrap();
    let mut complete = false;
    // The 8 ms fixture needs about 11,000 one-grain turns at this distance.
    for _ in 0..12_000 {
        if pages
            .step(
                &mut reader,
                &plan,
                VerifiedGranularBuffers {
                    source: &mut raw,
                    cache: &mut cache,
                    grain: &mut grain,
                },
                || false,
            )
            .unwrap()
            == GranularPageProgress::Complete
        {
            complete = true;
            break;
        }
        assert!(pages.take_complete().is_none());
    }
    assert!(
        complete,
        "distant page did not finish within the fixture's step bound"
    );
    let page = pages.take_complete().unwrap();
    assert_eq!(page.page_index, page_index as u64);
    assert_eq!(page.valid_frames, STREAM_PAGE_FRAMES);
    assert!(page.frames.iter().any(|frame| *frame != [0.0; 2]));
    assert_eq!(&*page.frames, &resident.frames[target..total]);
}

#[test]
fn distant_tones_page_matches_resident() {
    distant_page_matches_resident(hex_warp::Mode::Tones { grain_ms: 200 });
}

#[test]
fn distant_texture_page_matches_resident() {
    distant_page_matches_resident(hex_warp::Mode::Texture {
        grain_ms: 200,
        fluctuation: 35,
        seed: 42,
    });
}

#[test]
fn distant_short_texture_page_matches_resident() {
    distant_page_matches_resident(hex_warp::Mode::Texture {
        grain_ms: 8,
        fluctuation: 35,
        seed: 42,
    });
}

#[test]
fn canonical_granular_projection_drives_verified_page_matching_resident() {
    use hex_engine::{STREAM_PAGE_FRAMES, audio_transform::streamed_granular_root};
    use hex_project::{SourceGainPoint, WarpMode, default_audio_warp};
    use hex_stream::{
        GranularPageError, GranularPageProgress, GranularSourceBuffers, GranularSourceError,
        GranularSourcePreparation, VerifiedGranularSource, VerifiedGranularStepError,
    };
    use std::sync::atomic::AtomicBool;

    let (wav, pcm) = Wav::create(8192, false);
    let (reader, plan, _) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 8192,
        })
        .unwrap()
    else {
        panic!("audio fixture")
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: true,
        })
        .unwrap();
    editor
        .execute(Command::SetClipSourcePhase { clip, phase: 137 })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![
                SourceGainPoint {
                    frame: 0,
                    gain_q16: 40_000,
                },
                SourceGainPoint {
                    frame: 4_000,
                    gain_q16: 60_000,
                },
            ],
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 300 })
        .unwrap();
    let raw = StreamedSourceMetadata::new(
        StreamSourceKey::new(7, asset),
        48_000,
        48_000,
        8192,
        8192,
        IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .unwrap();
    let edit = editor.project().clip(clip).unwrap().audio_edit().unwrap();
    let root = streamed_granular_root(
        &raw,
        edit,
        editor.project().tempo_map(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let (metadata, region, map, mode, pitch, law) = root.into_parts();
    let expected = hex_warp::render_with_resampler(
        &hex_warp::Request {
            attacks: &[],
            input: hex_warp::Input {
                frames: &pcm,
                sample_rate: 48_000,
                region_start: region.start(),
                region_length: region.length(),
                phase: region.phase(),
                looping: region.looping(),
                envelope: region.envelope(),
            },
            map: &map,
            output_frames: metadata.playback_frames() as usize,
            pitch_cents: pitch,
            mode,
        },
        law,
        &mut || false,
    )
    .unwrap()
    .frames;
    let make_input = |raw_input: StreamedSourceMetadata, scratch_budget, grain_frames| {
        let (reader, plan, _) = setup(&wav, 8192, 48_000, ResampleVersion::V1);
        GranularSourcePreparation {
            raw: raw_input,
            root: streamed_granular_root(
                &raw,
                edit,
                editor.project().tempo_map(),
                &AtomicBool::new(false),
            )
            .unwrap(),
            reader,
            plan,
            buffers: GranularSourceBuffers {
                source: vec![[0.0; 2]; 8192].into_boxed_slice(),
                cache: vec![[0.0; 2]; 8192].into_boxed_slice(),
                grain: vec![[0.0; 2]; grain_frames].into_boxed_slice(),
            },
            scratch_budget,
            checkpoint_entries: 2,
            checkpoint_budget: 8 * 1024 * 1024,
        }
    };
    let foreign = StreamedSourceMetadata::new(
        StreamSourceKey::new(8, asset),
        48_000,
        48_000,
        8192,
        8192,
        IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .unwrap();
    assert!(matches!(
        VerifiedGranularSource::prepare(make_input(foreign, 1_000_000, 10_000), || false),
        Err(GranularSourceError::InvalidSource)
    ));
    let wrong_fingerprint = StreamedSourceMetadata::new(
        StreamSourceKey::new(7, asset),
        48_000,
        48_000,
        8192,
        8192,
        IDENTITY_CONVERSION,
        [9; 32],
    )
    .unwrap();
    assert!(matches!(
        VerifiedGranularSource::prepare(make_input(wrong_fingerprint, 1_000_000, 10_000), || false),
        Err(GranularSourceError::Source(
            StreamFailureKind::SourceChanged
        ))
    ));
    let mut wrong_plan = make_input(raw.clone(), 1_000_000, 10_000);
    wrong_plan.plan = ResamplePlan::with_kernel(
        ResampleKernel::with_version(48_000, 48_000, ResampleVersion::V2).unwrap(),
        8192,
        8192,
        16384,
        4096,
    )
    .unwrap();
    assert!(matches!(
        VerifiedGranularSource::prepare(wrong_plan, || false),
        Err(GranularSourceError::Source(
            StreamFailureKind::UnknownSource
        ))
    ));
    assert!(matches!(
        VerifiedGranularSource::prepare(make_input(raw.clone(), 1, 10_000), || false),
        Err(GranularSourceError::ScratchLimit)
    ));
    assert!(matches!(
        VerifiedGranularSource::prepare(make_input(raw.clone(), 1_000_000, 1), || false),
        Err(GranularSourceError::ScratchLimit)
    ));
    assert!(matches!(
        VerifiedGranularSource::prepare(make_input(raw.clone(), 1_000_000, 10_000), || true),
        Err(GranularSourceError::Step(
            VerifiedGranularStepError::Source(StreamFailureKind::Cancelled)
        ))
    ));
    let root = streamed_granular_root(
        &raw,
        edit,
        editor.project().tempo_map(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut source = VerifiedGranularSource::prepare(
        GranularSourcePreparation {
            raw,
            root,
            reader,
            plan,
            buffers: GranularSourceBuffers {
                source: vec![[0.0; 2]; 8192].into_boxed_slice(),
                cache: vec![[0.0; 2]; 8192].into_boxed_slice(),
                grain: vec![[0.0; 2]; 10_000].into_boxed_slice(),
            },
            scratch_budget: 1_000_000,
            checkpoint_entries: 2,
            checkpoint_budget: 8 * 1024 * 1024,
        },
        || false,
    )
    .unwrap();
    assert_eq!(source.metadata(), &metadata);
    let scratch_bytes =
        2 * 8192 * std::mem::size_of::<[f32; 2]>() + 10_000 * std::mem::size_of::<[f64; 2]>();
    assert!(source.reserved_processing_bytes() >= 8 * 1024 * 1024 + scratch_bytes);
    assert!(source.reserved_processing_bytes() <= hex_warp::MAX_SCRATCH_BYTES);
    let page = vec![[0.0; 2]; STREAM_PAGE_FRAMES].into_boxed_slice();
    let pointer = page.as_ptr();
    let (error, page) = source.begin_page(u64::MAX, page, 0, || false).unwrap_err();
    assert_eq!(error, GranularPageError::InvalidRequest);
    assert_eq!(page.as_ptr(), pointer);
    source.begin_page(0, page, 0, || false).unwrap();
    assert_eq!(
        source.step(|| true),
        Err(GranularPageError::Step(VerifiedGranularStepError::Source(
            StreamFailureKind::Cancelled
        )))
    );
    assert!(source.take_complete().is_none());
    let page = source.reclaim().unwrap();
    assert_eq!(page.as_ptr(), pointer);
    source.begin_page(0, page, 0, || false).unwrap();
    for _ in 0..1000 {
        if source.step(|| false).unwrap() == GranularPageProgress::Complete {
            break;
        }
    }
    let page = source.take_complete().expect("canonical page completed");
    assert_eq!(page.valid_frames, STREAM_PAGE_FRAMES);
    assert_eq!(&*page.frames, &expected[..STREAM_PAGE_FRAMES]);
    let pointer = page.frames.as_ptr();
    source.begin_page(0, page.frames, 0, || false).unwrap();
    OpenOptions::new()
        .write(true)
        .open(&wav.0)
        .unwrap()
        .set_len(100)
        .unwrap();
    let error = loop {
        match source.step(|| false) {
            Ok(GranularPageProgress::Pending) => continue,
            Ok(GranularPageProgress::Complete) => panic!("changed source published a page"),
            Err(error) => break error,
        }
    };
    assert_eq!(
        error,
        GranularPageError::Step(VerifiedGranularStepError::Source(
            StreamFailureKind::SourceChanged
        ))
    );
    assert!(source.take_complete().is_none());
    let returned = source.reclaim().unwrap();
    assert_eq!(returned.as_ptr(), pointer);
}
