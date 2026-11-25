//! Unit, streaming, >4 GiB RF64/BW64, AIFF/AIFC, and FLAC codec tests.
use crate::{
    ImportError, Limits, MediaFormat, VerifiedWavReader, VerifiedWavReaderLimits, WavFrameReader,
    WavReaderLimits, WavTruncation, aiff, decode_audio, decode_audio_file, detect_format, flac,
    recover_wav_file, wav,
};
use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::atomic::{AtomicUsize, Ordering},
};

fn temp_file(ext: &str, bytes: &[u8]) -> std::path::PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "hex-multicodec-{}-{}.{ext}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, bytes).expect("write fixture");
    path
}

fn wav_fmt_pcm16(channels: u16, sample_rate: u32) -> Vec<u8> {
    let align = channels * 2;
    let mut out = Vec::with_capacity(16);
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * u32::from(align)).to_le_bytes());
    out.extend_from_slice(&align.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out
}

fn build_rf64(magic: &[u8; 4], fmt: &[u8], data: &[u8]) -> Vec<u8> {
    // 12 (container) + 8 + 28 (ds64) + 8 + fmt.len() + 8 + data.len() (+ pad)
    let fmt_pad = fmt.len() & 1;
    let data_pad = data.len() & 1;
    let riff_size = (4 + 8 + 28 + 8 + fmt.len() + fmt_pad + 8 + data.len() + data_pad) as u64;
    let data_size = data.len() as u64;
    let mut out = Vec::new();
    out.extend_from_slice(magic);
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    // ds64 chunk
    out.extend_from_slice(b"ds64");
    out.extend_from_slice(&28_u32.to_le_bytes());
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(&data_size.to_le_bytes());
    out.extend_from_slice(&0_u64.to_le_bytes()); // sample_count
    out.extend_from_slice(&0_u32.to_le_bytes()); // table_length
    // fmt chunk
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    out.extend_from_slice(fmt);
    if fmt_pad == 1 {
        out.push(0);
    }
    // data chunk with 0xFFFF_FFFF marker resolved via ds64
    out.extend_from_slice(b"data");
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(data);
    if data_pad == 1 {
        out.push(0);
    }
    out
}

fn build_aiff(
    form_type: &[u8; 4],
    channels: u16,
    sample_rate: u32,
    sample_size: u16,
    num_frames: u32,
    compression: Option<&[u8; 4]>,
    audio: &[u8],
) -> Vec<u8> {
    let mut chunks = Vec::new();
    // COMM chunk
    let mut comm = Vec::new();
    comm.extend_from_slice(&channels.to_be_bytes());
    comm.extend_from_slice(&num_frames.to_be_bytes());
    comm.extend_from_slice(&sample_size.to_be_bytes());
    comm.extend_from_slice(&aiff::encode_ieee_extended_rate(sample_rate));
    if let Some(comp) = compression {
        comm.extend_from_slice(comp);
        comm.push(0); // empty Pascal compression name
        comm.push(0); // pad to even
    }
    chunks.extend_from_slice(b"COMM");
    chunks.extend_from_slice(&(comm.len() as u32).to_be_bytes());
    chunks.extend_from_slice(&comm);
    if comm.len() % 2 == 1 {
        chunks.push(0);
    }
    // SSND chunk
    let ssnd_len = 8 + audio.len();
    chunks.extend_from_slice(b"SSND");
    chunks.extend_from_slice(&(ssnd_len as u32).to_be_bytes());
    chunks.extend_from_slice(&0_u32.to_be_bytes()); // offset
    chunks.extend_from_slice(&0_u32.to_be_bytes()); // block_size
    chunks.extend_from_slice(audio);
    if ssnd_len % 2 == 1 {
        chunks.push(0);
    }

    let mut out = Vec::new();
    out.extend_from_slice(b"FORM");
    out.extend_from_slice(&((4 + chunks.len()) as u32).to_be_bytes());
    out.extend_from_slice(form_type);
    out.extend_from_slice(&chunks);
    out
}

