//! Bounded AIFF and AIFF-C (`FORM`/`AIFF`, `FORM`/`AIFC`) topology validation and decoding.
//!
//! Supports mono and stereo at 8–192 kHz:
//! - AIFF uncompressed / AIFC `NONE` / `twos`: big-endian signed PCM 8/16/24/32-bit
//! - AIFC `sowt`: little-endian signed PCM 8/16/24/32-bit
//! - AIFC `fl32` / `FL32`: big-endian IEEE 754 32-bit float
//! - AIFC `fl64` / `FL64`: big-endian IEEE 754 64-bit float
//! - AIFC `alaw` / `ALAW` and `ulaw` / `ULAW`: 8-bit ITU-T G.711 A-law / mu-law
//!
//! Unknown metadata chunks are skipped under `header_limit`. Truncated `SSND`
//! data reports [`ImportError::Truncated`] unless recovered explicitly.
use crate::{ImportError, WavTruncation, wav};
use hex_engine::Audio;
use std::{
    io::{Cursor, Read, Seek, SeekFrom},
    sync::atomic::AtomicBool,
};

const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AiffEncoding {
    PcmBe,
    PcmLe,
    FloatBe,
    Alaw,
    Ulaw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AiffFormat {
    pub(super) encoding: AiffEncoding,
    pub(super) channels: u16,
    pub(super) sample_rate: u32,
    pub(super) block_align: usize,
    pub(super) bits_per_sample: u16,
    pub(super) is_aifc: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AiffLayout {
    pub(super) format: AiffFormat,
    pub(super) data_offset: u64,
    pub(super) data_bytes: u64,
    pub(super) frames: u64,
}

impl AiffFormat {
    /// Numeric WAV-equivalent encoding code for caller diagnostics.
    pub(super) const fn compat_encoding(self) -> u16 {
        match self.encoding {
            AiffEncoding::PcmBe | AiffEncoding::PcmLe => 1,
            AiffEncoding::FloatBe => 3,
            AiffEncoding::Alaw => 6,
            AiffEncoding::Ulaw => 7,
        }
    }
}

/// Scans AIFF/AIFC chunk headers while seeking over payloads.
pub(super) fn scan<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
    header_limit: usize,
) -> Result<AiffLayout, ImportError> {
    match scan_recoverable(reader, file_len, header_limit)? {
        (layout, None) => Ok(layout),
        (_, Some(truncation)) => Err(ImportError::Truncated(truncation)),
    }
}

fn scan_recoverable<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
    header_limit: usize,
) -> Result<(AiffLayout, Option<WavTruncation>), ImportError> {
    reader.seek(SeekFrom::Start(0))?;
    let mut container = [0_u8; 12];
    read_exact_invalid(reader, &mut container, "truncated FORM/AIFF header")?;
    if &container[..4] != b"FORM" {
        return Err(ImportError::Invalid("missing FORM/AIFF header"));
    }
    let is_aifc = match &container[8..12] {
        b"AIFF" => false,
        b"AIFC" => true,
        _ => return Err(ImportError::Invalid("missing AIFF/AIFC form type")),
    };
    let declared_form_end = u64::from(u32::from_be_bytes([
        container[4],
        container[5],
        container[6],
        container[7],
    ]))
    .checked_add(8)
    .ok_or(ImportError::Invalid("FORM size overflow"))?;
    if declared_form_end < 12 {
        return Err(ImportError::Invalid("FORM boundary exceeds input"));
    }
    let form_end = declared_form_end.min(file_len);
    let overrun = declared_form_end - form_end;

    let mut cursor = 12_u64;
    let mut header_count = 0_usize;
    let mut comm: Option<(AiffFormat, u64)> = None;
    let mut ssnd: Option<(u64, u64)> = None; // (audio_start, declared_audio_bytes)
    let mut missing_final_pad = false;
    let mut truncated_data = false;

    while cursor < form_end {
        header_count = header_count
            .checked_add(1)
            .ok_or(ImportError::LimitExceeded("chunk headers"))?;
        if header_count > header_limit {
            return Err(ImportError::LimitExceeded("chunk headers"));
        }
        let header_end = cursor
            .checked_add(8)
            .ok_or(ImportError::Invalid("chunk boundary overflow"))?;
        if header_end > form_end {
            return Err(ImportError::Invalid("truncated chunk header"));
        }
        reader.seek(SeekFrom::Start(cursor))?;
        let mut header = [0_u8; 8];
        read_exact_invalid(reader, &mut header, "truncated chunk header")?;
        let chunk_size = u64::from(u32::from_be_bytes([
            header[4], header[5], header[6], header[7],
        ]));
        let payload_start = header_end;
        let payload_end = payload_start
            .checked_add(chunk_size)
            .ok_or(ImportError::Invalid("chunk size overflow"))?;

        if payload_end > form_end {
            if overrun > 0 && &header[..4] == b"SSND" {
                if chunk_size < 8 || payload_start.checked_add(8).is_none_or(|e| e > file_len) {
                    return Err(ImportError::Invalid("truncated SSND header"));
                }
                let mut ssnd_hdr = [0_u8; 8];
                read_exact_invalid(reader, &mut ssnd_hdr, "truncated SSND header")?;
                let offset = u64::from(u32::from_be_bytes([
                    ssnd_hdr[0],
                    ssnd_hdr[1],
                    ssnd_hdr[2],
                    ssnd_hdr[3],
                ]));
                let audio_start = payload_start
                    .checked_add(8)
                    .and_then(|v| v.checked_add(offset))
                    .ok_or(ImportError::Invalid("SSND offset overflow"))?;
                let declared_audio = chunk_size
                    .checked_sub(8)
                    .and_then(|v| v.checked_sub(offset))
                    .ok_or(ImportError::Invalid("SSND offset exceeds chunk"))?;
                if audio_start > file_len {
                    return Err(ImportError::Invalid("truncated SSND data"));
                }
                if ssnd.replace((audio_start, declared_audio)).is_some() {
                    return Err(ImportError::Invalid("duplicate SSND chunk"));
                }
                truncated_data = true;
                break;
            }
            return Err(ImportError::Invalid("chunk exceeds FORM boundary"));
        }

        if &header[..4] == b"COMM" {
            if comm.is_some() {
                return Err(ImportError::Invalid("duplicate COMM chunk"));
            }
            let min_comm = if is_aifc { 22 } else { 18 };
            if chunk_size < min_comm {
                return Err(ImportError::Invalid("COMM chunk is too short"));
            }
            let mut buf = [0_u8; 22];
            let read_len = usize::try_from(chunk_size.min(22)).unwrap_or(22);
            read_exact_invalid(reader, &mut buf[..read_len], "truncated COMM chunk")?;
            comm = Some(parse_comm(&buf[..read_len], is_aifc)?);
        } else if &header[..4] == b"SSND" {
            if chunk_size < 8 {
                return Err(ImportError::Invalid("SSND chunk is shorter than 8 bytes"));
            }
            let mut ssnd_hdr = [0_u8; 8];
            read_exact_invalid(reader, &mut ssnd_hdr, "truncated SSND header")?;
            let offset = u64::from(u32::from_be_bytes([
                ssnd_hdr[0],
                ssnd_hdr[1],
                ssnd_hdr[2],
                ssnd_hdr[3],
            ]));
            let audio_start = payload_start
                .checked_add(8)
                .and_then(|v| v.checked_add(offset))
                .ok_or(ImportError::Invalid("SSND offset overflow"))?;
            let audio_bytes = chunk_size
                .checked_sub(8)
                .and_then(|v| v.checked_sub(offset))
                .ok_or(ImportError::Invalid("SSND offset exceeds chunk"))?;
            if ssnd.replace((audio_start, audio_bytes)).is_some() {
                return Err(ImportError::Invalid("duplicate SSND chunk"));
            }
        }

        let padding = chunk_size & 1;
        cursor = payload_end
            .checked_add(padding)
            .ok_or(ImportError::Invalid("chunk padding overflow"))?;
        if cursor > form_end {
            if payload_end == file_len && overrun <= 1 {
                missing_final_pad = true;
                break;
            }
            return Err(ImportError::Invalid("missing odd-chunk padding byte"));
        }
    }

    if overrun > 0 && !truncated_data && !(overrun == 1 && missing_final_pad) {
        return Err(ImportError::Invalid("FORM boundary exceeds input"));
    }

    let (format, comm_frames) = comm.ok_or(ImportError::Invalid("missing COMM chunk"))?;
    if comm_frames == 0 {
        return Err(ImportError::Invalid("AIFF has zero sample frames"));
    }
    let (data_offset, declared_ssnd_bytes) =
        ssnd.ok_or(ImportError::Invalid("missing SSND chunk"))?;
    let block_align = format.block_align as u64;
    let expected_audio_bytes = comm_frames
        .checked_mul(block_align)
        .ok_or(ImportError::Invalid("AIFF audio byte count overflow"))?;
    let present_bytes = if truncated_data {
        file_len
            .saturating_sub(data_offset)
            .min(expected_audio_bytes)
    } else {
        if declared_ssnd_bytes < expected_audio_bytes {
            // SSND shorter than COMM declares -> treat as truncation if frames remain
            declared_ssnd_bytes
        } else {
            expected_audio_bytes
        }
    };

    let partial_frame_bytes = present_bytes % block_align;
    let readable_bytes = present_bytes - partial_frame_bytes;
    let readable_frames = readable_bytes / block_align;

    let mut truncation = None;
    if truncated_data || present_bytes < expected_audio_bytes || partial_frame_bytes != 0 {
        if readable_frames == 0 {
            return Err(ImportError::Invalid(
                "truncated AIFF data has no complete frames",
            ));
        }
        truncation = Some(WavTruncation {
            readable_frames,
            missing_bytes: expected_audio_bytes.saturating_sub(present_bytes),
            partial_frame_bytes,
        });
    }
    if readable_bytes == 0 {
        return Err(ImportError::Invalid("AIFF audio data is empty"));
    }

    Ok((
        AiffLayout {
            format,
            data_offset,
            data_bytes: readable_bytes,
            frames: readable_frames,
        },
        truncation,
    ))
}

