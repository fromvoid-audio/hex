use crate::{
    ImportError, Limits, ResamplePlan, VerifiedWavReader, VerifiedWavReaderLimits, WavReaderLimits,
    decode_wav,
};
use hex_engine::Frame;
use sha2::{Digest, Sha256};
use sha2_legacy::{Digest as LegacyDigest, Sha256 as LegacySha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

const PAGE_BYTES: usize = 64 * 1024;
static NEXT_FILE: AtomicU64 = AtomicU64::new(1);

struct TempFile(PathBuf);

impl TempFile {
    fn create(bytes: &[u8]) -> Self {
        let path = temp_path();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create verified wav fixture");
        file.write_all(bytes).expect("write verified wav fixture");
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
        "hex-media-verified-{}-{serial}.wav",
        std::process::id()
    ))
}

fn limits(max_window_frames: usize) -> VerifiedWavReaderLimits {
    VerifiedWavReaderLimits::new(
        WavReaderLimits::new(128 * 1024 * 1024, 20_000_000, 128, max_window_frames),
        1024 * 1024,
    )
}

fn pcm16_wav(frames: &[[i16; 2]], odd_prefix: bool) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1_u16.to_le_bytes());
    fmt.extend_from_slice(&2_u16.to_le_bytes());
    fmt.extend_from_slice(&48_000_u32.to_le_bytes());
    fmt.extend_from_slice(&192_000_u32.to_le_bytes());
    fmt.extend_from_slice(&4_u16.to_le_bytes());
    fmt.extend_from_slice(&16_u16.to_le_bytes());
    push_chunk(&mut body, b"fmt ", &fmt);
    if odd_prefix {
        push_chunk(&mut body, b"JUNK", &[0x4d]);
    }
    let mut data = Vec::with_capacity(frames.len() * 4);
    for frame in frames {
        data.extend_from_slice(&frame[0].to_le_bytes());
        data.extend_from_slice(&frame[1].to_le_bytes());
    }
    push_chunk(&mut body, b"data", &data);
    let mut bytes = Vec::with_capacity(body.len() + 4);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes
}

fn float32_wav(samples: &[[f32; 2]]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&3_u16.to_le_bytes());
    fmt.extend_from_slice(&2_u16.to_le_bytes());
    fmt.extend_from_slice(&48_000_u32.to_le_bytes());
    fmt.extend_from_slice(&384_000_u32.to_le_bytes());
    fmt.extend_from_slice(&8_u16.to_le_bytes());
    fmt.extend_from_slice(&32_u16.to_le_bytes());
    push_chunk(&mut body, b"fmt ", &fmt);
    let mut data = Vec::with_capacity(samples.len() * 8);
    for frame in samples {
        data.extend_from_slice(&frame[0].to_le_bytes());
        data.extend_from_slice(&frame[1].to_le_bytes());
    }
    push_chunk(&mut body, b"data", &data);
    let mut bytes = Vec::with_capacity(body.len() + 4);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes
}

fn supported_format_wav(encoding: u16, channels: u16, bits: u16) -> Vec<u8> {
    let block_align = channels * (bits / 8);
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&encoding.to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&48_000_u32.to_le_bytes());
    fmt.extend_from_slice(&(48_000_u32 * u32::from(block_align)).to_le_bytes());
    fmt.extend_from_slice(&block_align.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    push_chunk(&mut body, b"fmt ", &fmt);
    let mut data = Vec::new();
    for frame_index in 0..20_000_i32 {
        for channel in 0..channels {
            match (encoding, bits) {
                (1 | 6 | 7, 8) => data.push((frame_index + i32::from(channel) * 17) as u8),
                (3, 64) => {
                    let value = f64::from(frame_index) / 256.0 - f64::from(channel) * 0.125;
                    data.extend_from_slice(&value.to_le_bytes());
                }
                (1, 16) => {
                    let value = frame_index * 251 + i32::from(channel) * 103 - 20_000;
                    data.extend_from_slice(&(value as i16).to_le_bytes());
                }
                (1, 24) => {
                    let value = frame_index * 32_771 + i32::from(channel) * 17_003 - 2_000_000;
                    let bytes = value.to_le_bytes();
                    data.extend_from_slice(&bytes[..3]);
                }
                (1, 32) => {
                    let value = frame_index * 65_537 + i32::from(channel) * 31_337 - 6_000_000;
                    data.extend_from_slice(&value.to_le_bytes());
                }
                (3, 32) => {
                    let value = (frame_index as f32 / 256.0) - f32::from(channel) * 0.125;
                    data.extend_from_slice(&value.to_le_bytes());
                }
                _ => unreachable!("test uses a supported source format"),
            }
        }
    }
    push_chunk(&mut body, b"data", &data);
    let mut bytes = Vec::with_capacity(body.len() + 4);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes
}