#[test]
fn rf64_and_bw64_decode_stream_and_verify_over_64bit_sizes() {
    let samples: Vec<i16> = [-32768, 16384, -8192, 32767, 0, -1, 4096, -4096].to_vec();
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let expected: Vec<[f32; 2]> = samples
        .chunks_exact(2)
        .map(|p| [f32::from(p[0]) / 32_768.0, f32::from(p[1]) / 32_768.0])
        .collect();

    for (magic, ext, expected_fmt) in [
        (b"RF64", "rf64", MediaFormat::Rf64),
        (b"BW64", "bw64", MediaFormat::Bw64),
    ] {
        let bytes = build_rf64(magic, &wav_fmt_pcm16(2, 48_000), &data);
        assert_eq!(detect_format(&bytes), Some(expected_fmt));
        let audio = decode_audio(&bytes, Limits::new(bytes.len(), 64)).unwrap();
        assert_eq!(audio.frames(), expected.as_slice());

        let path = temp_file(ext, &bytes);
        let decoded = decode_audio_file(&path, Limits::new(bytes.len(), 64)).unwrap();
        assert_eq!(decoded.sample_rate(), 48_000);
        assert_eq!(decoded.frames(), expected.len());
        assert_eq!(decoded.audio().frames(), expected.as_slice());

        let limits = WavReaderLimits::new(bytes.len() as u64, 64, 16, 64);
        let mut plain = WavFrameReader::open(&path, limits).unwrap();
        let mut verified =
            VerifiedWavReader::open(&path, VerifiedWavReaderLimits::new(limits, 1024)).unwrap();
        let mut out = vec![[0.0; 2]; expected.len()];
        assert_eq!(plain.read_frames_into(0, &mut out).unwrap(), expected.len());
        assert_eq!(out, expected);
        assert_eq!(
            verified.read_frames_into(0, &mut out).unwrap(),
            expected.len()
        );
        assert_eq!(out, expected);

        // Complete verification pass covers all pages cleanly.
        let mut cov = verified.track_complete_verification(1024).unwrap();
        cov.read_frames_into_with_cancel(0, &mut out, || false)
            .unwrap();
        cov.finish(|| false).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    // Virtual >4 GiB RF64 scan test (without writing 5 GiB to disk!).
    struct VirtualRf64 {
        header: Vec<u8>,
        total_len: u64,
        pos: u64,
    }
    impl Read for VirtualRf64 {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.pos >= self.total_len {
                return Ok(0);
            }
            let avail = (self.total_len - self.pos).min(buf.len() as u64) as usize;
            for (i, byte) in buf[..avail].iter_mut().enumerate() {
                let abs = self.pos + i as u64;
                *byte = self.header.get(abs as usize).copied().unwrap_or(0);
            }
            self.pos += avail as u64;
            Ok(avail)
        }
    }
    impl Seek for VirtualRf64 {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            let next = match pos {
                SeekFrom::Start(p) => p as i128,
                SeekFrom::Current(d) => self.pos as i128 + i128::from(d),
                SeekFrom::End(d) => self.total_len as i128 + i128::from(d),
            };
            if next < 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "negative seek"));
            }
            self.pos = next as u64;
            Ok(self.pos)
        }
    }

    let five_gib_data: u64 = 5 * 1024 * 1024 * 1024;
    let fmt = wav_fmt_pcm16(2, 48_000);
    let total_len: u64 = 12 + 8 + 28 + 8 + fmt.len() as u64 + 8 + five_gib_data;
    let riff_size = total_len - 8;
    let mut header = Vec::new();
    header.extend_from_slice(b"RF64");
    header.extend_from_slice(&u32::MAX.to_le_bytes());
    header.extend_from_slice(b"WAVE");
    header.extend_from_slice(b"ds64");
    header.extend_from_slice(&28_u32.to_le_bytes());
    header.extend_from_slice(&riff_size.to_le_bytes());
    header.extend_from_slice(&five_gib_data.to_le_bytes());
    header.extend_from_slice(&0_u64.to_le_bytes());
    header.extend_from_slice(&0_u32.to_le_bytes());
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    header.extend_from_slice(&fmt);
    header.extend_from_slice(b"data");
    header.extend_from_slice(&u32::MAX.to_le_bytes());

    let mut vreader = VirtualRf64 {
        header,
        total_len,
        pos: 0,
    };
    let layout = wav::scan(&mut vreader, total_len, 16).unwrap();
    assert_eq!(layout.data_bytes, five_gib_data);
    assert_eq!(layout.frames, five_gib_data / 4);

    // Malformed RF64: missing ds64 first chunk must be refused
    let mut bad = build_rf64(b"RF64", &wav_fmt_pcm16(2, 48_000), &data);
    bad[12..16].copy_from_slice(b"JUNK");
    assert!(matches!(
        decode_audio(&bad, Limits::new(bad.len(), 64)),
        Err(ImportError::Invalid("RF64/BW64 requires ds64 first chunk"))
    ));
}