fn parse_comm(bytes: &[u8], is_aifc: bool) -> Result<(AiffFormat, u64), ImportError> {
    let channels = u16::from_be_bytes([bytes[0], bytes[1]]);
    let num_frames = u64::from(u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]));
    let sample_size = u16::from_be_bytes([bytes[6], bytes[7]]);
    let sample_rate = parse_ieee_extended_rate(&bytes[8..18])?;

    if channels != 1 && channels != 2 {
        return Err(ImportError::Invalid("only mono and stereo are supported"));
    }
    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
        return Err(ImportError::Invalid(
            "sample rate is outside 8 kHz..192 kHz",
        ));
    }

    let (encoding, bits_per_sample) = if !is_aifc {
        if !matches!(sample_size, 8 | 16 | 24 | 32) {
            return Err(ImportError::Invalid("unsupported AIFF bit depth"));
        }
        (AiffEncoding::PcmBe, sample_size)
    } else {
        let comp: [u8; 4] = [bytes[18], bytes[19], bytes[20], bytes[21]];
        match &comp {
            b"NONE" | b"twos" => {
                if !matches!(sample_size, 8 | 16 | 24 | 32) {
                    return Err(ImportError::Invalid("unsupported AIFC PCM bit depth"));
                }
                (AiffEncoding::PcmBe, sample_size)
            }
            b"sowt" => {
                if !matches!(sample_size, 8 | 16 | 24 | 32) {
                    return Err(ImportError::Invalid("unsupported AIFC sowt bit depth"));
                }
                (AiffEncoding::PcmLe, sample_size)
            }
            b"fl32" | b"FL32" => (AiffEncoding::FloatBe, 32),
            b"fl64" | b"FL64" => (AiffEncoding::FloatBe, 64),
            b"alaw" | b"ALAW" => (AiffEncoding::Alaw, 8),
            b"ulaw" | b"ULAW" => (AiffEncoding::Ulaw, 8),
            _ => {
                return Err(ImportError::Invalid("unsupported AIFC compression type"));
            }
        }
    };

    let bytes_per_sample = usize::from(bits_per_sample / 8);
    let block_align = usize::from(channels)
        .checked_mul(bytes_per_sample)
        .ok_or(ImportError::Invalid("AIFF block alignment overflow"))?;

    Ok((
        AiffFormat {
            encoding,
            channels,
            sample_rate,
            block_align,
            bits_per_sample,
            is_aifc,
        },
        num_frames,
    ))
}

