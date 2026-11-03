//! Independent codec oracles and shared resident/seekable format regressions.
use crate::{
    ImportError, Limits, VerifiedWavReader, VerifiedWavReaderLimits, WavFrameReader,
    WavReaderLimits, WavTruncation, decode_wav, decode_wav_file_cancellable, recover_wav_file,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 != 0 {
        out.push(0);
    }
}

fn wave(fmt: &[u8], data: &[u8], fact: Option<u32>) -> Vec<u8> {
    let mut chunks = Vec::new();
    chunk(&mut chunks, b"fmt ", fmt);
    if let Some(frames) = fact {
        chunk(&mut chunks, b"fact", &frames.to_le_bytes());
    }
    chunk(&mut chunks, b"data", data);
    let mut result = b"RIFF".to_vec();
    result.extend_from_slice(&(chunks.len() as u32 + 4).to_le_bytes());
    result.extend_from_slice(b"WAVE");
    result.extend_from_slice(&chunks);
    result
}

fn format(encoding: u16, channels: u16, bits: u16) -> Vec<u8> {
    let align = channels * (bits / 8);
    let mut result = Vec::new();
    result.extend_from_slice(&encoding.to_le_bytes());
    result.extend_from_slice(&channels.to_le_bytes());
    result.extend_from_slice(&48000_u32.to_le_bytes());
    result.extend_from_slice(&(48000 * u32::from(align)).to_le_bytes());
    result.extend_from_slice(&align.to_le_bytes());
    result.extend_from_slice(&bits.to_le_bytes());
    result
}

fn extensible(encoding: u16, channels: u16, bits: u16, valid_bits: u16) -> Vec<u8> {
    let mut fmt = format(0xfffe, channels, bits);
    fmt.extend_from_slice(&22_u16.to_le_bytes());
    fmt.extend_from_slice(&valid_bits.to_le_bytes());
    fmt.extend_from_slice(&(if channels == 1 { 4_u32 } else { 3 }).to_le_bytes());
    fmt.extend_from_slice(&encoding.to_le_bytes());
    fmt.extend_from_slice(&[0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]);
    fmt
}

fn assert_readers(bytes: &[u8], expected: &[[f32; 2]]) {
    let audio =
        decode_wav(bytes, Limits::new(bytes.len(), expected.len())).expect("resident decode");
    assert_eq!(audio.frames(), expected);
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "hex-codecs-{}-{}.wav",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, bytes).expect("write fixture");
    let limits = WavReaderLimits::new(
        bytes.len() as u64,
        expected.len() as u64,
        8,
        expected.len() + 2,
    );
    let mut plain = WavFrameReader::open(&path, limits).expect("plain reader");
    let mut verified = VerifiedWavReader::open(&path, VerifiedWavReaderLimits::new(limits, 32))
        .expect("verified reader");
    let mut output = vec![[9.0; 2]; expected.len() + 1];
    plain
        .read_frames_into(1, &mut output)
        .expect("plain window");
    assert_eq!(&output[..expected.len() - 1], &expected[1..]);
    assert_eq!(&output[expected.len() - 1..], &[[0.0; 2]; 2]);
    verified
        .read_frames_into(0, &mut output)
        .expect("verified window");
    assert_eq!(&output[..expected.len()], expected);
    assert_eq!(output[expected.len()], [0.0; 2]);
    std::fs::remove_file(path).expect("remove fixture");
}

fn set_riff_overrun(bytes: &mut [u8], overrun: u32) {
    let declared_size = u32::try_from(bytes.len() - 8).expect("small fixture") + overrun;
    bytes[4..8].copy_from_slice(&declared_size.to_le_bytes());
}

fn pcm16_with_odd_list() -> Vec<u8> {
    let data: Vec<u8> = [-32768_i16, 32767, 16384, -8192, 0, 0, 256, -256]
        .into_iter()
        .flat_map(i16::to_le_bytes)
        .collect();
    let mut bytes = wave(&format(1, 2, 16), &data, None);
    // LIST contents are opaque to the WAV reader; its top-level odd padding is not.
    chunk(&mut bytes, b"LIST", b"INFOsynthetic");
    set_riff_overrun(&mut bytes, 0);
    bytes
}

