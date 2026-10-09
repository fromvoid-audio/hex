use crate::{
    ImportError, Limits, ResampleLimits, ResamplePlan, WavFrameReader, WavReaderLimits, decode_wav,
    resample,
};
use hex_engine::Audio;
use std::{
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_FILE: AtomicU64 = AtomicU64::new(1);

struct TempFile(PathBuf);

impl TempFile {
    fn create(bytes: &[u8]) -> Self {
        let path = temp_path();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create wav fixture");
        file.write_all(bytes).expect("write wav fixture");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn temp_path() -> PathBuf {
    let serial = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "hex-media-resample-window-{}-{serial}.wav",
        std::process::id()
    ))
}

fn push_chunk(chunks: &mut Vec<u8>, id: &[u8; 4], payload: &[u8]) {
    chunks.extend_from_slice(id);
    chunks.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    chunks.extend_from_slice(payload);
    if payload.len() & 1 != 0 {
        chunks.push(0);
    }
}

fn format_chunk(sample_rate: u32, channels: u16, bits: u16) -> Vec<u8> {
    let block_align = channels * (bits / 8);
    let byte_rate = sample_rate * u32::from(block_align);
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1_u16.to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&sample_rate.to_le_bytes());
    fmt.extend_from_slice(&byte_rate.to_le_bytes());
    fmt.extend_from_slice(&block_align.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    fmt
}

fn wave(sample_rate: u32, channels: u16, bits: u16, data: &[u8]) -> Vec<u8> {
    let mut chunks = Vec::new();
    push_chunk(&mut chunks, b"data", data);
    push_chunk(
        &mut chunks,
        b"fmt ",
        &format_chunk(sample_rate, channels, bits),
    );
    let riff_size = u32::try_from(4 + chunks.len()).expect("small wav fixture");
    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&riff_size.to_le_bytes());
    file.extend_from_slice(b"WAVE");
    file.extend_from_slice(&chunks);
    file
}

fn pcm16_stereo(frames: &[[i16; 2]]) -> Vec<u8> {
    let mut data = Vec::with_capacity(frames.len() * 4);
    for frame in frames {
        data.extend_from_slice(&frame[0].to_le_bytes());
        data.extend_from_slice(&frame[1].to_le_bytes());
    }
    data
}