#[test]
fn aiff_and_aifc_encodings_stream_recover_and_reject_malformed() {
    let expected_stereo = vec![[-1.0_f32, 0.5_f32], [0.25_f32, -0.5_f32]];

    // 1. Standard big-endian 16-bit AIFF
    let be16: Vec<u8> = [-32768_i16, 16384, 8192, -16384]
        .into_iter()
        .flat_map(i16::to_be_bytes)
        .collect();
    let aiff_bytes = build_aiff(b"AIFF", 2, 48_000, 16, 2, None, &be16);
    assert_eq!(detect_format(&aiff_bytes), Some(MediaFormat::Aiff));
    let decoded = decode_audio(&aiff_bytes, Limits::new(aiff_bytes.len(), 16)).unwrap();
    assert_eq!(decoded.frames(), expected_stereo.as_slice());

    // 2. AIFC little-endian sowt 16-bit
    let le16: Vec<u8> = [-32768_i16, 16384, 8192, -16384]
        .into_iter()
        .flat_map(i16::to_le_bytes)
        .collect();
    let sowt_bytes = build_aiff(b"AIFC", 2, 44_100, 16, 2, Some(b"sowt"), &le16);
    assert_eq!(detect_format(&sowt_bytes), Some(MediaFormat::Aifc));
    let decoded_sowt = decode_audio(&sowt_bytes, Limits::new(sowt_bytes.len(), 16)).unwrap();
    assert_eq!(decoded_sowt.frames(), expected_stereo.as_slice());

    // 3. AIFC big-endian fl32
    let fl32: Vec<u8> = [-1.0_f32, 0.5, 0.25, -0.5]
        .into_iter()
        .flat_map(f32::to_be_bytes)
        .collect();
    let fl32_bytes = build_aiff(b"AIFC", 2, 96_000, 32, 2, Some(b"fl32"), &fl32);
    let path = temp_file("aifc", &fl32_bytes);
    let limits = WavReaderLimits::new(fl32_bytes.len() as u64, 16, 16, 16);
    let mut plain = WavFrameReader::open(&path, limits).unwrap();
    let mut verified =
        VerifiedWavReader::open(&path, VerifiedWavReaderLimits::new(limits, 1024)).unwrap();
    assert_eq!(verified.sample_rate(), 96_000);
    assert_eq!(verified.source_frames(), 2);
    let mut out = vec![[0.0; 2]; 2];
    assert_eq!(plain.read_frames_into(0, &mut out).unwrap(), 2);
    assert_eq!(out, expected_stereo);
    assert_eq!(verified.read_frames_into(0, &mut out).unwrap(), 2);
    assert_eq!(out, expected_stereo);
    std::fs::remove_file(&path).unwrap();

    // 4. Truncated AIFF reports ImportError::Truncated and recovers readable whole frames
    let mut trunc = aiff_bytes.clone();
    trunc.truncate(trunc.len() - 4); // drop second stereo frame
    assert!(matches!(
        decode_audio(&trunc, Limits::new(trunc.len(), 16)),
        Err(ImportError::Truncated(WavTruncation {
            readable_frames: 1,
            missing_bytes: 4,
            partial_frame_bytes: 0,
        }))
    ));
    let tpath = temp_file("aiff", &trunc);
    let (rec, report) = recover_wav_file(&tpath, Limits::new(trunc.len(), 16)).unwrap();
    assert_eq!(rec.audio().frames(), &[[-1.0_f32, 0.5_f32]]);
    assert!(report.is_some());
    std::fs::remove_file(&tpath).unwrap();

    // 5. Malformed AIFF: unsupported compression and NaN float refused
    let bad_comp = build_aiff(b"AIFC", 2, 48_000, 16, 2, Some(b"ima4"), &be16);
    assert!(decode_audio(&bad_comp, Limits::new(bad_comp.len(), 16)).is_err());

    let nan_fl32: Vec<u8> = [f32::NAN, 0.0]
        .into_iter()
        .flat_map(f32::to_be_bytes)
        .collect();
    let nan_bytes = build_aiff(b"AIFC", 2, 48_000, 32, 1, Some(b"fl32"), &nan_fl32);
    assert!(matches!(
        decode_audio(&nan_bytes, Limits::new(nan_bytes.len(), 16)),
        Err(ImportError::Invalid("float samples must be finite"))
    ));
}

