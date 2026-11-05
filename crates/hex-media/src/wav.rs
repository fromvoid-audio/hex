//! Shared RIFF/WAVE topology validation and sample conversion.
//!
//! Resident import supports mono/stereo at 8–192 kHz: PCM 8/16/24/32-bit,
//! IEEE float 32/64-bit, extensible PCM/float, 8-bit A-law/mu-law and 4-bit
//! Microsoft/IMA ADPCM. Seekable readers support the fixed-frame encodings;
//! ADPCM remains resident-only. Unknown metadata chunks (including BWF/INFO)
//! are skipped with bounded header work and checked payload/padding extents.
//! RF64/BW64, big-endian RIFX and other encodings are refused explicitly.
//!
//! A legacy producer defect overstates the RIFF container by exactly four bytes.
//! Only that mismatch may use physical EOF as the scan boundary, and only a
//! complete chunk chain terminating there succeeds. Chunk sizes, padding, format
//! and frame validation remain strict; no missing audio bytes are synthesized.
//! Standard RIFF sizes exclude the first eight bytes; see
//! <https://learn.microsoft.com/en-us/windows/win32/xaudio2/resource-interchange-file-format--riff->.
//!
//! Some writers omit the pad byte after an odd-sized final chunk. That is
//! accepted only for the final chunk, only when it ends exactly at physical EOF,
//! and only when the declared RIFF end is that EOF or EOF plus the missing pad.
//!
//! Truncated recordings (a `data` chunk crossing physical EOF while the RIFF
//! size also claims more bytes than exist) and complete `data` chunks ending in a
//! partial frame are never accepted silently: strict scans refuse them with
//! [`ImportError::Truncated`], which reports the readable whole frames. Only an
//! explicit recovery decode keeps those frames, for fixed-frame encodings only.
use crate::{ImportError, WavTruncation};
use hex_engine::Audio;
use std::{
    io::{Cursor, Read, Seek, SeekFrom},
    sync::atomic::AtomicBool,
};

/// Decoded audio with its source sample rate and frame count.
pub(super) type Decoded = (Audio, u32, usize);

const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Format {
    pub(super) encoding: u16,
    pub(super) channels: u16,
    pub(super) sample_rate: u32,
    pub(super) block_align: usize,
    pub(super) bits_per_sample: u16,
    pub(super) samples_per_block: usize,
    pub(super) coefficients: [(i16, i16); 32],
    pub(super) coefficient_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    pub(super) format: Format,
    pub(super) data_offset: u64,
    pub(super) data_bytes: u64,
    pub(super) frames: u64,
}

impl Format {
    pub(super) fn require_fixed_frames(self) -> Result<(), ImportError> {
        if self.samples_per_block != 0 {
            return Err(ImportError::Invalid(
                "adpcm is supported by resident import only",
            ));
        }
        Ok(())
    }

