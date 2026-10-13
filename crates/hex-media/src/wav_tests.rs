//! Independent codec oracles and shared resident/seekable format regressions.
use crate::{
    ImportError, Limits, VerifiedWavReader, VerifiedWavReaderLimits, WavFrameReader,
    WavReaderLimits, decode_wav,
};
use std::sync::atomic::{AtomicUsize, Ordering};

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