#[test]
fn wav_reader_windows_convert_with_global_phase_and_match_resident_audio() {
    let pcm = (0..1_200)
        .map(|index| {
            [
                ((index * 47 % 8_000) as i16) - 4_000,
                ((index * 29 % 6_000) as i16) - 3_000,
            ]
        })
        .collect::<Vec<_>>();
    let bytes = wave(44_100, 2, 16, &pcm16_stereo(&pcm));
    let source = decode_wav(&bytes, Limits::new(bytes.len(), 2_000)).expect("resident decode");
    let expected = resample(
        &source,
        48_000,
        ResampleLimits {
            max_source_frames: 2_000,
            max_output_frames: 3_000,
        },
    )
    .expect("resident conversion");
    let file = TempFile::create(&bytes);
    let limits = WavReaderLimits::new(bytes.len() as u64, 2_000, 8, 600);
    let mut reader = WavFrameReader::open(file.path(), limits).expect("bounded source reader");
    let plan = ResamplePlan::new(44_100, 48_000, 1_200, 2_000, 3_000, 257)
        .expect("bounded conversion plan");
    let mut output_start = 0_u64;
    let mut scratch = vec![[0.0; 2]; 600];
    let mut seed = 0xabcd_1234_u64;
    while output_start < plan.output_frames() {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let count = usize::try_from((seed % 257 + 1).min(plan.output_frames() - output_start))
            .expect("small output window");
        let mut output = vec![[1.0, -1.0]; count];
        plan.render_wav_window(&mut reader, output_start, &mut output, &mut scratch)
            .expect("bounded wav conversion window");
        let start = usize::try_from(output_start).expect("output address");
        for (actual, expected) in output
            .iter()
            .flatten()
            .zip(expected.frames()[start..start + count].iter().flatten())
        {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
        output_start += count as u64;
    }
}

#[test]
fn wav_conversion_preflights_metadata_reader_window_and_scratch() {
    let pcm = [[1_i16, -1_i16]; 128];
    let bytes = wave(48_000, 2, 16, &pcm16_stereo(&pcm));
    let file = TempFile::create(&bytes);
    let mut reader = WavFrameReader::open(
        file.path(),
        WavReaderLimits::new(bytes.len() as u64, 256, 8, 32),
    )
    .expect("reader with small source window limit");
    let plan = ResamplePlan::new(48_000, 16_000, 128, 256, 256, 8).expect("plan");
    let mut output = [[0.75, -0.75]; 4];
    let mut scratch = [[0.0; 2]; 256];
    assert!(matches!(
        plan.render_wav_window(&mut reader, 0, &mut output, &mut scratch),
        Err(ImportError::LimitExceeded("wav reader window frames"))
    ));
    assert_eq!(output, [[0.0; 2]; 4]);

    let mut reader = WavFrameReader::open(file.path(), WavReaderLimits::new(10_000, 256, 8, 256))
        .expect("reader");
    let mut short_scratch = [[0.0; 2]; 10];
    output.fill([0.75, -0.75]);
    assert!(matches!(
        plan.render_wav_window(&mut reader, 0, &mut output, &mut short_scratch),
        Err(ImportError::LimitExceeded("source scratch frames"))
    ));
    assert_eq!(output, [[0.0; 2]; 4]);

    let wrong_rate = wave(44_100, 2, 16, &pcm16_stereo(&pcm));
    let wrong_file = TempFile::create(&wrong_rate);
    let mut wrong_reader = WavFrameReader::open(
        wrong_file.path(),
        WavReaderLimits::new(wrong_rate.len() as u64, 256, 8, 256),
    )
    .expect("wrong-rate reader still opens");
    output.fill([0.75, -0.75]);
    assert!(matches!(
        plan.render_wav_window(&mut wrong_reader, 0, &mut output, &mut scratch),
        Err(ImportError::Invalid(
            "wav reader metadata does not match resample plan"
        ))
    ));
    assert_eq!(output, [[0.0; 2]; 4]);

    let mut changed_reader = WavFrameReader::open(
        file.path(),
        WavReaderLimits::new(bytes.len() as u64, 256, 8, 256),
    )
    .expect("reader for mutation error");
    OpenOptions::new()
        .write(true)
        .open(file.path())
        .expect("open source for truncate")
        .set_len(64)
        .expect("truncate source");
    output.fill([0.75, -0.75]);
    assert!(
        plan.render_wav_window(&mut changed_reader, 0, &mut output, &mut scratch)
            .is_err()
    );
    assert_eq!(output, [[0.0; 2]; 4]);
}

#[test]
fn sparse_far_wav_conversion_matches_local_global_phase_oracle() {
    let path = temp_path();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("create sparse source");
    let _cleanup = TempFile(path.clone());
    let source_frames = 17_000_001_u64;
    let data_bytes = source_frames * 4;
    let data_offset = 20_u64;
    let format_offset = data_offset + data_bytes;
    let riff_size = 4_u64 + 8 + data_bytes + 8 + 16;
    let file_len = riff_size + 8;
    file.set_len(file_len).expect("sparse source length");
    file.seek(SeekFrom::Start(0)).expect("seek RIFF header");
    file.write_all(b"RIFF").expect("write RIFF");
    file.write_all(&(riff_size as u32).to_le_bytes())
        .expect("write RIFF length");
    file.write_all(b"WAVEdata").expect("write data header");
    file.write_all(&(data_bytes as u32).to_le_bytes())
        .expect("write data length");
    file.seek(SeekFrom::Start(format_offset))
        .expect("seek fmt after payload");
    file.write_all(b"fmt ").expect("write fmt id");
    file.write_all(&16_u32.to_le_bytes())
        .expect("write fmt length");
    file.write_all(&format_chunk(48_000, 2, 16))
        .expect("write fmt");

    let origin = source_frames - 255;
    assert_eq!(origin % 3, 0);
    let local = {
        let mut frames = vec![[0_i16; 2]; 255];
        frames[17] = [16_384, -16_384];
        frames[42] = [-8_192, 4_096];
        frames[250] = [24_576, -24_576];
        frames
    };
    file.seek(SeekFrom::Start(data_offset + origin * 4))
        .expect("seek far source range");
    file.write_all(&pcm16_stereo(&local))
        .expect("write sparse far frames");
    drop(file);
    assert!(fs::metadata(&path).expect("source metadata").len() > 64 * 1024 * 1024);

    let mut reader =
        WavFrameReader::open(&path, WavReaderLimits::new(file_len, 20_000_000, 4, 512))
            .expect("sparse source opens by header scan");
    let plan = ResamplePlan::new(48_000, 16_000, source_frames, 20_000_000, u64::MAX, 32)
        .expect("large-source conversion plan");
    let local_audio = Audio::new(
        48_000,
        local
            .iter()
            .map(|frame| [frame[0] as f32 / 32_768.0, frame[1] as f32 / 32_768.0])
            .collect(),
    )
    .expect("local oracle source");
    let local_converted = resample(
        &local_audio,
        16_000,
        ResampleLimits {
            max_source_frames: 255,
            max_output_frames: 100,
        },
    )
    .expect("local oracle conversion");
    let global_start = origin / 3 + 5;
    let count = 9;
    let mut output = vec![[1.0, -1.0]; count];
    let mut scratch = vec![[0.0; 2]; 512];
    plan.render_wav_window(&mut reader, global_start, &mut output, &mut scratch)
        .expect("far sparse window converts");
    for (actual, expected) in output
        .iter()
        .flatten()
        .zip(local_converted.frames()[5..5 + count].iter().flatten())
    {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
}