#[test]
fn flac_decodes_streams_verifies_pages_and_rejects_corruption() {
    let total_frames = 300_usize;
    let left: Vec<i32> = (0..total_frames)
        .map(|i| ((i as i32 * 97) % 30_000) - 15_000)
        .collect();
    let right: Vec<i32> = (0..total_frames)
        .map(|i| ((i as i32 * -131) % 30_000) + 5_000)
        .collect();
    let expected: Vec<[f32; 2]> = (0..total_frames)
        .map(|i| [left[i] as f32 / 32_768.0, right[i] as f32 / 32_768.0])
        .collect();

    // Test both VERBATIM and FIXED+Rice2 with SEEKTABLE across multiple frames
    for use_fixed in [false, true] {
        let bytes = flac::encode_test_flac(
            48_000,
            2,
            16,
            64,
            &[left.clone(), right.clone()],
            true,
            use_fixed,
        );
        assert_eq!(detect_format(&bytes), Some(MediaFormat::Flac));
        let audio = decode_audio(&bytes, Limits::new(bytes.len(), total_frames)).unwrap();
        assert_eq!(audio.frames(), expected.as_slice());

        let path = temp_file("flac", &bytes);
        let limits = WavReaderLimits::new(bytes.len() as u64, total_frames as u64, 16, 128);
        let mut plain = WavFrameReader::open(&path, limits).unwrap();
        let mut verified =
            VerifiedWavReader::open(&path, VerifiedWavReaderLimits::new(limits, 4096)).unwrap();
        assert_eq!(verified.sample_rate(), 48_000);
        assert_eq!(verified.source_frames(), total_frames as u64);

        // Random-access seek across frame boundaries (e.g., frame 50..180, then backward to 10..40)
        for (start, len) in [(50_u64, 110_usize), (160, 100), (10, 30), (260, 60)] {
            let mut p_buf = vec![[9.0; 2]; len];
            let mut v_buf = vec![[9.0; 2]; len];
            let p_n = plain.read_frames_into(start, &mut p_buf).unwrap();
            let v_n = verified.read_frames_into(start, &mut v_buf).unwrap();
            let want_n = (total_frames - start as usize).min(len);
            assert_eq!(p_n, want_n);
            assert_eq!(v_n, want_n);
            assert_eq!(
                &p_buf[..want_n],
                &expected[start as usize..start as usize + want_n]
            );
            assert_eq!(
                &v_buf[..want_n],
                &expected[start as usize..start as usize + want_n]
            );
            assert_eq!(&v_buf[want_n..], &vec![[0.0; 2]; len - want_n][..]);
        }

        // Complete SHA-256 verification coverage pass
        let mut cov = verified.track_complete_verification(4096).unwrap();
        let mut buf = vec![[0.0; 2]; 64];
        cov.read_frames_into_with_cancel(0, &mut buf, || false)
            .unwrap();
        cov.finish(|| false).unwrap();

        // Corrupt last byte (CRC-16) -> refused cleanly!
        let mut corrupt_crc = bytes.clone();
        *corrupt_crc.last_mut().unwrap() ^= 0xFF;
        assert!(matches!(
            decode_audio(&corrupt_crc, Limits::new(corrupt_crc.len(), total_frames)),
            Err(ImportError::Invalid("FLAC frame footer CRC-16 mismatch"))
        ));

        // Truncated FLAC stream -> refused cleanly!
        let trunc = &bytes[..bytes.len() - 20];
        assert!(decode_audio(trunc, Limits::new(trunc.len(), total_frames)).is_err());

        std::fs::remove_file(&path).unwrap();
    }
}
