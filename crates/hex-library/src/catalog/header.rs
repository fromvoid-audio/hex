//! Lightweight, zero-extra-open audio header metadata parser.
//!
//! Inspects only the head buffer already read during content fingerprinting
//! (up to `FINGERPRINT_BLOCK` = 64 KiB). Never performs disk I/O or audio decoding.
//! Unrecognized, truncated, or malformed bytes return `None` without failing a scan.

/// Container/sample codec recognized from the leading audio file header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum AudioCodec {
    Unknown = 0,
    WavPcm = 1,
    WavFloat = 2,
    Rf64Pcm = 3,
    Rf64Float = 4,
    AiffPcm = 5,
    AifcFloat = 6,
    Flac = 7,
}

impl AudioCodec {
    pub const fn from_u8(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Unknown),
            1 => Some(Self::WavPcm),
            2 => Some(Self::WavFloat),
            3 => Some(Self::Rf64Pcm),
            4 => Some(Self::Rf64Float),
            5 => Some(Self::AiffPcm),
            6 => Some(Self::AifcFloat),
            7 => Some(Self::Flac),
            _ => None,
        }
    }

    /// Lowercase display label suitable for browser badges and `codec:` search tokens.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::WavPcm => "wav",
            Self::WavFloat => "wav-f",
            Self::Rf64Pcm => "rf64",
            Self::Rf64Float => "rf64-f",
            Self::AiffPcm => "aiff",
            Self::AifcFloat => "aifc",
            Self::Flac => "flac",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "wav" | "wav-pcm" => Some(Self::WavPcm),
            "wav-f" | "wav-float" => Some(Self::WavFloat),
            "rf64" | "bw64" | "rf64-pcm" => Some(Self::Rf64Pcm),
            "rf64-f" | "rf64-float" => Some(Self::Rf64Float),
            "aif" | "aiff" => Some(Self::AiffPcm),
            "aifc" => Some(Self::AifcFloat),
            "flac" => Some(Self::Flac),
            _ => None,
        }
    }
}

/// Lightweight audio header facts captured alongside content fingerprinting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioHeaderMetadata {
    pub sample_rate: u32,
    pub channels: u16,
    pub bit_depth: u16,
    pub frame_count: u64,
    pub codec: AudioCodec,
}

impl AudioHeaderMetadata {
    /// Duration in whole milliseconds, using checked 128-bit arithmetic.
    pub const fn duration_ms(&self) -> u64 {
        if self.sample_rate == 0 {
            return 0;
        }
        let ms = (self.frame_count as u128).saturating_mul(1_000) / (self.sample_rate as u128);
        if ms > u64::MAX as u128 {
            u64::MAX
        } else {
            ms as u64
        }
    }

    /// Duration in seconds as a finite non-negative `f64`.
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frame_count as f64 / f64::from(self.sample_rate)
        }
    }
}

use std::io::{Read, Seek, SeekFrom};

const MAX_HEADER_CHUNKS: usize = 128;