/// Exact integer sample rate parser for 80-bit big-endian IEEE 754 extended float.
pub(super) fn parse_ieee_extended_rate(bytes: &[u8]) -> Result<u32, ImportError> {
    if bytes.len() < 10 {
        return Err(ImportError::Invalid("truncated AIFF sample rate"));
    }
    let sign_exp = u16::from_be_bytes([bytes[0], bytes[1]]);
    let negative = (sign_exp & 0x8000) != 0;
    let exponent = i32::from(sign_exp & 0x7FFF);
    let mantissa = u64::from_be_bytes([
        bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9],
    ]);
    if negative || exponent == 0 || exponent == 0x7FFF || mantissa == 0 {
        return Err(ImportError::Invalid("invalid AIFF sample rate"));
    }
    // Value is mantissa * 2^(exponent - 16383 - 63).
    let unbiased = exponent - 16383;
    if !(0..=20).contains(&unbiased) {
        return Err(ImportError::Invalid(
            "sample rate is outside 8 kHz..192 kHz",
        ));
    }
    let shift = 63 - unbiased as u32;
    let fractional_mask = if shift == 64 {
        u64::MAX
    } else {
        (1_u64 << shift) - 1
    };
    if (mantissa & fractional_mask) != 0 {
        return Err(ImportError::Invalid(
            "fractional AIFF sample rates are unsupported",
        ));
    }
    let rate = mantissa >> shift;
    u32::try_from(rate).map_err(|_| ImportError::Invalid("AIFF sample rate overflow"))
}