    fn parse(bytes: &[u8]) -> Result<Self, ImportError> {
        if bytes.len() < 16 {
            return Err(ImportError::Invalid(
                "format chunk is shorter than 16 bytes",
            ));
        }
        let mut encoding = read_u16(bytes, 0)?;
        let channels = read_u16(bytes, 2)?;
        let sample_rate = read_u32(bytes, 4)?;
        let byte_rate = read_u32(bytes, 8)?;
        let block_align = usize::from(read_u16(bytes, 12)?);
        let bits_per_sample = read_u16(bytes, 14)?;
        if encoding == 0xfffe {
            if bytes.len() < 40 || read_u16(bytes, 16)? < 22 {
                return Err(ImportError::Invalid("truncated extensible format"));
            }
            let valid_bits = read_u16(bytes, 18)?;
            if valid_bits == 0 || valid_bits > bits_per_sample {
                return Err(ImportError::Invalid("invalid extensible valid bit depth"));
            }
            let mask = read_u32(bytes, 20)?;
            if mask != 0 && !matches!((channels, mask), (1, 1 | 4) | (2, 3)) {
                return Err(ImportError::Invalid("unsupported channel layout"));
            }
            // KSDATAFORMAT_SUBTYPE_PCM / IEEE_FLOAT: validate the entire GUID.
            if bytes[26..40] != [0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71] {
                return Err(ImportError::Invalid("unsupported extensible subtype"));
            }
            encoding = read_u16(bytes, 24)?;
            if !matches!(encoding, 1 | 3) || (encoding == 3 && valid_bits != bits_per_sample) {
                return Err(ImportError::Invalid(
                    "unsupported extensible subtype or depth",
                ));
            }
        }
        if !matches!(encoding, 1 | 2 | 3 | 6 | 7 | 17) {
            return Err(ImportError::Invalid("unsupported wav encoding"));
        }
        if channels != 1 && channels != 2 {
            return Err(ImportError::Invalid("only mono and stereo are supported"));
        }
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(ImportError::Invalid(
                "sample rate is outside 8 kHz..192 kHz",
            ));
        }
        let mut samples_per_block = 0;
        let mut coefficients = [(0, 0); 32];
        let mut coefficient_count = 0;
        if matches!(encoding, 2 | 17) {
            if bits_per_sample != 4 || bytes.len() < 20 || read_u16(bytes, 16)? < 2 {
                return Err(ImportError::Invalid("invalid adpcm format extension"));
            }
            samples_per_block = usize::from(read_u16(bytes, 18)?);
            let channels = usize::from(channels);
            let header_bytes = channels * if encoding == 2 { 7 } else { 4 };
            if block_align < header_bytes {
                return Err(ImportError::Invalid("adpcm block is shorter than header"));
            }
            let payload = block_align - header_bytes;
            let expected_samples = payload * 2 / channels + if encoding == 2 { 2 } else { 1 };
            if samples_per_block != expected_samples
                || (encoding == 17 && payload % (4 * channels) != 0)
            {
                return Err(ImportError::Invalid("inconsistent adpcm block dimensions"));
            }
            if encoding == 2 {
                coefficient_count = usize::from(read_u16(bytes, 20)?);
                if coefficient_count == 0 || coefficient_count > coefficients.len() {
                    return Err(ImportError::Invalid("unsupported adpcm coefficient count"));
                }
                let required = 22 + 4 * coefficient_count;
                if bytes.len() < required || usize::from(read_u16(bytes, 16)?) < required - 18 {
                    return Err(ImportError::Invalid("truncated adpcm coefficients"));
                }
                for (index, pair) in coefficients[..coefficient_count].iter_mut().enumerate() {
                    *pair = (
                        read_u16(bytes, 22 + index * 4)? as i16,
                        read_u16(bytes, 24 + index * 4)? as i16,
                    );
                }
            }
            if byte_rate == 0 {
                return Err(ImportError::Invalid("zero adpcm byte rate"));
            }
        }
        let supported_depth = match encoding {
            1 => matches!(bits_per_sample, 8 | 16 | 24 | 32),
            3 => matches!(bits_per_sample, 32 | 64),
            6 | 7 => bits_per_sample == 8,
            2 | 17 => bits_per_sample == 4,
            _ => false,
        };
        if !supported_depth {
            return Err(ImportError::Invalid(
                "sample encoding or bit depth is unsupported",
            ));
        }
        let bytes_per_sample = usize::from(bits_per_sample / 8);
        let expected_align = usize::from(channels)
            .checked_mul(bytes_per_sample)
            .ok_or(ImportError::Invalid("block alignment overflow"))?;
        let expected_rate = sample_rate
            .checked_mul(expected_align as u32)
            .ok_or(ImportError::Invalid("byte rate overflow"))?;
        if samples_per_block == 0 && (block_align != expected_align || byte_rate != expected_rate) {
            return Err(ImportError::Invalid(
                "inconsistent byte rate or block alignment",
            ));
        }
        Ok(Self {
            encoding,
            channels,
            sample_rate,
            block_align,
            bits_per_sample,
            samples_per_block,
            coefficients,
            coefficient_count,
        })
    }
}