fn reader_errors(bytes: &[u8], max_header_count: usize) -> [Option<ImportError>; 3] {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "hex-riff-boundary-{}-{}.wav",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, bytes).expect("write fixture");
    let limits = WavReaderLimits::new(bytes.len() as u64, 1024, max_header_count, 1024);
    let errors = [
        decode_wav(bytes, Limits::new(bytes.len(), 1024)).err(),
        WavFrameReader::open(&path, limits).err(),
        VerifiedWavReader::open(&path, VerifiedWavReaderLimits::new(limits, 32)).err(),
    ];
    std::fs::remove_file(path).expect("remove fixture");
    errors
}

fn assert_invalid_readers(bytes: &[u8], reason: &'static str) {
    for (reader, error) in ["resident", "plain", "verified"]
        .into_iter()
        .zip(reader_errors(bytes, 8))
    {
        assert!(
            matches!(error, Some(ImportError::Invalid(actual)) if actual == reason),
            "{reader}: expected {reason:?}, got {error:?}"
        );
    }
}

fn assert_truncated_readers(bytes: &[u8], expected: WavTruncation) {
    for (reader, error) in ["resident", "plain", "verified"]
        .into_iter()
        .zip(reader_errors(bytes, 8))
    {
        assert!(
            matches!(error, Some(ImportError::Truncated(actual)) if actual == expected),
            "{reader}: expected {expected:?}, got {error:?}"
        );
    }
}

/// Explicit recovery of the readable frames, through the public file entry point.
fn recover(bytes: &[u8]) -> Result<(Vec<[f32; 2]>, Option<WavTruncation>), ImportError> {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "hex-wav-recover-{}-{}.wav",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, bytes).expect("write fixture");
    let result = recover_wav_file(&path, Limits::new(bytes.len(), 1024));
    std::fs::remove_file(path).expect("remove fixture");
    result.map(|(decoded, truncation)| (decoded.audio().frames().to_vec(), truncation))
}

