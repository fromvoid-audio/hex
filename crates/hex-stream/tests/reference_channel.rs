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
