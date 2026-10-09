//! Shared RIFF/WAVE topology validation and sample conversion.
use crate::ImportError;
use hex_engine::Audio;
use std::io::{Cursor, Read, Seek, SeekFrom};

const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Format {
    pub(super) encoding: u16,
    pub(super) channels: u16,
    pub(super) sample_rate: u32,
    pub(super) block_align: usize,
    pub(super) bits_per_sample: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    pub(super) format: Format,
    pub(super) data_offset: u64,
    pub(super) data_bytes: u64,
    pub(super) frames: u64,
}

impl Format {
    fn parse(bytes: &[u8]) -> Result<Self, ImportError> {
        if bytes.len() < 16 {
            return Err(ImportError::Invalid(
                "format chunk is shorter than 16 bytes",
            ));
        }
        let encoding = read_u16(bytes, 0)?;
        let channels = read_u16(bytes, 2)?;
        let sample_rate = read_u32(bytes, 4)?;
        let byte_rate = read_u32(bytes, 8)?;
        let block_align = usize::from(read_u16(bytes, 12)?);
        let bits_per_sample = read_u16(bytes, 14)?;
        if encoding != 1 && encoding != 3 {
            return Err(ImportError::Invalid("encoding is not PCM or IEEE float"));
        }
        if channels != 1 && channels != 2 {
            return Err(ImportError::Invalid("only mono and stereo are supported"));
        }
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(ImportError::Invalid(
                "sample rate is outside 8 kHz..192 kHz",
            ));
        }
        let supported_depth = match encoding {
            1 => matches!(bits_per_sample, 16 | 24 | 32),
            3 => bits_per_sample == 32,
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
        if block_align != expected_align || byte_rate != expected_rate {
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
        })
    }
}

/// Scans all chunk headers while seeking over payloads. `header_limit` bounds
/// work even for a file containing many tiny chunks.
pub(super) fn scan<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
    header_limit: usize,
) -> Result<Layout, ImportError> {
    reader.seek(SeekFrom::Start(0))?;
    let mut container = [0_u8; 12];
    read_exact_invalid(reader, &mut container, "truncated RIFF/WAVE header")?;
    if &container[..4] != b"RIFF" || &container[8..12] != b"WAVE" {
        return Err(ImportError::Invalid("missing RIFF/WAVE header"));
    }
    let riff_end = u64::from(u32::from_le_bytes([
        container[4],
        container[5],
        container[6],
        container[7],
    ]))
    .checked_add(8)
    .ok_or(ImportError::Invalid("RIFF size overflow"))?;
    if riff_end < 12 || riff_end > file_len {
        return Err(ImportError::Invalid("RIFF boundary exceeds input"));
    }

    let mut cursor = 12_u64;
    let mut header_count = 0_usize;
    let mut format_bytes = None;
    let mut data = None;
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
            let mut fmt = [0_u8; 16];
            read_exact_invalid(reader, &mut fmt, "truncated format chunk")?;
            format_bytes = Some(fmt);
        } else if &header[..4] == b"data" && data.replace((payload_start, chunk_size)).is_some() {
            return Err(ImportError::Invalid("duplicate data chunk"));
        }
        let padding = chunk_size & 1;
        cursor = payload_end
            .checked_add(padding)
            .ok_or(ImportError::Invalid("chunk padding overflow"))?;
        if cursor > riff_end {
            return Err(ImportError::Invalid("missing odd-chunk padding byte"));
        }
    }

    let format_bytes = format_bytes.ok_or(ImportError::Invalid("missing format chunk"))?;
    let format = Format::parse(&format_bytes)?;
    let (data_offset, data_bytes) = data.ok_or(ImportError::Invalid("missing data chunk"))?;
    if data_bytes == 0 || data_bytes % format.block_align as u64 != 0 {
        return Err(ImportError::Invalid(
            "data chunk is empty or not frame-aligned",
        ));
    }
    let frames = data_bytes / format.block_align as u64;
    Ok(Layout {
        format,
        data_offset,
        data_bytes,
        frames,
    })
}

pub(super) fn decode(bytes: &[u8], max_frames: usize) -> Result<(Audio, u32, usize), ImportError> {
    let mut cursor = Cursor::new(bytes);
    let layout = scan(&mut cursor, bytes.len() as u64, usize::MAX)?;
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
    decode_samples(data, layout.format, frame_count)
}

pub(super) fn decode_samples(
    data: &[u8],
    format: Format,
    frame_count: usize,
) -> Result<(Audio, u32, usize), ImportError> {
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(frame_count)
        .map_err(|_| ImportError::AllocationFailed)?;
    for frame in data.chunks_exact(format.block_align) {
        frames.push(decode_frame(frame, format)?);
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