fn pcm16(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

#[test]
fn final_odd_chunk_without_pad_byte_decodes_in_all_readers() {
    // One 24-bit mono frame: an odd data chunk that some writers leave unpadded.
    let mut bytes = wave(&format(1, 1, 24), &[0, 0, 0xc0], None);
    assert_eq!(bytes.pop(), Some(0), "fixture pad byte");
    // RIFF size may exclude the absent pad or count it.
    for overrun in [0, 1] {
        set_riff_overrun(&mut bytes, overrun);
        assert_readers(&bytes, &[[-0.5; 2]]);
        assert_eq!(recover(&bytes).expect("recover"), (vec![[-0.5; 2]], None));
    }
    // Any other RIFF claim around a missing pad remains malformed.
    for overrun in [2, 4, 5] {
        set_riff_overrun(&mut bytes, overrun);
        assert_invalid_readers(&bytes, "missing odd-chunk padding byte");
    }
    // A pad missing before a further chunk (or before RIFF end) is still refused.
    let mut inner = wave(&format(1, 1, 24), &[0, 0, 0xc0], None);
    inner.pop();
    inner.extend_from_slice(b"junk\x02\x00\x00\x00ab");
    set_riff_overrun(&mut inner, 0);
    assert!(reader_errors(&inner, 8).iter().all(Option::is_some));
}

#[test]
fn truncated_recording_is_refused_with_its_readable_frames_and_recoverable() {
    let samples = [100_i16, -100, 200, -200, 300, -300, 400, -400];
    let complete = wave(&format(1, 2, 16), &pcm16(&samples), None);
    let expected: Vec<[f32; 2]> = samples
        .chunks_exact(2)
        .map(|pair| [f32::from(pair[0]) / 32768.0, f32::from(pair[1]) / 32768.0])
        .collect();
    // A recorder crash keeps the intended sizes but loses the last six bytes,
    // leaving two complete frames and half of the third.
    let mut cut = complete.clone();
    cut.truncate(complete.len() - 6);
    let report = WavTruncation {
        readable_frames: 2,
        missing_bytes: 6,
        partial_frame_bytes: 2,
    };
    assert_truncated_readers(&cut, report);
    let message = ImportError::Truncated(report).to_string();
    assert!(message.contains("recover readable frames"), "{message}");
    assert_eq!(
        message,
        message.to_lowercase(),
        "app-owned text is lowercase"
    );
    assert_eq!(
        recover(&cut).expect("recover"),
        (expected[..2].to_vec(), Some(report))
    );

    // Placeholder sizes left by an unfinished recording are recovered the same way.
    let mut placeholder = cut.clone();
    placeholder[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    // The data size field follows the 12-byte container and 24-byte fmt chunk.
    placeholder[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
    let (frames, truncation) = recover(&placeholder).expect("recover placeholder");
    assert_eq!(frames, expected[..2]);
    assert_eq!(truncation.map(|report| report.readable_frames), Some(2));
}

#[test]
fn partial_final_frame_is_refused_and_recoverable() {
    // Two 16-bit mono frames plus one stray byte in a complete, padded chunk.
    let mut data = pcm16(&[16384, -16384]);
    data.push(7);
    let bytes = wave(&format(1, 1, 16), &data, None);
    let report = WavTruncation {
        readable_frames: 2,
        missing_bytes: 0,
        partial_frame_bytes: 1,
    };
    assert_truncated_readers(&bytes, report);
    assert_eq!(
        recover(&bytes).expect("recover"),
        (vec![[0.5; 2], [-0.5; 2]], Some(report))
    );
}

#[test]
fn truncation_recovery_still_refuses_malformed_files() {
    let data = pcm16(&[1, 2, 3, 4]);
    let complete = wave(&format(1, 2, 16), &data, None);
    // Data crossing EOF while RIFF claims exactly the file is inconsistent.
    let mut riff_exact = complete.clone();
    riff_exact.truncate(complete.len() - 4);
    set_riff_overrun(&mut riff_exact, 0);
    assert_invalid_readers(&riff_exact, "chunk exceeds RIFF boundary");
    assert!(matches!(
        recover(&riff_exact),
        Err(ImportError::Invalid("chunk exceeds RIFF boundary"))
    ));
    // Cut before the first complete frame: nothing to recover.
    let mut no_frames = complete.clone();
    no_frames.truncate(complete.len() - 6);
    assert_invalid_readers(&no_frames, "truncated data chunk has no complete frames");
    assert!(recover(&no_frames).is_err());
    // A truncated format chunk is a malformed header, not a recording.
    let mut header_cut = complete.clone();
    header_cut.truncate(12 + 8 + 10);
    assert_invalid_readers(&header_cut, "chunk exceeds RIFF boundary");
    assert!(recover(&header_cut).is_err());
    // Data before any format chunk is refused even when truncated.
    let mut no_format = b"RIFF\xff\xff\xff\xffWAVEdata\x10\x00\x00\x00".to_vec();
    no_format.extend_from_slice(&data[..4]);
    assert_invalid_readers(&no_format, "missing format chunk");
    // Block-compressed data has no recoverable frame boundary contract.
    let mut adpcm = wave(&adpcm_format(17, 1, 8, 9), &[0; 16], None);
    adpcm.truncate(adpcm.len() - 4);
    assert!(matches!(
        decode_wav(&adpcm, Limits::new(1024, 100)),
        Err(ImportError::Invalid(_))
    ));
    assert!(matches!(recover(&adpcm), Err(ImportError::Invalid(_))));
}

#[test]
fn exact_four_byte_riff_overrun_preserves_pcm_and_odd_metadata_in_all_readers() {
    let expected = [
        [-1.0, 32767.0 / 32768.0],
        [0.5, -0.25],
        [0.0, 0.0],
        [1.0 / 128.0, -1.0 / 128.0],
    ];
    let mut bytes = pcm16_with_odd_list();
    assert_readers(&bytes, &expected);
    set_riff_overrun(&mut bytes, 4);
    assert_readers(&bytes, &expected);
}

#[test]
fn riff_overrun_compatibility_rejects_other_sizes_in_all_readers() {
    let mut bytes = pcm16_with_odd_list();
    for overrun in [1, 2, 3, 5, 8] {
        set_riff_overrun(&mut bytes, overrun);
        assert_invalid_readers(&bytes, "RIFF boundary exceeds input");
    }
}

#[test]
fn exact_four_byte_riff_overrun_still_requires_complete_chunks_and_padding() {
    // Keep data last so its declared payload crosses physical EOF after truncation.
    let mut truncated_data = wave(&format(1, 2, 16), &[0; 16], None);
    truncated_data.truncate(truncated_data.len() - 4);
    set_riff_overrun(&mut truncated_data, 4);
    // A data chunk cut at EOF is a truncated recording: refused, but recoverable.
    assert_truncated_readers(
        &truncated_data,
        WavTruncation {
            readable_frames: 3,
            missing_bytes: 4,
            partial_frame_bytes: 0,
        },
    );

    let mut truncated_metadata = pcm16_with_odd_list();
    truncated_metadata.truncate(truncated_metadata.len() - 4);
    set_riff_overrun(&mut truncated_metadata, 4);
    assert_invalid_readers(&truncated_metadata, "chunk exceeds RIFF boundary");

    let mut incomplete_header = pcm16_with_odd_list();
    incomplete_header.extend_from_slice(b"LIST\x01\x00\x00");
    set_riff_overrun(&mut incomplete_header, 4);
    assert_invalid_readers(&incomplete_header, "truncated chunk header");

    let mut missing_padding = pcm16_with_odd_list();
    missing_padding.pop();
    set_riff_overrun(&mut missing_padding, 4);
    assert_invalid_readers(&missing_padding, "missing odd-chunk padding byte");
}

#[test]
fn exact_four_byte_riff_overrun_preserves_seekable_header_count_limits() {
    let mut bytes = pcm16_with_odd_list();
    set_riff_overrun(&mut bytes, 4);
    let [resident, plain, verified] = reader_errors(&bytes, 2);
    assert!(resident.is_none(), "resident decode: {resident:?}");
    for (reader, error) in [("plain", plain), ("verified", verified)] {
        assert!(
            matches!(error, Some(ImportError::LimitExceeded("chunk headers"))),
            "{reader}: expected the two-header ceiling, got {error:?}"
        );
    }
}

#[test]
fn pcm8_float64_and_extensible_match_all_readers() {
    let pcm8 = [0, 128, 255];
    let expected = [[-1.0; 2], [0.0; 2], [127.0 / 128.0; 2]];
    assert_readers(&wave(&format(1, 1, 8), &pcm8, None), &expected);
    assert_readers(&wave(&extensible(1, 1, 8, 8), &pcm8, None), &expected);
    let values = [-1.0_f64, 0.5, 0.25, -0.75];
    let data: Vec<u8> = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    for fmt in [format(3, 2, 64), extensible(3, 2, 64, 64)] {
        assert_readers(&wave(&fmt, &data, None), &[[-1.0, 0.5], [0.25, -0.75]]);
    }
    for bits in [16, 24, 32] {
        let mut samples = vec![0; usize::from(bits / 8) * 2];
        samples[usize::from(bits / 8) - 1] = 0x80;
        samples[usize::from(bits / 8) * 2 - 1] = 0x40;
        assert_readers(
            &wave(&extensible(1, 2, bits, bits), &samples, None),
            &[[-1.0, 0.5]],
        );
    }
    assert_readers(
        &wave(
            &extensible(1, 1, 32, 24),
            &(-8388608_i32 * 256).to_le_bytes(),
            None,
        ),
        &[[-1.0; 2]],
    );
    assert_readers(
        &wave(&extensible(3, 1, 32, 32), &0.5_f32.to_le_bytes(), None),
        &[[0.5; 2]],
    );
}

#[test]
fn rejects_malformed_extensible_and_unrepresentable_float64() {
    let fmt = extensible(1, 2, 24, 24);
    for offset in [16, 18, 20, 26, 39] {
        let mut bad = fmt.clone();
        bad[offset] = 0xff;
        assert!(
            decode_wav(&wave(&bad, &[0; 6], None), Limits::new(1024, 4)).is_err(),
            "offset {offset}"
        );
    }
    assert!(decode_wav(&wave(&fmt[..39], &[0; 6], None), Limits::new(1024, 4)).is_err());
    for value in [f64::NAN, f64::INFINITY, f64::MAX] {
        assert!(
            decode_wav(
                &wave(&format(3, 1, 64), &value.to_le_bytes(), None),
                Limits::new(1024, 4)
            )
            .is_err()
        );
    }
}

fn oracle(bytes: &[u8], pcm: &[u8], channels: usize, seekable: bool) {
    let expected: Vec<[f32; 2]> = pcm
        .chunks_exact(channels * 2)
        .map(|frame| {
            let left = f32::from(i16::from_le_bytes([frame[0], frame[1]])) / 32768.0;
            let right = if channels == 1 {
                left
            } else {
                f32::from(i16::from_le_bytes([frame[2], frame[3]])) / 32768.0
            };
            [left, right]
        })
        .collect();
    if seekable {
        assert_readers(bytes, &expected);
    } else {
        let audio =
            decode_wav(bytes, Limits::new(bytes.len(), expected.len())).expect("compressed decode");
        assert_eq!(audio.frames().len(), expected.len());
        for (index, (actual, reference)) in audio.frames().iter().zip(&expected).enumerate() {
            assert_eq!(
                actual,
                reference,
                "frame {index}, channels {channels}, encoding {}",
                u16::from_le_bytes([bytes[20], bytes[21]])
            );
        }
    }
    assert!(matches!(
        decode_wav(bytes, Limits::new(bytes.len(), expected.len() - 1)),
        Err(ImportError::LimitExceeded("decoded frames"))
    ));
}

#[test]
fn common_compressed_encodings_match_independent_ffmpeg_oracles() {
    macro_rules! check {
        ($name:literal, $channels:expr, $seek:expr) => {
            oracle(
                include_bytes!(concat!("../tests/fixtures/", $name, ".wav")),
                include_bytes!(concat!("../tests/fixtures/", $name, ".s16")),
                $channels,
                $seek,
            )
        };
    }
    check!("alaw", 1, true);
    check!("mulaw", 1, true);
    check!("adpcm_ima_wav-1", 1, false);
    check!("adpcm_ima_wav-2", 2, false);
    check!("adpcm_ms-1", 1, false);
    check!("adpcm_ms-2", 2, false);
}

fn adpcm_format(encoding: u16, channels: u16, align: u16, samples: u16) -> Vec<u8> {
    let mut fmt = format(encoding, channels, 4);
    fmt[8..12].copy_from_slice(&24000_u32.to_le_bytes());
    fmt[12..14].copy_from_slice(&align.to_le_bytes());
    fmt.extend_from_slice(&(if encoding == 2 { 8_u16 } else { 2 }).to_le_bytes());
    fmt.extend_from_slice(&samples.to_le_bytes());
    if encoding == 2 {
        fmt.extend_from_slice(&1_u16.to_le_bytes());
        fmt.extend_from_slice(&256_i16.to_le_bytes());
        fmt.extend_from_slice(&0_i16.to_le_bytes());
    }
    fmt
}

#[test]
fn adpcm_fact_padding_missing_fact_custom_coefficients_and_bad_headers() {
    let fmt = adpcm_format(17, 1, 8, 9);
    let data = [0, 0, 0, 0, 0x11, 0x11, 0x11, 0x11];
    let full =
        decode_wav(&wave(&fmt, &data, None), Limits::new(1024, 9)).expect("ima without fact");
    assert_eq!(full.frames().len(), 9);
    let trimmed =
        decode_wav(&wave(&fmt, &data, Some(3)), Limits::new(1024, 3)).expect("trim padded block");
    assert_eq!(trimmed.frames(), &full.frames()[..3]);
    for count in [0, 10, u32::MAX] {
        assert!(decode_wav(&wave(&fmt, &data, Some(count)), Limits::new(1024, 100)).is_err());
    }
    for offset in [2, 3] {
        let mut bad = data;
        bad[offset] = 255;
        assert!(decode_wav(&wave(&fmt, &bad, None), Limits::new(1024, 100)).is_err());
    }
    let mut bad = fmt.clone();
    bad[18..20].copy_from_slice(&10_u16.to_le_bytes());
    assert!(decode_wav(&wave(&bad, &data, None), Limits::new(1024, 100)).is_err());
    assert!(decode_wav(&wave(&fmt, &data[..7], None), Limits::new(1024, 100)).is_err());

    let fmt = adpcm_format(2, 1, 8, 4);
    // Original single-coefficient predictor: previous sample + signed nibble*delta.
    let data = [0, 16, 0, 100, 0, 50, 0, 0x1f];
    let audio = decode_wav(&wave(&fmt, &data, Some(4)), Limits::new(1024, 4))
        .expect("custom microsoft coefficients");
    assert_eq!(
        audio.frames(),
        &[
            [50.0 / 32768.0; 2],
            [100.0 / 32768.0; 2],
            [116.0 / 32768.0; 2],
            [100.0 / 32768.0; 2]
        ]
    );
    for (offset, value) in [(0, 1), (1, 0)] {
        let mut bad = data;
        bad[offset] = value;
        assert!(decode_wav(&wave(&fmt, &bad, None), Limits::new(1024, 4)).is_err());
    }
}

#[test]
fn block_compressed_sources_are_refused_by_random_access_readers() {
    let bytes = wave(&adpcm_format(17, 1, 8, 9), &[0; 8], None);
    let path = std::env::temp_dir().join(format!("hex-adpcm-random-{}.wav", std::process::id()));
    std::fs::write(&path, &bytes).expect("write fixture");
    let limits = WavReaderLimits::new(1024, 100, 8, 100);
    assert!(matches!(
        WavFrameReader::open(&path, limits),
        Err(ImportError::Invalid(
            "adpcm is supported by resident import only"
        ))
    ));
    assert!(matches!(
        VerifiedWavReader::open(&path, VerifiedWavReaderLimits::new(limits, 32)),
        Err(ImportError::Invalid(
            "adpcm is supported by resident import only"
        ))
    ));
    std::fs::remove_file(path).expect("remove fixture");
}

#[test]
fn cancellable_file_decode_matches_plain_decode_and_stops_when_cancelled() {
    // More than one cancellation window, so the chunked loop is exercised.
    let frames = crate::CANCEL_CHECK_FRAMES + 3;
    let data: Vec<u8> = (0..frames)
        .flat_map(|index| ((index % 1_000) as i16 * 16).to_le_bytes())
        .collect();
    let bytes = wave(&format(1, 1, 16), &data, None);
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "hex-cancellable-decode-{}-{}.wav",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, &bytes).expect("write fixture");
    let limits = Limits::new(bytes.len(), frames);
    let expected = decode_wav(&bytes, limits).expect("resident decode");
    let running = AtomicBool::new(false);
    let decoded = decode_wav_file_cancellable(&path, limits, &running).expect("decode");
    assert_eq!(decoded.audio().frames(), expected.frames());
    let cancel = AtomicBool::new(true);
    assert!(matches!(
        decode_wav_file_cancellable(&path, limits, &cancel),
        Err(ImportError::Cancelled)
    ));
    std::fs::remove_file(&path).expect("remove fixture");
}