/// Scans all chunk headers while seeking over payloads. `header_limit` bounds
/// work even for a file containing many tiny chunks. Truncated or partial-frame
/// data is refused with [`ImportError::Truncated`].
pub(super) fn scan<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
    header_limit: usize,
) -> Result<Layout, ImportError> {
    match scan_recoverable(reader, file_len, header_limit)? {
        (layout, None) => Ok(layout),
        (_, Some(truncation)) => Err(ImportError::Truncated(truncation)),
    }
}

/// Like [`scan`], but returns a layout restricted to the readable whole frames
/// of truncated or partial-frame data together with the truncation report.
fn scan_recoverable<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
    header_limit: usize,
) -> Result<(Layout, Option<WavTruncation>), ImportError> {
    reader.seek(SeekFrom::Start(0))?;
    let mut container = [0_u8; 12];
    read_exact_invalid(reader, &mut container, "truncated RIFF/WAVE header")?;
    if &container[..4] != b"RIFF" || &container[8..12] != b"WAVE" {
        return Err(ImportError::Invalid("missing RIFF/WAVE header"));
    }
    let declared_riff_end = u64::from(u32::from_le_bytes([
        container[4],
        container[5],
        container[6],
        container[7],
    ]))
    .checked_add(8)
    .ok_or(ImportError::Invalid("RIFF size overflow"))?;
    if declared_riff_end < 12 {
        return Err(ImportError::Invalid("RIFF boundary exceeds input"));
    }
    // Scan no further than physical EOF; any overrun is justified after the scan.
    let riff_end = declared_riff_end.min(file_len);
    let overrun = declared_riff_end - riff_end;

    let mut cursor = 12_u64;
    let mut header_count = 0_usize;
    let mut format_bytes = None;
    let mut data = None;
    let mut fact_frames = None;
    let mut missing_final_pad = false;
    let mut truncated_data = false;
    while cursor < riff_end {
        header_count = header_count
            .checked_add(1)
            .ok_or(ImportError::LimitExceeded("chunk headers"))?;
        if header_count > header_limit {
            return Err(ImportError::LimitExceeded("chunk headers"));
        }
        let header_end = cursor
            .checked_add(8)
            .ok_or(ImportError::Invalid("chunk boundary overflow"))?;
        if header_end > riff_end {
            return Err(ImportError::Invalid("truncated chunk header"));
        }
        reader.seek(SeekFrom::Start(cursor))?;
        let mut header = [0_u8; 8];
        read_exact_invalid(reader, &mut header, "truncated chunk header")?;
        let chunk_size = u64::from(u32::from_le_bytes([
            header[4], header[5], header[6], header[7],
        ]));
        let payload_start = header_end;
        let payload_end = payload_start
            .checked_add(chunk_size)
            .ok_or(ImportError::Invalid("chunk size overflow"))?;
        if payload_end > riff_end {
            // A recording cut short: both the container and its data chunk claim
            // bytes beyond physical EOF. Only data, which is then last, qualifies.
            if overrun > 0 && &header[..4] == b"data" {
                if data.replace((payload_start, chunk_size)).is_some() {
                    return Err(ImportError::Invalid("duplicate data chunk"));
                }
                truncated_data = true;
                break;
            }
            return Err(ImportError::Invalid("chunk exceeds RIFF boundary"));
        }
        if &header[..4] == b"fmt " {
            if format_bytes.is_some() {
                return Err(ImportError::Invalid("duplicate format chunk"));
            }
            if chunk_size < 16 {
                return Err(ImportError::Invalid(
                    "format chunk is shorter than 16 bytes",
                ));
            }
            reader.seek(SeekFrom::Start(payload_start))?;
            // Only the bounded prefix is needed; skip arbitrary trailing metadata.
            let mut fmt = [0_u8; 150];
            let length = chunk_size.min(fmt.len() as u64) as usize;
            read_exact_invalid(reader, &mut fmt[..length], "truncated format chunk")?;
            if chunk_size > 16
                && (chunk_size < 18 || u64::from(read_u16(&fmt[..length], 16)?) + 18 > chunk_size)
            {
                return Err(ImportError::Invalid("truncated format extension"));
            }
            format_bytes = Some(Format::parse(&fmt[..length])?);
        } else if &header[..4] == b"fact" {
            if fact_frames.is_some() || chunk_size < 4 {
                return Err(ImportError::Invalid("invalid or duplicate fact chunk"));
            }
            let mut fact = [0_u8; 4];
            read_exact_invalid(reader, &mut fact, "truncated fact chunk")?;
            fact_frames = Some(u64::from(u32::from_le_bytes(fact)));
        } else if &header[..4] == b"data" && data.replace((payload_start, chunk_size)).is_some() {
            return Err(ImportError::Invalid("duplicate data chunk"));
        }
        let padding = chunk_size & 1;
        cursor = payload_end
            .checked_add(padding)
            .ok_or(ImportError::Invalid("chunk padding overflow"))?;
        if cursor > riff_end {
            // The pad may be absent only after the final chunk at physical EOF,
            // with a RIFF size that either excludes or counts that missing pad.
            if payload_end == file_len && overrun <= 1 {
                missing_final_pad = true;
                break;
            }
            return Err(ImportError::Invalid("missing odd-chunk padding byte"));
        }
    }
    // Legacy producers overstate RIFF by exactly four bytes, and a missing final
    // pad may be counted in the RIFF size. Every other overrun is refused.
    if overrun > 0 && !truncated_data && overrun != 4 && !(overrun == 1 && missing_final_pad) {
        return Err(ImportError::Invalid("RIFF boundary exceeds input"));
    }

    let format = format_bytes.ok_or(ImportError::Invalid("missing format chunk"))?;
    let (data_offset, declared_data_bytes) =
        data.ok_or(ImportError::Invalid("missing data chunk"))?;
    let present_data_bytes = if truncated_data {
        file_len.saturating_sub(data_offset)
    } else {
        declared_data_bytes
    };
    let block_align = format.block_align as u64;
    let partial_frame_bytes = present_data_bytes % block_align;
    let mut truncation = None;
    if truncated_data || partial_frame_bytes != 0 {
        if format.samples_per_block != 0 {
            return Err(ImportError::Invalid(
                "data chunk is empty or not frame-aligned",
            ));
        }
        let readable_frames = present_data_bytes / block_align;
        if readable_frames == 0 {
            return Err(ImportError::Invalid(
                "truncated data chunk has no complete frames",
            ));
        }
        truncation = Some(WavTruncation {
            readable_frames,
            missing_bytes: declared_data_bytes - present_data_bytes,
            partial_frame_bytes,
        });
    }
    let data_bytes = present_data_bytes - partial_frame_bytes;
    if data_bytes == 0 {
        return Err(ImportError::Invalid(
            "data chunk is empty or not frame-aligned",
        ));
    }
    let mut frames = data_bytes / block_align;
    if format.samples_per_block != 0 {
        frames = frames
            .checked_mul(format.samples_per_block as u64)
            .ok_or(ImportError::Invalid("adpcm frame count overflow"))?;
        if let Some(count) = fact_frames {
            // Only the final block may contain encoder padding.
            if count == 0 || count > frames || frames - count >= format.samples_per_block as u64 {
                return Err(ImportError::Invalid("inconsistent adpcm fact frame count"));
            }
            frames = count;
        }
    }
    Ok((
        Layout {
            format,
            data_offset,
            data_bytes,
            frames,
        },
        truncation,
    ))
}