fn push_chunk(body: &mut Vec<u8>, id: &[u8; 4], payload: &[u8]) {
    body.extend_from_slice(id);
    body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    body.extend_from_slice(payload);
    if payload.len() & 1 != 0 {
        body.push(0x5a);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn set_modified(file: &File, time: SystemTime) {
    file.set_times(std::fs::FileTimes::new().set_modified(time))
        .expect("restore mtime");
}

#[test]
fn fingerprint_matches_independent_sha256_and_decoded_pcm_matches_resident_import() {
    let input_frames = [[i16::MIN, i16::MAX], [16_384, -8_192], [-1, 0]];
    let bytes = pcm16_wav(&input_frames, true);
    let fixture = TempFile::create(&bytes);
    let mut reader = VerifiedWavReader::open(fixture.path(), limits(16)).expect("verified open");
    let decoded = decode_wav(&bytes, Limits::new(4096, 16)).expect("resident decode");
    let actual = reader
        .read_frames(0, input_frames.len())
        .expect("verified decode");
    assert_eq!(actual, decoded.frames());
    assert_eq!(reader.source_frames(), 3);
    assert_eq!(reader.source_channels(), 2);
    assert_eq!(reader.bits_per_sample(), 16);
    assert_eq!(
        hex(reader.fingerprint()),
        "abb71a7aaf6907d20d4ceb11108a451eb85937088fb52867846880dcf13611c0"
    );
    assert_eq!(reader.page_digests().len(), 1);
    assert_eq!(reader.page_digests()[0], *reader.fingerprint());
    assert_eq!(reader.page_manifest_bytes(), 32);
}

#[test]
fn verified_windows_match_resident_bits_for_every_supported_encoding_and_channel_count() {
    for (encoding, bits) in [
        (1, 8),
        (1, 16),
        (1, 24),
        (1, 32),
        (3, 32),
        (3, 64),
        (6, 8),
        (7, 8),
    ] {
        for channels in [1, 2] {
            let bytes = supported_format_wav(encoding, channels, bits);
            let expected = decode_wav(&bytes, Limits::new(bytes.len(), 20_000))
                .expect("resident source decoder");
            let fixture = TempFile::create(&bytes);
            let mut reader = VerifiedWavReader::open(fixture.path(), limits(8193))
                .expect("verified source decoder");
            // Include frame/8 KiB decode-batch/64 KiB raw-page crossings and EOF.
            // 24-bit mono/stereo frames do not divide the staging size.
            for (start, count) in [
                (0, 1),
                (29, 41),
                (191, 8),
                (2039, 4097),
                (10899, 8193),
                (19997, 16),
            ] {
                let mut actual = vec![[4.0; 2]; count];
                let decoded = reader
                    .read_frames_into(start, &mut actual)
                    .expect("verified source window");
                assert_eq!(decoded, count.min(expected.frames().len() - start as usize));
                for (offset, actual) in actual.iter().enumerate() {
                    let expected = expected
                        .frames()
                        .get(start as usize + offset)
                        .copied()
                        .unwrap_or([0.0; 2]);
                    assert_eq!(actual[0].to_bits(), expected[0].to_bits());
                    assert_eq!(actual[1].to_bits(), expected[1].to_bits());
                }
            }
        }
    }
}

#[test]
fn verified_reader_handles_page_crossing_frames_and_resample_halos() {
    let frames = (0..17_000_i32)
        .map(|index| [(index % 32_767) as i16, -((index % 32_767) as i16)])
        .collect::<Vec<_>>();
    let bytes = pcm16_wav(&frames, true);
    let fixture = TempFile::create(&bytes);
    let decoded = decode_wav(&bytes, Limits::new(256 * 1024, 20_000)).expect("decode fixture");
    let mut reader = VerifiedWavReader::open(fixture.path(), limits(1024)).expect("open");
    let byte_data_start = 46_u64;
    let first_crossing_frame = (PAGE_BYTES as u64 - byte_data_start) / 4;
    let mut window = vec![[0.0; 2]; 6];
    reader
        .read_frames_into(first_crossing_frame - 2, &mut window)
        .expect("page-crossing read");
    let start = usize::try_from(first_crossing_frame - 2).expect("index");
    assert_eq!(window, decoded.frames()[start..start + 6]);

    let plan =
        ResamplePlan::new(48_000, 44_100, 17_000, 20_000, 20_000, 64).expect("conversion plan");
    let output_start = first_crossing_frame * 44_100 / 48_000;
    let mut converted = vec![[0.0; 2]; 12];
    let (source_start, source_end) = plan
        .required_source_range(output_start, converted.len())
        .expect("source halo");
    let mut source_scratch = vec![[0.0; 2]; (source_end - source_start) as usize];
    plan.render_verified_wav_window(
        &mut reader,
        output_start,
        &mut converted,
        &mut source_scratch,
    )
    .expect("verified conversion window");
    let mut direct = vec![[0.0; 2]; converted.len()];
    plan.render_window(
        source_start,
        &decoded.frames()[source_start as usize..source_end as usize],
        output_start,
        &mut direct,
    )
    .expect("direct conversion oracle");
    assert_eq!(converted, direct);
}

#[test]
fn same_size_same_mtime_content_edit_is_rejected_and_output_is_cleared() {
    let fixture = TempFile::create(&pcm16_wav(&[[1, -1], [2, -2], [3, -3]], false));
    let mut reader = VerifiedWavReader::open(fixture.path(), limits(8)).expect("open");
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.path())
        .expect("open for mutation");
    let original_modified = file
        .metadata()
        .expect("metadata")
        .modified()
        .expect("mtime");
    file.seek(SeekFrom::Start(44)).expect("seek data");
    file.write_all(&[0xfe, 0x7f]).expect("edit source sample");
    set_modified(&file, original_modified);
    let mut output = [[9.0, -9.0]; 3];
    assert!(matches!(
        reader.read_frames_into(0, &mut output),
        Err(ImportError::SourceChanged)
    ));
    assert_eq!(output, [[0.0; 2]; 3]);
}

#[test]
fn changed_header_truncation_and_path_replacement_have_defined_results() {
    let fixture = TempFile::create(&pcm16_wav(&[[7, -7], [8, -8]], false));
    let mut reader = VerifiedWavReader::open(fixture.path(), limits(8)).expect("open");
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.path())
        .expect("open for header edit");
    let modified = file
        .metadata()
        .expect("metadata")
        .modified()
        .expect("mtime");
    file.seek(SeekFrom::Start(20)).expect("seek sample rate");
    file.write_all(&44_100_u32.to_le_bytes())
        .expect("change header");
    set_modified(&file, modified);
    let mut output = [[2.0, 2.0]; 2];
    assert!(matches!(
        reader.read_frames_into(0, &mut output),
        Err(ImportError::SourceChanged)
    ));
    assert_eq!(output, [[0.0; 2]; 2]);

    let path_replacement = TempFile::create(&pcm16_wav(&[[10, -10]], false));
    let replacement_path = path_replacement.path().to_path_buf();
    let opened_source = TempFile::create(&pcm16_wav(&[[11, -11]], false));
    let mut pinned = VerifiedWavReader::open(opened_source.path(), limits(8)).expect("pin");
    fs::rename(path_replacement.path(), opened_source.path()).expect("replace lexical path");
    assert_eq!(
        pinned.read_frames(0, 1).expect("pinned descriptor"),
        [[11.0 / 32768.0, -11.0 / 32768.0]]
    );
    assert_ne!(replacement_path, opened_source.path());
}