pub(super) fn decode(
    bytes: &[u8],
    max_frames: usize,
    cancel: &AtomicBool,
) -> Result<(Audio, u32, usize), ImportError> {
    let mut cursor = Cursor::new(bytes);
    let (layout, truncation) = scan_recoverable(
        &mut cursor,
        bytes.len() as u64,
        crate::MAX_RESIDENT_HEADER_COUNT,
    )?;
    let frame_count =
        usize::try_from(layout.frames).map_err(|_| ImportError::LimitExceeded("decoded frames"))?;
    if frame_count > max_frames {
        return Err(ImportError::LimitExceeded("decoded frames"));
    }
    let data = slice_data(bytes, layout)?;
    crate::cancelled(cancel)?;
    if let Some(truncation) = truncation {
        validate_samples(data, layout.format, cancel)?;
        return Err(ImportError::Truncated(truncation));
    }
    decode_samples(data, layout.format, frame_count, cancel)
}

pub(super) fn decode_recoverable(
    bytes: &[u8],
    max_frames: usize,
    cancel: &AtomicBool,
) -> Result<(wav::Decoded, Option<WavTruncation>), ImportError> {
    let mut cursor = Cursor::new(bytes);
    let (layout, truncation) = scan_recoverable(
        &mut cursor,
        bytes.len() as u64,
        crate::MAX_RESIDENT_HEADER_COUNT,
    )?;
    let frame_count =
        usize::try_from(layout.frames).map_err(|_| ImportError::LimitExceeded("decoded frames"))?;
    if frame_count > max_frames {
        return Err(ImportError::LimitExceeded("decoded frames"));
    }
    let data = slice_data(bytes, layout)?;
    crate::cancelled(cancel)?;
    let decoded = decode_samples(data, layout.format, frame_count, cancel)?;
    Ok((decoded, truncation))
}

fn slice_data(bytes: &[u8], layout: AiffLayout) -> Result<&[u8], ImportError> {
    let start = usize::try_from(layout.data_offset)
        .map_err(|_| ImportError::Invalid("data offset exceeds address space"))?;
    let end = start
        .checked_add(
            usize::try_from(layout.data_bytes)
                .map_err(|_| ImportError::Invalid("data size exceeds address space"))?,
        )
        .ok_or(ImportError::Invalid("data range overflow"))?;
    bytes
        .get(start..end)
        .ok_or(ImportError::Invalid("SSND data exceeds input"))
}