/// Decodes a complete resident WAV. Truncated or partial-frame data is refused.
pub(super) fn decode(
    bytes: &[u8],
    max_frames: usize,
    cancel: &AtomicBool,
) -> Result<(Audio, u32, usize), ImportError> {
    match decode_recoverable(bytes, max_frames, cancel)? {
        (decoded, None) => Ok(decoded),
        (_, Some(truncation)) => Err(ImportError::Truncated(truncation)),
    }
}

/// Decodes the readable whole frames of a resident WAV and reports truncation.
/// Callers must surface the report; recovery is an explicit user choice.
pub(super) fn decode_recoverable(
    bytes: &[u8],
    max_frames: usize,
    cancel: &AtomicBool,
) -> Result<(Decoded, Option<WavTruncation>), ImportError> {
    let mut cursor = Cursor::new(bytes);
    let (layout, truncation) = scan_recoverable(&mut cursor, bytes.len() as u64, usize::MAX)?;
    let frame_count =
        usize::try_from(layout.frames).map_err(|_| ImportError::LimitExceeded("decoded frames"))?;
    if frame_count > max_frames {
        return Err(ImportError::LimitExceeded("decoded frames"));
    }
    let start = usize::try_from(layout.data_offset)
        .map_err(|_| ImportError::Invalid("data offset exceeds address space"))?;
    let end = start
        .checked_add(
            usize::try_from(layout.data_bytes)
                .map_err(|_| ImportError::Invalid("data size exceeds address space"))?,
        )
        .ok_or(ImportError::Invalid("data range overflow"))?;
    let data = bytes
        .get(start..end)
        .ok_or(ImportError::Invalid("data chunk exceeds input"))?;
    crate::cancelled(cancel)?;
    let decoded = if layout.format.samples_per_block != 0 {
        crate::adpcm::decode(data, layout.format, frame_count, cancel)?
    } else {
        decode_samples(data, layout.format, frame_count, cancel)?
    };
    Ok((decoded, truncation))
}