#[test]
fn cancellation_and_all_manifest_window_and_header_caps_are_enforced() {
    let many_frames = (0..34_000_i32)
        .map(|index| {
            let sample = (index % 30_000) as i16;
            [sample, -sample]
        })
        .collect::<Vec<_>>();
    let fixture = TempFile::create(&pcm16_wav(&many_frames, false));
    assert!(matches!(
        VerifiedWavReader::open(
            fixture.path(),
            VerifiedWavReaderLimits::new(WavReaderLimits::new(256 * 1024, 1_000_000, 128, 128), 16)
        ),
        Err(ImportError::LimitExceeded("verified page manifest bytes"))
    ));
    let mut checks = 0;
    assert!(matches!(
        VerifiedWavReader::open_with_cancel(fixture.path(), limits(8), || {
            checks += 1;
            checks == 2
        }),
        Err(ImportError::Cancelled)
    ));

    let wav = TempFile::create(&pcm16_wav(&[[1, -1], [2, -2]], false));
    let tight = VerifiedWavReaderLimits::new(WavReaderLimits::new(1024, 10, 1, 1), 1024);
    assert!(matches!(
        VerifiedWavReader::open(wav.path(), tight),
        Err(ImportError::LimitExceeded("chunk headers"))
    ));
    let mut reader = VerifiedWavReader::open(wav.path(), limits(1)).expect("open");
    let mut output = [[5.0, 5.0]; 2];
    assert!(matches!(
        reader.read_frames_into(0, &mut output),
        Err(ImportError::LimitExceeded("decoded window frames"))
    ));
    assert_eq!(output, [[0.0; 2]; 2]);
    assert!(matches!(
        reader.read_frames(3, 1),
        Err(ImportError::Invalid("window starts beyond source end"))
    ));
    let mut eof = [[1.0, -1.0]; 1];
    assert_eq!(reader.read_frames_into(2, &mut eof).expect("eof"), 0);
    assert_eq!(eof, [[0.0; 2]; 1]);
    assert!(matches!(
        reader.read_frames_into(u64::MAX, &mut eof),
        Err(ImportError::Invalid("requested frame range overflow"))
    ));
    assert_eq!(eof, [[0.0; 2]; 1]);

    let byte_limited = VerifiedWavReaderLimits::new(WavReaderLimits::new(1, 10, 128, 8), 1024);
    assert!(matches!(
        VerifiedWavReader::open(wav.path(), byte_limited),
        Err(ImportError::LimitExceeded("input bytes"))
    ));
    let source_limited = VerifiedWavReaderLimits::new(WavReaderLimits::new(1024, 1, 128, 8), 1024);
    assert!(matches!(
        VerifiedWavReader::open(wav.path(), source_limited),
        Err(ImportError::LimitExceeded("source frames"))
    ));

    let large = TempFile::create(&pcm16_wav(&many_frames, true));
    let mut large_reader = VerifiedWavReader::open(large.path(), limits(8193)).expect("large open");
    // Calibrate a complete short read, rather than assuming a fixed number of
    // per-frame reader calls. A longer read must continue to poll and honor
    // cancellation after a valid prefix has been decoded.
    let mut prefix_checks = 0;
    let mut prefix = [[0.0; 2]; 1];
    large_reader
        .read_frames_into_with_cancel(16_370, &mut prefix, || {
            prefix_checks += 1;
            false
        })
        .expect("short read");
    assert_ne!(prefix[0], [0.0; 2]);
    assert!(prefix_checks > 0);
    let mut cancel_checks = 0;
    let mut partial = vec![[8.0, 8.0]; 8193];
    assert!(matches!(
        large_reader.read_frames_into_with_cancel(16_370, &mut partial, || {
            cancel_checks += 1;
            cancel_checks > prefix_checks
        }),
        Err(ImportError::Cancelled)
    ));
    assert!(partial.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn truncation_is_reported_as_source_change_and_clears_output() {
    let fixture = TempFile::create(&pcm16_wav(&[[1, -1], [2, -2], [3, -3]], false));
    let mut reader = VerifiedWavReader::open(fixture.path(), limits(8)).expect("open");
    OpenOptions::new()
        .write(true)
        .open(fixture.path())
        .expect("open for truncate")
        .set_len(48)
        .expect("truncate");
    let mut output = [[7.0, 7.0]; 3];
    assert!(matches!(
        reader.read_frames_into(0, &mut output),
        Err(ImportError::SourceChanged)
    ));
    assert_eq!(output, [[0.0; 2]; 3]);
}

#[test]
fn nonfinite_float_window_clears_the_entire_verified_output() {
    // The invalid sample follows valid decoded batches and crosses a raw page.
    let mut samples = vec![[0.25, -0.25]; 8193];
    samples[8192][0] = f32::NAN;
    let fixture = TempFile::create(&float32_wav(&samples));
    let mut reader = VerifiedWavReader::open(fixture.path(), limits(samples.len())).expect("open");
    let mut output = vec![[4.0, 4.0]; samples.len()];
    assert!(matches!(
        reader.read_frames_into(0, &mut output),
        Err(ImportError::Invalid("float samples must be finite"))
    ));
    assert!(output.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn sparse_seventeen_million_frame_file_supports_far_verified_conversion() {
    const FRAME_COUNT: u64 = 17_000_000;
    const DATA_OFFSET: u64 = 44;
    let path = temp_path();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("create sparse verified fixture");
    let data_bytes = FRAME_COUNT * 4;
    let file_len = DATA_OFFSET + data_bytes;
    file.set_len(file_len)
        .expect("allocate sparse logical file");
    file.seek(SeekFrom::Start(0)).expect("seek header");
    file.write_all(b"RIFF").expect("riff");
    file.write_all(&((file_len - 8) as u32).to_le_bytes())
        .expect("riff size");
    file.write_all(b"WAVEfmt ").expect("wave fmt");
    file.write_all(&16_u32.to_le_bytes()).expect("fmt size");
    file.write_all(&1_u16.to_le_bytes()).expect("pcm");
    file.write_all(&2_u16.to_le_bytes()).expect("stereo");
    file.write_all(&48_000_u32.to_le_bytes()).expect("rate");
    file.write_all(&192_000_u32.to_le_bytes())
        .expect("byte rate");
    file.write_all(&4_u16.to_le_bytes()).expect("block align");
    file.write_all(&16_u16.to_le_bytes()).expect("bit depth");
    file.write_all(b"data").expect("data id");
    file.write_all(&(data_bytes as u32).to_le_bytes())
        .expect("data size");
    const SOURCE_START: u64 = FRAME_COUNT - 256;
    let mut source_fixture = Vec::new();
    for index in 0..160_i16 {
        let left = index - 80;
        let right = 80 - index;
        source_fixture.extend_from_slice(&left.to_le_bytes());
        source_fixture.extend_from_slice(&right.to_le_bytes());
    }
    file.seek(SeekFrom::Start(DATA_OFFSET + SOURCE_START * 4))
        .expect("seek sparse far data");
    file.write_all(&source_fixture).expect("write far samples");
    drop(file);
    let fixture = TempFile(path);
    let mut source_limits = limits(1024);
    source_limits.reader.max_file_bytes = file_len;
    source_limits.reader.max_source_frames = FRAME_COUNT;
    let mut reader = VerifiedWavReader::open(fixture.path(), source_limits).expect("verified scan");
    assert_eq!(reader.source_frames(), FRAME_COUNT);
    let plan = ResamplePlan::new(48_000, 44_100, FRAME_COUNT, FRAME_COUNT, 16_000_000, 32)
        .expect("large conversion plan");
    let output_start = SOURCE_START * 44_100 / 48_000;
    let mut actual = [[0.0; 2]; 12];
    let (required_start, required_end) = plan
        .required_source_range(output_start, actual.len())
        .expect("required halo");
    let mut scratch = vec![[0.0; 2]; (required_end - required_start) as usize];
    plan.render_verified_wav_window(&mut reader, output_start, &mut actual, &mut scratch)
        .expect("far verified conversion");
    let local_frames = (required_start..required_end)
        .map(|source_index| {
            if (SOURCE_START..SOURCE_START + 160).contains(&source_index) {
                let index = (source_index - SOURCE_START) as i16;
                [
                    f32::from(index - 80) / 32_768.0,
                    f32::from(80 - index) / 32_768.0,
                ]
            } else {
                [0.0, 0.0]
            }
        })
        .collect::<Vec<Frame>>();
    let mut expected = [[0.0; 2]; 12];
    plan.render_window(required_start, &local_frames, output_start, &mut expected)
        .expect("local conversion oracle");
    assert_eq!(actual, expected);
}

#[test]
fn sha256_backend_preserves_legacy_bytes_and_hex_across_padding_and_page_boundaries() {
    for (input, expected) in [
        (
            b"".as_slice(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            b"abc".as_slice(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
    ] {
        let actual: [u8; 32] = Sha256::digest(input).into();
        assert_eq!(hex(&actual), expected);
    }
    let lengths = (0..=130).chain([
        255, 256, 257, 511, 512, 513, 8191, 8192, 8193, 65535, 65536, 65537, 262147,
    ]);
    for length in lengths {
        let input: Vec<u8> = (0..length)
            .map(|i| ((i * 37 + i / 3) & 255) as u8)
            .collect();
        let expected: [u8; 32] = LegacySha256::digest(&input).into();
        let actual: [u8; 32] = Sha256::digest(&input).into();
        assert_eq!(actual, expected, "whole input length {length}");
        assert_eq!(hex(&actual), format!("{:x}", LegacySha256::digest(&input)));
        for width in [1, 3, 4, 17, 63, 64, 65, 4096, 65536] {
            let mut legacy = LegacySha256::new();
            let mut current = Sha256::new();
            for chunk in input.chunks(width) {
                legacy.update(chunk);
                current.update(chunk);
            }
            let legacy: [u8; 32] = legacy.finalize().into();
            let current: [u8; 32] = current.finalize().into();
            assert_eq!(legacy, expected, "legacy length {length}, chunk {width}");
            assert_eq!(current, expected, "current length {length}, chunk {width}");
        }
    }
}

/// Ten raw pages of 16-bit stereo PCM, header in the first page.
fn ten_page_fixture() -> (TempFile, Vec<[i16; 2]>) {
    let frames: Vec<[i16; 2]> = (0..(10 * PAGE_BYTES / 4 - 16))
        .map(|index| [index as i16, (index as i16).wrapping_neg()])
        .collect();
    (TempFile::create(&pcm16_wav(&frames, false)), frames)
}

fn reopen(
    path: &Path,
    limits: VerifiedWavReaderLimits,
    manifest: &crate::VerifiedWavManifest,
    cancelled: bool,
) -> Result<VerifiedWavReader, ImportError> {
    VerifiedWavReader::open_verified_with_cancel(path, limits, manifest.clone(), || cancelled)
}

#[test]
fn reopening_under_a_captured_manifest_reads_only_header_pages() {
    let (fixture, frames) = ten_page_fixture();
    let full = VerifiedWavReader::open(fixture.path(), limits(64)).expect("full open");
    assert_eq!(full.manifest().page_digests().len(), 10);
    assert!(full.raw_page_reads() >= 10, "a full open hashes every page");
    let mut reused =
        reopen(fixture.path(), limits(64), full.manifest(), false).expect("reuse open");
    assert_eq!(
        reused.raw_page_reads(),
        1,
        "only the header page is read and verified"
    );
    assert_eq!(reused.fingerprint(), full.fingerprint());
    assert_eq!(reused.source_frames(), full.source_frames());
    assert_eq!(reused.manifest(), full.manifest());
    // Data pages are still verified lazily: a far window reads its own page.
    let tail = &frames[frames.len() - 4..];
    let mut output = [[0.0; 2]; 4];
    let start = frames.len() as u64 - 4;
    assert_eq!(reused.read_frames_into(start, &mut output).unwrap(), 4);
    for (decoded, [left, right]) in output.iter().zip(tail) {
        assert_eq!(
            *decoded,
            [f32::from(*left) / 32_768.0, f32::from(*right) / 32_768.0]
        );
    }
    assert_eq!(
        reused.raw_page_reads(),
        3,
        "header page plus the window page"
    );
}

#[test]
fn a_changed_file_refuses_a_captured_manifest() {
    let (fixture, _) = ten_page_fixture();
    let full = VerifiedWavReader::open(fixture.path(), limits(64)).expect("full open");
    let manifest = full.manifest().clone();
    drop(full);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.path())
        .expect("open for mutation");
    let original = file.metadata().unwrap().modified().unwrap();
    // A later modification time alone refuses reuse.
    set_modified(&file, original + std::time::Duration::from_secs(5));
    assert!(matches!(
        reopen(fixture.path(), limits(64), &manifest, false),
        Err(ImportError::SourceChanged)
    ));
    // A changed length refuses reuse even with the captured time restored.
    file.set_len(manifest.file_len() - 4).unwrap();
    set_modified(&file, original);
    assert!(matches!(
        reopen(fixture.path(), limits(64), &manifest, false),
        Err(ImportError::SourceChanged)
    ));
}

#[test]
fn identity_preserving_edit_passes_reuse_open_but_fails_its_page_read() {
    let (fixture, frames) = ten_page_fixture();
    let manifest = VerifiedWavReader::open(fixture.path(), limits(64))
        .expect("full open")
        .manifest()
        .clone();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.path())
        .expect("open for mutation");
    let original = file.metadata().unwrap().modified().unwrap();
    // Edit one sample in the last page, keeping length and modification time.
    file.seek(SeekFrom::Start(9 * PAGE_BYTES as u64 + 8))
        .unwrap();
    file.write_all(&[0x55, 0x55]).unwrap();
    set_modified(&file, original);
    // Documented residual risk: identical length and mtime pass the open...
    let mut reader = reopen(fixture.path(), limits(64), &manifest, false)
        .expect("an identity-preserving edit is not visible at open");
    // ...unchanged pages still serve, and the edited page is refused on read.
    let mut early = [[0.0; 2]; 4];
    assert_eq!(reader.read_frames_into(0, &mut early).unwrap(), 4);
    let mut late = [[9.0; 2]; 4];
    assert!(matches!(
        reader.read_frames_into(frames.len() as u64 - 4, &mut late),
        Err(ImportError::SourceChanged)
    ));
    assert_eq!(late, [[0.0; 2]; 4]);
}

#[test]
fn reuse_open_keeps_manifest_limits_and_cancellation() {
    let (fixture, _) = ten_page_fixture();
    let manifest = VerifiedWavReader::open(fixture.path(), limits(64))
        .expect("full open")
        .manifest()
        .clone();
    let tight = VerifiedWavReaderLimits::new(limits(64).reader, manifest.manifest_bytes() - 1);
    assert!(matches!(
        reopen(fixture.path(), tight, &manifest, false),
        Err(ImportError::LimitExceeded("verified page manifest bytes"))
    ));
    assert!(matches!(
        reopen(fixture.path(), limits(64), &manifest, true),
        Err(ImportError::Cancelled)
    ));
}