/// Standard `KSDATAFORMAT_SUBTYPE_*` GUID tail (bytes 2..16) shared by PCM (`0x0001`)
/// and IEEE float (`0x0003`) in `WAVEFORMATEXTENSIBLE`.
const WAVE_EXTENSIBLE_GUID_TAIL: [u8; 14] = [
    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

/// Parse lightweight audio header metadata from the leading bytes already read
/// for content fingerprinting. Returns `None` for non-audio or truncated headers.
pub fn parse_audio_header(head: &[u8]) -> Option<AudioHeaderMetadata> {
    if head.len() < 12 {
        return None;
    }
    match &head[0..4] {
        b"RIFF" if &head[8..12] == b"WAVE" => parse_riff_wave(head, false).0,
        b"RF64" | b"BW64" if &head[8..12] == b"WAVE" => parse_riff_wave(head, true).0,
        b"FORM" if &head[8..12] == b"AIFF" => parse_aiff(head, false),
        b"FORM" if &head[8..12] == b"AIFC" => parse_aiff(head, true),
        b"fLaC" => parse_flac(head),
        _ => None,
    }
}

/// Like [`parse_audio_header`], with a bounded seek-only fallback when a RIFF/RF64
/// WAVE `fmt ` or `data` chunk lies beyond the leading 64 KiB fingerprint buffer.
pub(super) fn parse_audio_header_with_reader(
    head: &[u8],
    reader: &mut (impl Read + Seek),
    file_size: u64,
) -> Option<AudioHeaderMetadata> {
    if head.len() < 12 {
        return None;
    }
    match &head[0..4] {
        b"RIFF" if &head[8..12] == b"WAVE" => {
            let (meta, complete) = parse_riff_wave(head, false);
            if complete || file_size <= head.len() as u64 {
                meta
            } else {
                parse_riff_wave_reader(reader, file_size, false).or(meta)
            }
        }
        b"RF64" | b"BW64" if &head[8..12] == b"WAVE" => {
            let (meta, complete) = parse_riff_wave(head, true);
            if complete || file_size <= head.len() as u64 {
                meta
            } else {
                parse_riff_wave_reader(reader, file_size, true).or(meta)
            }
        }
        b"FORM" if &head[8..12] == b"AIFF" => parse_aiff(head, false),
        b"FORM" if &head[8..12] == b"AIFC" => parse_aiff(head, true),
        b"fLaC" => parse_flac(head),
        _ => None,
    }
}

struct WaveFmt {
    sample_rate: u32,
    channels: u16,
    bit_depth: u16,
    block_align: u16,
    is_float: bool,
}

fn parse_wave_fmt(body: &[u8]) -> Option<WaveFmt> {
    if body.len() < 16 {
        return None;
    }
    let format_tag = u16::from_le_bytes(body[0..2].try_into().ok()?);
    let channels = u16::from_le_bytes(body[2..4].try_into().ok()?);
    let sample_rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
    let block_align = u16::from_le_bytes(body[12..14].try_into().ok()?);
    let bit_depth = u16::from_le_bytes(body[14..16].try_into().ok()?);
    let is_float = match format_tag {
        1 => false,
        3 => true,
        0xFFFE => {
            if body.len() < 40 {
                return None;
            }
            let cb_size = u16::from_le_bytes(body[16..18].try_into().ok()?);
            if cb_size < 22 || body[26..40] != WAVE_EXTENSIBLE_GUID_TAIL {
                return None;
            }
            let sub_tag = u16::from_le_bytes(body[24..26].try_into().ok()?);
            match sub_tag {
                1 => false,
                3 => true,
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(WaveFmt {
        sample_rate,
        channels,
        bit_depth,
        block_align,
        is_float,
    })
}

fn finish_wave_metadata(
    fmt: Option<WaveFmt>,
    data_size: Option<u64>,
    is_rf64: bool,
) -> Option<AudioHeaderMetadata> {
    let fmt = fmt?;
    if fmt.sample_rate == 0 || fmt.channels == 0 || fmt.bit_depth == 0 {
        return None;
    }
    let frame_count = match (data_size, fmt.block_align) {
        (Some(bytes), align) if align > 0 => bytes / u64::from(align),
        _ => 0,
    };
    let codec = match (is_rf64, fmt.is_float) {
        (false, false) => AudioCodec::WavPcm,
        (false, true) => AudioCodec::WavFloat,
        (true, false) => AudioCodec::Rf64Pcm,
        (true, true) => AudioCodec::Rf64Float,
    };
    Some(AudioHeaderMetadata {
        sample_rate: fmt.sample_rate,
        channels: fmt.channels,
        bit_depth: fmt.bit_depth,
        frame_count,
        codec,
    })
}

fn parse_riff_wave(head: &[u8], is_rf64: bool) -> (Option<AudioHeaderMetadata>, bool) {
    let mut offset = 12_usize;
    let mut fmt: Option<WaveFmt> = None;
    let mut ds64_data_size: Option<u64> = None;
    let mut data_size: Option<u64> = None;

    for _ in 0..MAX_HEADER_CHUNKS {
        let Some(header_end) = offset.checked_add(8) else {
            break;
        };
        if header_end > head.len() {
            break;
        }
        let id = &head[offset..offset + 4];
        let Ok(size_bytes) = head[offset + 4..offset + 8].try_into() else {
            break;
        };
        let chunk_size = u32::from_le_bytes(size_bytes);
        let body_start = header_end;
        let body_end = body_start
            .saturating_add(chunk_size as usize)
            .min(head.len());
        let body = &head[body_start..body_end];

        match id {
            b"ds64" if is_rf64 && body.len() >= 16 => {
                if let Ok(raw) = body[8..16].try_into() {
                    ds64_data_size = Some(u64::from_le_bytes(raw));
                }
            }
            b"fmt " => {
                // Only reject an unsupported codec when the chunk body is intact;
                // if `fmt ` was truncated at the 64 KiB boundary, allow the reader
                // fallback to read the full chunk.
                let body_complete = body_start.saturating_add(chunk_size as usize) <= head.len();
                match parse_wave_fmt(body) {
                    Some(parsed) => fmt = Some(parsed),
                    None if body_complete => return (None, true),
                    None => {}
                }
            }
            b"data" => {
                data_size = Some(if is_rf64 && chunk_size == u32::MAX {
                    ds64_data_size.unwrap_or(0)
                } else {
                    u64::from(chunk_size)
                });
                if fmt.is_some() {
                    break;
                }
            }
            _ => {}
        }

        let Some(padded) = u64::from(chunk_size).checked_add(u64::from(chunk_size & 1)) else {
            break;
        };
        let Some(next) = u64::try_from(body_start)
            .ok()
            .and_then(|s| s.checked_add(padded))
        else {
            break;
        };
        let Ok(next_usize) = usize::try_from(next) else {
            break;
        };
        if next_usize <= offset || next_usize > head.len() {
            break;
        }
        offset = next_usize;
    }

    let complete = fmt.is_some() && data_size.is_some();
    (finish_wave_metadata(fmt, data_size, is_rf64), complete)
}

fn parse_riff_wave_reader(
    reader: &mut (impl Read + Seek),
    file_size: u64,
    is_rf64: bool,
) -> Option<AudioHeaderMetadata> {
    let mut offset = 12_u64;
    let mut fmt: Option<WaveFmt> = None;
    let mut ds64_data_size: Option<u64> = None;
    let mut data_size: Option<u64> = None;
    let mut header = [0_u8; 8];
    let mut scratch = [0_u8; 40];

    for _ in 0..MAX_HEADER_CHUNKS {
        let body_start = offset.checked_add(8)?;
        if body_start > file_size {
            break;
        }
        reader.seek(SeekFrom::Start(offset)).ok()?;
        reader.read_exact(&mut header).ok()?;
        let id = &header[0..4];
        let chunk_size = u32::from_le_bytes(header[4..8].try_into().ok()?);
        let available = file_size.saturating_sub(body_start);
        let chunk_len = u64::from(chunk_size).min(available);

        match id {
            b"ds64" if is_rf64 && chunk_len >= 16 => {
                reader.read_exact(&mut scratch[..16]).ok()?;
                ds64_data_size = Some(u64::from_le_bytes(scratch[8..16].try_into().ok()?));
            }
            b"fmt " => {
                let to_read = (chunk_len as usize).min(scratch.len());
                reader.read_exact(&mut scratch[..to_read]).ok()?;
                fmt = Some(parse_wave_fmt(&scratch[..to_read])?);
            }
            b"data" => {
                data_size = Some(if is_rf64 && chunk_size == u32::MAX {
                    ds64_data_size.unwrap_or(0)
                } else {
                    u64::from(chunk_size)
                });
                if fmt.is_some() {
                    break;
                }
            }
            _ => {}
        }

        let effective_size = if is_rf64 && id == b"data" && chunk_size == u32::MAX {
            ds64_data_size?
        } else {
            u64::from(chunk_size)
        };
        let padded = effective_size.checked_add(effective_size & 1)?;
        let next = body_start.checked_add(padded)?;
        if next <= offset || next > file_size {
            break;
        }
        offset = next;
    }

    finish_wave_metadata(fmt, data_size, is_rf64)
}

fn parse_aiff(head: &[u8], is_aifc: bool) -> Option<AudioHeaderMetadata> {
    let mut offset = 12_usize;
    for _ in 0..MAX_HEADER_CHUNKS {
        if offset.checked_add(8)? > head.len() {
            break;
        }
        let id = &head[offset..offset + 4];
        let chunk_size = u32::from_be_bytes(head[offset + 4..offset + 8].try_into().ok()?);
        let body_start = offset + 8;
        let body_end = body_start
            .saturating_add(chunk_size as usize)
            .min(head.len());
        let body = &head[body_start..body_end];

        if id == b"COMM" && body.len() >= 18 {
            let channels = u16::from_be_bytes(body[0..2].try_into().ok()?);
            let frame_count = u64::from(u32::from_be_bytes(body[2..6].try_into().ok()?));
            let bit_depth = u16::from_be_bytes(body[6..8].try_into().ok()?);
            let sample_rate = decode_ieee754_extended_rate(body[8..18].try_into().ok()?)?;
            if sample_rate == 0 || channels == 0 || bit_depth == 0 {
                return None;
            }
            let codec = if is_aifc {
                if body.len() < 22 {
                    return None;
                }
                match &body[18..22] {
                    b"NONE" | b"sowt" | b"twos" | b"raw " | b"in24" | b"in32" => {
                        AudioCodec::AiffPcm
                    }
                    b"fl32" | b"FL32" | b"fl64" | b"FL64" => AudioCodec::AifcFloat,
                    _ => return None,
                }
            } else {
                AudioCodec::AiffPcm
            };
            return Some(AudioHeaderMetadata {
                sample_rate,
                channels,
                bit_depth,
                frame_count,
                codec,
            });
        }

        let padded = u64::from(chunk_size).checked_add(u64::from(chunk_size & 1))?;
        let next = u64::try_from(body_start).ok()?.checked_add(padded)?;
        let Ok(next_usize) = usize::try_from(next) else {
            break;
        };
        if next_usize <= offset || next_usize > head.len() {
            break;
        }
        offset = next_usize;
    }
    None
}

/// Decode a positive 80-bit big-endian IEEE 754 extended float sample rate.
fn decode_ieee754_extended_rate(bytes: [u8; 10]) -> Option<u32> {
    let sign = (bytes[0] & 0x80) != 0;
    let exponent = u16::from_be_bytes([bytes[0] & 0x7f, bytes[1]]);
    let mantissa = u64::from_be_bytes(bytes[2..10].try_into().ok()?);
    if sign || exponent == 0 || exponent == 0x7fff || mantissa == 0 {
        return None;
    }
    let unbiased = i32::from(exponent) - 16_383;
    if !(0..=30).contains(&unbiased) {
        return None;
    }
    let value = (mantissa as f64) * 2.0_f64.powi(unbiased - 63);
    if !value.is_finite() || value <= 0.0 || value > u32::MAX as f64 {
        return None;
    }
    Some(value.round() as u32)
}

fn parse_flac(head: &[u8]) -> Option<AudioHeaderMetadata> {
    // 4-byte "fLaC" + 4-byte metadata block header + 34-byte STREAMINFO
    if head.len() < 42 {
        return None;
    }
    let block_type = head[4] & 0x7f;
    let block_len =
        (usize::from(head[5]) << 16) | (usize::from(head[6]) << 8) | usize::from(head[7]);
    if block_type != 0 || block_len < 34 {
        return None;
    }
    let info = &head[8..42];
    let sample_rate =
        (u32::from(info[10]) << 12) | (u32::from(info[11]) << 4) | (u32::from(info[12]) >> 4);
    let channels = u16::from((info[12] >> 1) & 0x07) + 1;
    let bit_depth = u16::from(((info[12] & 0x01) << 4) | (info[13] >> 4)) + 1;
    let frame_count = (u64::from(info[13] & 0x0f) << 32)
        | u64::from(u32::from_be_bytes(info[14..18].try_into().ok()?));
    if sample_rate == 0 || channels == 0 || bit_depth == 0 {
        return None;
    }
    Some(AudioHeaderMetadata {
        sample_rate,
        channels,
        bit_depth,
        frame_count,
        codec: AudioCodec::Flac,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_bytes(
        sample_rate: u32,
        channels: u16,
        bit_depth: u16,
        frames: u32,
        float: bool,
    ) -> Vec<u8> {
        let block_align = channels * (bit_depth / 8);
        let data_bytes = frames * u32::from(block_align);
        let byte_rate = sample_rate * u32::from(block_align);
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16_u32.to_le_bytes());
        out.extend_from_slice(&(if float { 3_u16 } else { 1_u16 }).to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&bit_depth.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_bytes.to_le_bytes());
        out
    }

    #[test]
    fn parses_wav_rf64_aiff_and_flac_headers_without_extra_io() {
        let wav = wav_bytes(48_000, 2, 24, 96_000, false);
        let meta = parse_audio_header(&wav).unwrap();
        assert_eq!(meta.sample_rate, 48_000);
        assert_eq!(meta.channels, 2);
        assert_eq!(meta.bit_depth, 24);
        assert_eq!(meta.frame_count, 96_000);
        assert_eq!(meta.codec, AudioCodec::WavPcm);
        assert_eq!(meta.duration_ms(), 2_000);

        let wav_f = wav_bytes(96_000, 1, 32, 48_000, true);
        let meta_f = parse_audio_header(&wav_f).unwrap();
        assert_eq!(meta_f.codec, AudioCodec::WavFloat);
        assert_eq!(meta_f.duration_ms(), 500);

        // Minimal RF64 with ds64 overriding data size
        let mut rf64 = Vec::new();
        rf64.extend_from_slice(b"RF64");
        rf64.extend_from_slice(&u32::MAX.to_le_bytes());
        rf64.extend_from_slice(b"WAVE");
        rf64.extend_from_slice(b"ds64");
        rf64.extend_from_slice(&28_u32.to_le_bytes());
        rf64.extend_from_slice(&0_u64.to_le_bytes()); // riff size
        rf64.extend_from_slice(&(48_000_u64 * 6).to_le_bytes()); // data size (48k frames stereo 24-bit)
        rf64.extend_from_slice(&0_u64.to_le_bytes());
        rf64.extend_from_slice(&0_u32.to_le_bytes());
        rf64.extend_from_slice(&wav[12..36]); // fmt chunk
        rf64.extend_from_slice(b"data");
        rf64.extend_from_slice(&u32::MAX.to_le_bytes());
        let meta_rf64 = parse_audio_header(&rf64).unwrap();
        assert_eq!(meta_rf64.codec, AudioCodec::Rf64Pcm);
        assert_eq!(meta_rf64.frame_count, 48_000);
        assert_eq!(meta_rf64.duration_ms(), 1_000);

        // Minimal AIFF with 44100 Hz 80-bit IEEE extended (0x400e AC44 0000 0000 0000)
        let mut aiff = Vec::new();
        aiff.extend_from_slice(b"FORM");
        aiff.extend_from_slice(&30_u32.to_be_bytes());
        aiff.extend_from_slice(b"AIFF");
        aiff.extend_from_slice(b"COMM");
        aiff.extend_from_slice(&18_u32.to_be_bytes());
        aiff.extend_from_slice(&2_u16.to_be_bytes());
        aiff.extend_from_slice(&44_100_u32.to_be_bytes());
        aiff.extend_from_slice(&16_u16.to_be_bytes());
        aiff.extend_from_slice(&[0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]);
        let meta_aiff = parse_audio_header(&aiff).unwrap();
        assert_eq!(meta_aiff.codec, AudioCodec::AiffPcm);
        assert_eq!(meta_aiff.sample_rate, 44_100);
        assert_eq!(meta_aiff.channels, 2);
        assert_eq!(meta_aiff.bit_depth, 16);
        assert_eq!(meta_aiff.frame_count, 44_100);

        // Minimal FLAC STREAMINFO (48000 Hz, 2ch, 24-bit, 144000 frames = 3s)
        let mut flac = vec![0_u8; 42];
        flac[0..4].copy_from_slice(b"fLaC");
        flac[4] = 0x80; // last block, type 0 (STREAMINFO)
        flac[7] = 34;
        let sr = 48_000_u32;
        let ch_m1 = 1_u32;
        let bps_m1 = 23_u32;
        let frames = 144_000_u64;
        flac[18] = (sr >> 12) as u8;
        flac[19] = (sr >> 4) as u8;
        flac[20] = (((sr & 0x0f) << 4) | (ch_m1 << 1) | (bps_m1 >> 4)) as u8;
        flac[21] = (((bps_m1 & 0x0f) << 4) | ((frames >> 32) as u32 & 0x0f)) as u8;
        flac[22..26].copy_from_slice(&(frames as u32).to_be_bytes());
        let meta_flac = parse_audio_header(&flac).unwrap();
        assert_eq!(meta_flac.codec, AudioCodec::Flac);
        assert_eq!(meta_flac.sample_rate, 48_000);
        assert_eq!(meta_flac.channels, 2);
        assert_eq!(meta_flac.bit_depth, 24);
        assert_eq!(meta_flac.frame_count, 144_000);
        assert_eq!(meta_flac.duration_ms(), 3_000);

        // Garbage / truncated bytes safely return None
        assert_eq!(parse_audio_header(b""), None);
        assert_eq!(parse_audio_header(b"not audio bytes"), None);
        assert_eq!(parse_audio_header(&wav[..20]), None);
    }

    #[test]
    fn validates_aifc_fourccs_wave_extensible_guid_tail_and_data_past_64k() {
        // 1. AIFC compression 4CCs: uncompressed PCM/float accepted, compressed codecs rejected.
        let make_aifc = |fourcc: &[u8; 4]| {
            let mut buf = Vec::new();
            buf.extend_from_slice(b"FORM");
            buf.extend_from_slice(&34_u32.to_be_bytes());
            buf.extend_from_slice(b"AIFC");
            buf.extend_from_slice(b"COMM");
            buf.extend_from_slice(&22_u32.to_be_bytes());
            buf.extend_from_slice(&2_u16.to_be_bytes());
            buf.extend_from_slice(&44_100_u32.to_be_bytes());
            buf.extend_from_slice(&24_u16.to_be_bytes());
            buf.extend_from_slice(&[0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]);
            buf.extend_from_slice(fourcc);
            buf
        };
        assert_eq!(
            parse_audio_header(&make_aifc(b"sowt")).unwrap().codec,
            AudioCodec::AiffPcm
        );
        assert_eq!(
            parse_audio_header(&make_aifc(b"NONE")).unwrap().codec,
            AudioCodec::AiffPcm
        );
        assert_eq!(
            parse_audio_header(&make_aifc(b"fl32")).unwrap().codec,
            AudioCodec::AifcFloat
        );
        for compressed in [b"ima4", b"ulaw", b"alaw", b"QDM2"] {
            assert_eq!(
                parse_audio_header(&make_aifc(compressed)),
                None,
                "compressed AIFC {compressed:?} must not be classified as AiffPcm"
            );
        }

        // 2. WAVEFORMATEXTENSIBLE must verify full 16-byte GUID tail, not only first 2 bytes.
        let make_extensible_wav = |sub_tag: u16, tail: [u8; 14]| {
            let mut out = Vec::new();
            out.extend_from_slice(b"RIFF");
            out.extend_from_slice(&100_u32.to_le_bytes());
            out.extend_from_slice(b"WAVE");
            out.extend_from_slice(b"fmt ");
            out.extend_from_slice(&40_u32.to_le_bytes());
            out.extend_from_slice(&0xFFFE_u16.to_le_bytes()); // WAVE_FORMAT_EXTENSIBLE
            out.extend_from_slice(&2_u16.to_le_bytes()); // channels
            out.extend_from_slice(&48_000_u32.to_le_bytes()); // sample_rate
            out.extend_from_slice(&(48_000_u32 * 6).to_le_bytes()); // byte_rate
            out.extend_from_slice(&6_u16.to_le_bytes()); // block_align
            out.extend_from_slice(&24_u16.to_le_bytes()); // bit_depth
            out.extend_from_slice(&22_u16.to_le_bytes()); // cbSize
            out.extend_from_slice(&24_u16.to_le_bytes()); // wValidBitsPerSample
            out.extend_from_slice(&3_u32.to_le_bytes()); // dwChannelMask
            out.extend_from_slice(&sub_tag.to_le_bytes());
            out.extend_from_slice(&tail);
            out.extend_from_slice(b"data");
            out.extend_from_slice(&(48_000_u32 * 6).to_le_bytes());
            out
        };
        let valid_pcm = make_extensible_wav(1, WAVE_EXTENSIBLE_GUID_TAIL);
        let meta_pcm = parse_audio_header(&valid_pcm).unwrap();
        assert_eq!(meta_pcm.codec, AudioCodec::WavPcm);
        assert_eq!(meta_pcm.frame_count, 48_000);

        let valid_float = make_extensible_wav(3, WAVE_EXTENSIBLE_GUID_TAIL);
        assert_eq!(
            parse_audio_header(&valid_float).unwrap().codec,
            AudioCodec::WavFloat
        );

        let mut bad_tail = WAVE_EXTENSIBLE_GUID_TAIL;
        bad_tail[13] ^= 0x01;
        assert_eq!(
            parse_audio_header(&make_extensible_wav(1, bad_tail)),
            None,
            "corrupted WAVEFORMATEXTENSIBLE GUID tail must be rejected"
        );

        // 3. WAV with >64 KiB JUNK chunk before `data` resolves exact frame_count via reader.
        let junk_len = 70_000_u32;
        let data_bytes = 96_000_u32 * 6;
        let mut big_wav = Vec::new();
        big_wav.extend_from_slice(b"RIFF");
        big_wav.extend_from_slice(&(4 + 24 + 8 + junk_len + 8 + data_bytes).to_le_bytes());
        big_wav.extend_from_slice(b"WAVE");
        big_wav.extend_from_slice(&wav_bytes(48_000, 2, 24, 0, false)[12..36]); // fmt chunk
        big_wav.extend_from_slice(b"JUNK");
        big_wav.extend_from_slice(&junk_len.to_le_bytes());
        big_wav.resize(big_wav.len() + junk_len as usize, 0);
        big_wav.extend_from_slice(b"data");
        big_wav.extend_from_slice(&data_bytes.to_le_bytes());
        let file_size = big_wav.len() as u64;
        let mut cursor = std::io::Cursor::new(&big_wav);
        let head = &big_wav[..64 * 1024];
        let meta = parse_audio_header_with_reader(head, &mut cursor, file_size).unwrap();
        assert_eq!(meta.codec, AudioCodec::WavPcm);
        assert_eq!(meta.sample_rate, 48_000);
        assert_eq!(meta.frame_count, 96_000);
        assert_eq!(meta.duration_ms(), 2_000);
    }
}