fn validate_samples(
    data: &[u8],
    format: AiffFormat,
    cancel: &AtomicBool,
) -> Result<(), ImportError> {
    let chunk_bytes = format
        .block_align
        .saturating_mul(crate::CANCEL_CHECK_FRAMES);
    let needs_check = matches!(format.encoding, AiffEncoding::FloatBe);
    for chunk in data.chunks(chunk_bytes.max(1)) {
        crate::cancelled(cancel)?;
        if needs_check {
            for frame in chunk.chunks_exact(format.block_align) {
                let decoded = decode_frame(frame, format)?;
                if !decoded[0].is_finite() || !decoded[1].is_finite() {
                    return Err(ImportError::Invalid(
                        "decoded samples are not valid finite audio",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn decode_samples(
    data: &[u8],
    format: AiffFormat,
    frame_count: usize,
    cancel: &AtomicBool,
) -> Result<(Audio, u32, usize), ImportError> {
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(frame_count)
        .map_err(|_| ImportError::AllocationFailed)?;
    let chunk_bytes = format
        .block_align
        .saturating_mul(crate::CANCEL_CHECK_FRAMES);
    for chunk in data.chunks(chunk_bytes.max(1)) {
        crate::cancelled(cancel)?;
        for frame in chunk.chunks_exact(format.block_align) {
            frames.push(decode_frame(frame, format)?);
        }
    }
    let audio = Audio::new(format.sample_rate, frames)
        .map_err(|_| ImportError::Invalid("decoded samples are not valid finite audio"))?;
    Ok((audio, format.sample_rate, frame_count))
}

pub(super) fn decode_frame(frame: &[u8], format: AiffFormat) -> Result<[f32; 2], ImportError> {
    let bytes_per_sample = usize::from(format.bits_per_sample / 8);
    let left = decode_sample(&frame[..bytes_per_sample], format)?;
    let right = if format.channels == 1 {
        left
    } else {
        decode_sample(&frame[bytes_per_sample..bytes_per_sample * 2], format)?
    };
    Ok([left, right])
}

fn decode_sample(sample: &[u8], format: AiffFormat) -> Result<f32, ImportError> {
    let value = match (format.encoding, format.bits_per_sample) {
        (AiffEncoding::PcmBe | AiffEncoding::PcmLe, 8) => (sample[0] as i8) as f32 / 128.0,
        (AiffEncoding::PcmBe, 16) => i16::from_be_bytes([sample[0], sample[1]]) as f32 / 32_768.0,
        (AiffEncoding::PcmLe, 16) => i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32_768.0,
        (AiffEncoding::PcmBe, 24) => {
            let raw =
                (i32::from(sample[0]) << 16) | (i32::from(sample[1]) << 8) | i32::from(sample[2]);
            let signed = if raw & 0x0080_0000 != 0 {
                raw | !0x00ff_ffff
            } else {
                raw
            };
            signed as f32 / 8_388_608.0
        }
        (AiffEncoding::PcmLe, 24) => {
            let raw =
                i32::from(sample[0]) | (i32::from(sample[1]) << 8) | (i32::from(sample[2]) << 16);
            let signed = if raw & 0x0080_0000 != 0 {
                raw | !0x00ff_ffff
            } else {
                raw
            };
            signed as f32 / 8_388_608.0
        }
        (AiffEncoding::PcmBe, 32) => {
            i32::from_be_bytes([sample[0], sample[1], sample[2], sample[3]]) as f32
                / 2_147_483_648.0
        }
        (AiffEncoding::PcmLe, 32) => {
            i32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]) as f32
                / 2_147_483_648.0
        }
        (AiffEncoding::FloatBe, 32) => {
            f32::from_be_bytes([sample[0], sample[1], sample[2], sample[3]])
        }
        (AiffEncoding::FloatBe, 64) => f64::from_be_bytes([
            sample[0], sample[1], sample[2], sample[3], sample[4], sample[5], sample[6], sample[7],
        ]) as f32,
        (AiffEncoding::Alaw, 8) => {
            let code = sample[0] ^ 0x55;
            let exponent = (code >> 4) & 7;
            let mut magnitude = i32::from(code & 15) << 4;
            magnitude += if exponent == 0 { 8 } else { 0x108 };
            if exponent > 1 {
                magnitude <<= exponent - 1;
            }
            (if code & 0x80 != 0 {
                magnitude
            } else {
                -magnitude
            }) as f32
                / 32_768.0
        }
        (AiffEncoding::Ulaw, 8) => {
            let code = !sample[0];
            let magnitude = ((i32::from(code & 15) << 3) + 132) << ((code >> 4) & 7);
            (if code & 0x80 != 0 {
                132 - magnitude
            } else {
                magnitude - 132
            }) as f32
                / 32_768.0
        }
        _ => return Err(ImportError::Invalid("unsupported AIFF sample encoding")),
    };
    if !value.is_finite() {
        return Err(ImportError::Invalid("float samples must be finite"));
    }
    Ok(value)
}

fn read_exact_invalid<R: Read>(
    reader: &mut R,
    bytes: &mut [u8],
    reason: &'static str,
) -> Result<(), ImportError> {
    match reader.read_exact(bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            Err(ImportError::Invalid(reason))
        }
        Err(error) => Err(ImportError::Io(error)),
    }
}

#[cfg(test)]
pub(crate) fn encode_ieee_extended_rate(rate: u32) -> [u8; 10] {
    if rate == 0 {
        return [0; 10];
    }
    let lz = rate.leading_zeros();
    let exp = (16383 + (31 - lz)) as u16;
    let mantissa = u64::from(rate) << (32 + lz);
    let mut out = [0_u8; 10];
    out[0..2].copy_from_slice(&exp.to_be_bytes());
    out[2..10].copy_from_slice(&mantissa.to_be_bytes());
    out
}