pub(super) fn decode_samples(
    data: &[u8],
    format: Format,
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

pub(super) fn decode_frame(frame: &[u8], format: Format) -> Result<[f32; 2], ImportError> {
    let bytes_per_sample = usize::from(format.bits_per_sample / 8);
    let left = decode_sample(&frame[..bytes_per_sample], format)?;
    let right = if format.channels == 1 {
        left
    } else {
        decode_sample(&frame[bytes_per_sample..bytes_per_sample * 2], format)?
    };
    Ok([left, right])
}

fn decode_sample(sample: &[u8], format: Format) -> Result<f32, ImportError> {
    let value = match (format.encoding, format.bits_per_sample) {
        (1, 8) => (f32::from(sample[0]) - 128.0) / 128.0,
        (1, 16) => i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32_768.0,
        (1, 24) => {
            let raw =
                i32::from(sample[0]) | (i32::from(sample[1]) << 8) | (i32::from(sample[2]) << 16);
            let signed = if raw & 0x0080_0000 != 0 {
                raw | !0x00ff_ffff
            } else {
                raw
            };
            signed as f32 / 8_388_608.0
        }
        (1, 32) => {
            i32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]) as f32
                / 2_147_483_648.0
        }
        (6, 8) => {
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
        (7, 8) => {
            let code = !sample[0];
            let magnitude = ((i32::from(code & 15) << 3) + 132) << ((code >> 4) & 7);
            (if code & 0x80 != 0 {
                132 - magnitude
            } else {
                magnitude - 132
            }) as f32
                / 32_768.0
        }
        (3, 64) => f64::from_le_bytes([
            sample[0], sample[1], sample[2], sample[3], sample[4], sample[5], sample[6], sample[7],
        ]) as f32,
        (3, 32) => f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]),
        _ => {
            return Err(ImportError::Invalid(
                "sample encoding or bit depth is unsupported",
            ));
        }
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

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ImportError> {
    let end = offset
        .checked_add(2)
        .ok_or(ImportError::Invalid("field offset overflow"))?;
    let slice = bytes
        .get(offset..end)
        .ok_or(ImportError::Invalid("truncated field"))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ImportError> {
    let end = offset
        .checked_add(4)
        .ok_or(ImportError::Invalid("field offset overflow"))?;
    let slice = bytes
        .get(offset..end)
        .ok_or(ImportError::Invalid("truncated field"))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}
