//! Pure-Rust bounded RFC 9639 FLAC decoder and verified seekable frame reader.
//!
//! Supports mono and stereo at 8–192 kHz, 4–32 bits per sample, all four
//! subframe types (`CONSTANT`, `VERBATIM`, `FIXED` orders 0..=4, `LPC` orders
//! 1..=32), wasted bits, Rice / Rice2 entropy coding, independent and stereo
//! decorrelated (`left-side`, `right-side`, `mid-side`) channel assignments,
//! `SEEKTABLE` indexing, and CRC-8/CRC-16 frame integrity checks.
use crate::ImportError;
use hex_engine::{Audio, Frame};
use std::{
    io::{self, Cursor, Read, Seek, SeekFrom},
    sync::atomic::AtomicBool,
};

const MIN_SAMPLE_RATE: u32 = 8_000;
const MAX_SAMPLE_RATE: u32 = 192_000;
const MAX_BLOCK_SIZE: usize = 65_535;
const MAX_SEEKPOINTS: usize = 4_096;
const MAX_SUBFRAME_ORDER: usize = 32;
/// Maximum raw byte read for a single FLAC frame (bounds malformed frames).
const MAX_FLAC_FRAME_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FlacSeekPoint {
    pub(super) sample_number: u64,
    pub(super) stream_offset: u64,
    pub(super) frame_samples: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FlacLayout {
    pub(super) min_block_size: u16,
    pub(super) max_block_size: u16,
    pub(super) sample_rate: u32,
    pub(super) channels: u16,
    pub(super) bits_per_sample: u16,
    pub(super) total_samples: u64,
    pub(super) first_frame_offset: u64,
    pub(super) seektable: Vec<FlacSeekPoint>,
}

/// Cached position of the most recently decoded FLAC frame so sequential
/// streaming reads continue in O(1) without rescanning from frame 0.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FlacStreamCursor {
    pub(super) next_sample: u64,
    pub(super) next_offset: u64,
    pub(super) valid: bool,
}

/// Scans `fLaC` header, mandatory `STREAMINFO`, and optional `SEEKTABLE`.
pub(super) fn scan<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
    header_limit: usize,
) -> Result<FlacLayout, ImportError> {
    reader.seek(SeekFrom::Start(0))?;
    let mut magic = [0_u8; 4];
    read_exact_invalid(reader, &mut magic, "truncated FLAC magic")?;
    if &magic != b"fLaC" {
        return Err(ImportError::Invalid("missing fLaC header"));
    }

    let mut cursor = 4_u64;
    let mut block_count = 0_usize;
    let mut streaminfo: Option<(u16, u16, u32, u16, u16, u64)> = None;
    let mut seektable = Vec::new();

    loop {
        block_count = block_count
            .checked_add(1)
            .ok_or(ImportError::LimitExceeded("chunk headers"))?;
        if block_count > header_limit {
            return Err(ImportError::LimitExceeded("chunk headers"));
        }
        let header_end = cursor
            .checked_add(4)
            .ok_or(ImportError::Invalid("FLAC metadata offset overflow"))?;
        if header_end > file_len {
            return Err(ImportError::Invalid("truncated FLAC metadata block header"));
        }
        reader.seek(SeekFrom::Start(cursor))?;
        let mut hdr = [0_u8; 4];
        read_exact_invalid(reader, &mut hdr, "truncated FLAC metadata block header")?;
        let is_last = (hdr[0] & 0x80) != 0;
        let block_type = hdr[0] & 0x7F;
        if block_type == 127 {
            return Err(ImportError::Invalid("invalid FLAC metadata block type 127"));
        }
        let length = (u64::from(hdr[1]) << 16) | (u64::from(hdr[2]) << 8) | u64::from(hdr[3]);
        let payload_end = header_end
            .checked_add(length)
            .ok_or(ImportError::Invalid("FLAC metadata length overflow"))?;
        if payload_end > file_len {
            return Err(ImportError::Invalid("FLAC metadata block exceeds file"));
        }

        if block_count == 1 {
            if block_type != 0 || length != 34 {
                return Err(ImportError::Invalid("missing or invalid FLAC STREAMINFO"));
            }
            let mut si = [0_u8; 34];
            read_exact_invalid(reader, &mut si, "truncated FLAC STREAMINFO")?;
            let min_bs = u16::from_be_bytes([si[0], si[1]]);
            let max_bs = u16::from_be_bytes([si[2], si[3]]);
            if min_bs < 16 || max_bs < min_bs {
                return Err(ImportError::Invalid("invalid FLAC block size bounds"));
            }
            let sample_rate =
                (u32::from(si[10]) << 12) | (u32::from(si[11]) << 4) | (u32::from(si[12]) >> 4);
            let channels = u16::from(((si[12] >> 1) & 0x07) + 1);
            let bits_per_sample = u16::from((((si[12] & 0x01) << 4) | (si[13] >> 4)) + 1);
            let total_samples = (u64::from(si[13] & 0x0F) << 32)
                | (u64::from(si[14]) << 24)
                | (u64::from(si[15]) << 16)
                | (u64::from(si[16]) << 8)
                | u64::from(si[17]);

            if channels != 1 && channels != 2 {
                return Err(ImportError::Invalid("only mono and stereo are supported"));
            }
            if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
                return Err(ImportError::Invalid(
                    "sample rate is outside 8 kHz..192 kHz",
                ));
            }
            if !(4..=32).contains(&bits_per_sample) {
                return Err(ImportError::Invalid("unsupported FLAC bit depth"));
            }
            if total_samples == 0 {
                return Err(ImportError::Invalid(
                    "FLAC stream with unknown total sample count is unsupported",
                ));
            }
            streaminfo = Some((
                min_bs,
                max_bs,
                sample_rate,
                channels,
                bits_per_sample,
                total_samples,
            ));
        } else if block_type == 0 {
            return Err(ImportError::Invalid("duplicate FLAC STREAMINFO block"));
        } else if block_type == 3 {
            if !length.is_multiple_of(18) {
                return Err(ImportError::Invalid("invalid FLAC SEEKTABLE length"));
            }
            let count = usize::try_from(length / 18)
                .map_err(|_| ImportError::LimitExceeded("FLAC seektable"))?;
            let read_count = count.min(MAX_SEEKPOINTS);
            seektable
                .try_reserve(read_count)
                .map_err(|_| ImportError::AllocationFailed)?;
            let mut prev_sample: Option<u64> = None;
            let mut entry = [0_u8; 18];
            for _ in 0..read_count {
                read_exact_invalid(reader, &mut entry, "truncated FLAC SEEKTABLE")?;
                let sample_number = u64::from_be_bytes([
                    entry[0], entry[1], entry[2], entry[3], entry[4], entry[5], entry[6], entry[7],
                ]);
                if sample_number == u64::MAX {
                    // Placeholder seekpoint; all subsequent seekpoints must also be placeholders.
                    continue;
                }
                let stream_offset = u64::from_be_bytes([
                    entry[8], entry[9], entry[10], entry[11], entry[12], entry[13], entry[14],
                    entry[15],
                ]);
                let frame_samples = u16::from_be_bytes([entry[16], entry[17]]);
                if prev_sample.is_some_and(|prev| sample_number <= prev) {
                    return Err(ImportError::Invalid("unsorted FLAC SEEKTABLE"));
                }
                prev_sample = Some(sample_number);
                seektable.push(FlacSeekPoint {
                    sample_number,
                    stream_offset,
                    frame_samples,
                });
            }
        }

        cursor = payload_end;
        if is_last {
            break;
        }
    }

    let (min_block_size, max_block_size, sample_rate, channels, bits_per_sample, total_samples) =
        streaminfo.ok_or(ImportError::Invalid("missing FLAC STREAMINFO"))?;

    if cursor >= file_len {
        return Err(ImportError::Invalid("FLAC file contains no audio frames"));
    }

    Ok(FlacLayout {
        min_block_size,
        max_block_size,
        sample_rate,
        channels,
        bits_per_sample,
        total_samples,
        first_frame_offset: cursor,
        seektable,
    })
}

/// Resident whole-file FLAC decode into stereo `f32`.
pub(super) fn decode(
    bytes: &[u8],
    max_frames: usize,
    cancel: &AtomicBool,
) -> Result<(Audio, u32, usize), ImportError> {
    let mut cursor = Cursor::new(bytes);
    let layout = scan(&mut cursor, bytes.len() as u64, usize::MAX)?;
    let frame_count = usize::try_from(layout.total_samples)
        .map_err(|_| ImportError::LimitExceeded("decoded frames"))?;
    if frame_count > max_frames {
        return Err(ImportError::LimitExceeded("decoded frames"));
    }

    let mut frames = Vec::new();
    frames
        .try_reserve_exact(frame_count)
        .map_err(|_| ImportError::AllocationFailed)?;

    let mut offset = usize::try_from(layout.first_frame_offset)
        .map_err(|_| ImportError::Invalid("FLAC offset exceeds address space"))?;
    let mut scratch = FlacScratch::new(usize::from(layout.max_block_size))?;
    let mut decoded_total = 0_u64;

    while decoded_total < layout.total_samples {
        crate::cancelled(cancel)?;
        if offset >= bytes.len() {
            return Err(ImportError::Invalid("truncated FLAC audio stream"));
        }
        let mut reader = SliceBitReader::new(&bytes[offset..]);
        let frame_info = decode_frame_into(&mut reader, &layout, &mut scratch)?;
        let consumed = reader.bytes_consumed();
        offset = offset
            .checked_add(consumed)
            .ok_or(ImportError::Invalid("FLAC frame offset overflow"))?;
        if frame_info.block_size == 0
            || decoded_total
                .checked_add(frame_info.block_size as u64)
                .is_none_or(|end| end > layout.total_samples)
        {
            return Err(ImportError::Invalid("FLAC frame count exceeds STREAMINFO"));
        }
        scratch.append_stereo_f32(
            &mut frames,
            frame_info.block_size,
            layout.channels,
            frame_info.bits_per_sample,
        );
        decoded_total += frame_info.block_size as u64;
    }

    let audio = Audio::new(layout.sample_rate, frames)
        .map_err(|_| ImportError::Invalid("decoded samples are not valid finite audio"))?;
    Ok((audio, layout.sample_rate, frame_count))
}

/// Random-access / streaming FLAC window read over any `Read + Seek` source
/// (including `VerifiedPageReader` so every raw page is SHA-256 verified).
pub(super) fn read_window<R: Read + Seek>(
    reader: &mut R,
    layout: &FlacLayout,
    stream_cursor: &mut FlacStreamCursor,
    file_len: u64,
    start_frame: u64,
    output: &mut [Frame],
    mut cancelled: impl FnMut() -> bool,
) -> Result<usize, ImportError> {
    if start_frame > layout.total_samples {
        return Err(ImportError::Invalid("window starts beyond source end"));
    }
    let available = layout.total_samples - start_frame;
    let requested = u64::try_from(output.len())
        .map_err(|_| ImportError::Invalid("requested frame count overflow"))?;
    let decode_count = usize::try_from(available.min(requested))
        .map_err(|_| ImportError::Invalid("window frame count exceeds address space"))?;
    if decode_count == 0 {
        return Ok(0);
    }
    let target_end = start_frame
        .checked_add(decode_count as u64)
        .ok_or(ImportError::Invalid("requested frame range overflow"))?;

    // Choose the best verified starting offset <= start_frame:
    // 1. Cached sequential cursor if it is <= start_frame (and closer than seektable)
    // 2. Latest SEEKTABLE entry <= start_frame
    // 3. first_frame_offset (sample 0)
    let mut pos_sample = 0_u64;
    let mut pos_offset = layout.first_frame_offset;

    if let Some(sp) = layout
        .seektable
        .iter()
        .rev()
        .find(|sp| sp.sample_number <= start_frame)
        && let Some(abs_offset) = layout.first_frame_offset.checked_add(sp.stream_offset)
        && abs_offset < file_len
    {
        pos_sample = sp.sample_number;
        pos_offset = abs_offset;
    }

    if stream_cursor.valid
        && stream_cursor.next_sample <= start_frame
        && stream_cursor.next_sample >= pos_sample
        && stream_cursor.next_offset < file_len
    {
        pos_sample = stream_cursor.next_sample;
        pos_offset = stream_cursor.next_offset;
    }

    let mut scratch = FlacScratch::new(usize::from(layout.max_block_size))?;
    let mut written = 0_usize;

    while written < decode_count {
        if cancelled() {
            return Err(ImportError::Cancelled);
        }
        if pos_offset >= file_len {
            return Err(ImportError::Invalid("truncated FLAC audio stream"));
        }
        reader.seek(SeekFrom::Start(pos_offset))?;
        let mut bit_reader = StreamBitReader::new(reader, file_len - pos_offset);
        let frame_info = decode_frame_into(&mut bit_reader, layout, &mut scratch)?;
        let consumed = bit_reader.bytes_consumed();
        if consumed == 0 || frame_info.block_size == 0 {
            return Err(ImportError::Invalid("invalid zero-length FLAC frame"));
        }
        let frame_start = frame_info.sample_number.unwrap_or(pos_sample);
        let frame_end = frame_start
            .checked_add(frame_info.block_size as u64)
            .ok_or(ImportError::Invalid("FLAC sample position overflow"))?;
        if frame_end > layout.total_samples {
            return Err(ImportError::Invalid("FLAC frame exceeds STREAMINFO"));
        }
        let next_offset = pos_offset
            .checked_add(consumed)
            .ok_or(ImportError::Invalid("FLAC offset overflow"))?;

        if frame_end > start_frame && frame_start < target_end {
            let copy_start = start_frame.max(frame_start);
            let copy_end = target_end.min(frame_end);
            let local_start = (copy_start - frame_start) as usize;
            let local_len = (copy_end - copy_start) as usize;
            let dst_start = (copy_start - start_frame) as usize;
            scratch.write_stereo_f32(
                &mut output[dst_start..dst_start + local_len],
                local_start,
                layout.channels,
                frame_info.bits_per_sample,
            );
            written = written.max(dst_start + local_len);
        }

        pos_sample = frame_end;
        pos_offset = next_offset;
        stream_cursor.next_sample = pos_sample;
        stream_cursor.next_offset = pos_offset;
        stream_cursor.valid = true;

        if frame_end >= target_end {
            break;
        }
    }

    Ok(decode_count)
}

// ---------------------------------------------------------------------------
// Core RFC 9639 Frame & Subframe Decoder
// ---------------------------------------------------------------------------

struct FlacScratch {
    ch0: Vec<i64>,
    ch1: Vec<i64>,
}

impl FlacScratch {
    fn new(max_block_size: usize) -> Result<Self, ImportError> {
        let cap = max_block_size.min(MAX_BLOCK_SIZE);
        let mut ch0 = Vec::new();
        ch0.try_reserve_exact(cap)
            .map_err(|_| ImportError::AllocationFailed)?;
        ch0.resize(cap, 0);
        let mut ch1 = Vec::new();
        ch1.try_reserve_exact(cap)
            .map_err(|_| ImportError::AllocationFailed)?;
        ch1.resize(cap, 0);
        Ok(Self { ch0, ch1 })
    }

    fn append_stereo_f32(&self, out: &mut Vec<Frame>, block_size: usize, channels: u16, bps: u16) {
        let scale = sample_scale(bps);
        if channels == 1 {
            for &s in &self.ch0[..block_size] {
                let v = (s as f64 * scale) as f32;
                out.push([v, v]);
            }
        } else {
            for i in 0..block_size {
                let l = (self.ch0[i] as f64 * scale) as f32;
                let r = (self.ch1[i] as f64 * scale) as f32;
                out.push([l, r]);
            }
        }
    }

    fn write_stereo_f32(&self, out: &mut [Frame], local_start: usize, channels: u16, bps: u16) {
        let scale = sample_scale(bps);
        if channels == 1 {
            for (dst, &s) in out.iter_mut().zip(&self.ch0[local_start..]) {
                let v = (s as f64 * scale) as f32;
                *dst = [v, v];
            }
        } else {
            for (i, dst) in out.iter_mut().enumerate() {
                let l = (self.ch0[local_start + i] as f64 * scale) as f32;
                let r = (self.ch1[local_start + i] as f64 * scale) as f32;
                *dst = [l, r];
            }
        }
    }
}

fn sample_scale(bps: u16) -> f64 {
    let shift = u32::from(bps.saturating_sub(1));
    1.0 / ((1_u64 << shift) as f64)
}

struct DecodedFrameInfo {
    block_size: usize,
    bits_per_sample: u16,
    sample_number: Option<u64>,
}

trait BitSource {
    fn read_byte_raw(&mut self) -> Result<u8, ImportError>;
    fn read_bits(&mut self, n: u32) -> Result<u64, ImportError>;
    fn read_signed_bits(&mut self, n: u32) -> Result<i64, ImportError> {
        let raw = self.read_bits(n)?;
        Ok(sign_extend(raw, n))
    }
    fn read_unary(&mut self) -> Result<u32, ImportError>;
    fn align_to_byte(&mut self) -> Result<(), ImportError>;
    fn crc8(&self) -> u8;
    fn crc16(&self) -> u16;
}

fn sign_extend(raw: u64, bits: u32) -> i64 {
    if bits == 0 {
        return 0;
    }
    if bits >= 64 {
        return raw as i64;
    }
    let shift = 64 - bits;
    ((raw << shift) as i64) >> shift
}

struct SliceBitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    bits: u32,
    crc8: u8,
    crc16: u16,
}

impl<'a> SliceBitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            buf: 0,
            bits: 0,
            crc8: 0,
            crc16: 0,
        }
    }

    fn bytes_consumed(&self) -> usize {
        self.pos
    }

    fn pull_byte(&mut self) -> Result<u8, ImportError> {
        if self.pos >= self.data.len() {
            return Err(ImportError::Invalid("truncated FLAC frame"));
        }
        if self.pos >= MAX_FLAC_FRAME_BYTES {
            return Err(ImportError::LimitExceeded("FLAC frame bytes"));
        }
        let b = self.data[self.pos];
        self.pos += 1;
        self.crc8 = update_crc8(self.crc8, b);
        self.crc16 = update_crc16(self.crc16, b);
        Ok(b)
    }
}

impl BitSource for SliceBitReader<'_> {
    fn read_byte_raw(&mut self) -> Result<u8, ImportError> {
        debug_assert_eq!(self.bits, 0);
        self.pull_byte()
    }

    fn read_bits(&mut self, mut n: u32) -> Result<u64, ImportError> {
        if n == 0 {
            return Ok(0);
        }
        let mut result = 0_u64;
        while n > 0 {
            if self.bits == 0 {
                let b = self.pull_byte()?;
                self.buf = u64::from(b);
                self.bits = 8;
            }
            let take = n.min(self.bits);
            self.bits -= take;
            let chunk = (self.buf >> self.bits) & ((1_u64 << take) - 1);
            result = (result << take) | chunk;
            n -= take;
        }
        Ok(result)
    }

    fn read_unary(&mut self) -> Result<u32, ImportError> {
        let mut count = 0_u32;
        loop {
            if self.bits == 0 {
                let b = self.pull_byte()?;
                self.buf = u64::from(b);
                self.bits = 8;
            }
            self.bits -= 1;
            if ((self.buf >> self.bits) & 1) != 0 {
                return Ok(count);
            }
            count = count
                .checked_add(1)
                .ok_or(ImportError::Invalid("FLAC unary residual overflow"))?;
            if count > 1 << 20 {
                return Err(ImportError::Invalid("FLAC unary residual exceeds bound"));
            }
        }
    }

    fn align_to_byte(&mut self) -> Result<(), ImportError> {
        if self.bits > 0 {
            let mask = (1_u64 << self.bits) - 1;
            if (self.buf & mask) != 0 {
                return Err(ImportError::Invalid("non-zero FLAC frame padding bits"));
            }
            self.bits = 0;
            self.buf = 0;
        }
        Ok(())
    }

    fn crc8(&self) -> u8 {
        self.crc8
    }

    fn crc16(&self) -> u16 {
        self.crc16
    }
}

struct StreamBitReader<'a, R: Read> {
    reader: &'a mut R,
    remaining: u64,
    consumed: u64,
    chunk: [u8; 4096],
    chunk_pos: usize,
    chunk_len: usize,
    buf: u64,
    bits: u32,
    crc8: u8,
    crc16: u16,
}

impl<'a, R: Read> StreamBitReader<'a, R> {
    fn new(reader: &'a mut R, remaining: u64) -> Self {
        Self {
            reader,
            remaining,
            consumed: 0,
            chunk: [0_u8; 4096],
            chunk_pos: 0,
            chunk_len: 0,
            buf: 0,
            bits: 0,
            crc8: 0,
            crc16: 0,
        }
    }

    fn bytes_consumed(&self) -> u64 {
        self.consumed
    }

    fn pull_byte(&mut self) -> Result<u8, ImportError> {
        if self.consumed >= MAX_FLAC_FRAME_BYTES as u64 {
            return Err(ImportError::LimitExceeded("FLAC frame bytes"));
        }
        if self.chunk_pos == self.chunk_len {
            if self.remaining == 0 {
                return Err(ImportError::Invalid("truncated FLAC frame"));
            }
            let to_read = usize::try_from(self.remaining.min(self.chunk.len() as u64))
                .unwrap_or(self.chunk.len());
            let n = self.reader.read(&mut self.chunk[..to_read])?;
            if n == 0 {
                return Err(ImportError::Invalid("truncated FLAC frame"));
            }
            self.chunk_pos = 0;
            self.chunk_len = n;
            self.remaining -= n as u64;
        }
        let b = self.chunk[self.chunk_pos];
        self.chunk_pos += 1;
        self.consumed += 1;
        self.crc8 = update_crc8(self.crc8, b);
        self.crc16 = update_crc16(self.crc16, b);
        Ok(b)
    }
}

impl<R: Read> BitSource for StreamBitReader<'_, R> {
    fn read_byte_raw(&mut self) -> Result<u8, ImportError> {
        debug_assert_eq!(self.bits, 0);
        self.pull_byte()
    }

    fn read_bits(&mut self, mut n: u32) -> Result<u64, ImportError> {
        if n == 0 {
            return Ok(0);
        }
        let mut result = 0_u64;
        while n > 0 {
            if self.bits == 0 {
                let b = self.pull_byte()?;
                self.buf = u64::from(b);
                self.bits = 8;
            }
            let take = n.min(self.bits);
            self.bits -= take;
            let chunk = (self.buf >> self.bits) & ((1_u64 << take) - 1);
            result = (result << take) | chunk;
            n -= take;
        }
        Ok(result)
    }

    fn read_unary(&mut self) -> Result<u32, ImportError> {
        let mut count = 0_u32;
        loop {
            if self.bits == 0 {
                let b = self.pull_byte()?;
                self.buf = u64::from(b);
                self.bits = 8;
            }
            self.bits -= 1;
            if ((self.buf >> self.bits) & 1) != 0 {
                return Ok(count);
            }
            count = count
                .checked_add(1)
                .ok_or(ImportError::Invalid("FLAC unary residual overflow"))?;
            if count > 1 << 20 {
                return Err(ImportError::Invalid("FLAC unary residual exceeds bound"));
            }
        }
    }

    fn align_to_byte(&mut self) -> Result<(), ImportError> {
        if self.bits > 0 {
            let mask = (1_u64 << self.bits) - 1;
            if (self.buf & mask) != 0 {
                return Err(ImportError::Invalid("non-zero FLAC frame padding bits"));
            }
            self.bits = 0;
            self.buf = 0;
        }
        Ok(())
    }

    fn crc8(&self) -> u8 {
        self.crc8
    }

    fn crc16(&self) -> u16 {
        self.crc16
    }
}

fn decode_frame_into(
    bits: &mut impl BitSource,
    layout: &FlacLayout,
    scratch: &mut FlacScratch,
) -> Result<DecodedFrameInfo, ImportError> {
    let b0 = bits.read_byte_raw()?;
    let b1 = bits.read_byte_raw()?;
    if b0 != 0xFF || (b1 & 0xFE) != 0xF8 {
        return Err(ImportError::Invalid("invalid FLAC frame sync code"));
    }
    let variable_block_size = (b1 & 0x01) != 0;

    let b2 = bits.read_byte_raw()?;
    let bs_code = b2 >> 4;
    let sr_code = b2 & 0x0F;

    let b3 = bits.read_byte_raw()?;
    let ch_assign = b3 >> 4;
    let ss_code = (b3 >> 1) & 0x07;
    if (b3 & 0x01) != 0 {
        return Err(ImportError::Invalid("reserved FLAC frame header bit set"));
    }

    // UTF-8 coded frame or sample number
    let coded_num = read_utf8_u64(bits)?;

    let block_size: usize = match bs_code {
        0 => return Err(ImportError::Invalid("reserved FLAC block size code")),
        1 => 192,
        2..=5 => 576 << (bs_code - 2),
        6 => usize::from(bits.read_byte_raw()?) + 1,
        7 => {
            let hi = usize::from(bits.read_byte_raw()?);
            let lo = usize::from(bits.read_byte_raw()?);
            ((hi << 8) | lo) + 1
        }
        8..=15 => 256 << (bs_code - 8),
        _ => unreachable!(),
    };
    if block_size == 0 || block_size > usize::from(layout.max_block_size) {
        return Err(ImportError::Invalid("FLAC block size exceeds STREAMINFO"));
    }

    let frame_sample_rate: u32 = match sr_code {
        0 => layout.sample_rate,
        1 => 88_200,
        2 => 176_400,
        3 => 192_000,
        4 => 8_000,
        5 => 16_000,
        6 => 22_050,
        7 => 24_000,
        8 => 32_000,
        9 => 44_100,
        10 => 48_000,
        11 => 96_000,
        12 => u32::from(bits.read_byte_raw()?) * 1_000,
        13 => {
            let hi = u32::from(bits.read_byte_raw()?);
            let lo = u32::from(bits.read_byte_raw()?);
            (hi << 8) | lo
        }
        14 => {
            let hi = u32::from(bits.read_byte_raw()?);
            let lo = u32::from(bits.read_byte_raw()?);
            ((hi << 8) | lo) * 10
        }
        _ => return Err(ImportError::Invalid("invalid FLAC sample rate code")),
    };
    if frame_sample_rate != layout.sample_rate {
        return Err(ImportError::Invalid(
            "FLAC frame sample rate differs from STREAMINFO",
        ));
    }

    let frame_bps: u16 = match ss_code {
        0 => layout.bits_per_sample,
        1 => 8,
        2 => 12,
        4 => 16,
        5 => 20,
        6 => 24,
        7 => 32,
        _ => return Err(ImportError::Invalid("reserved FLAC sample size code")),
    };
    if frame_bps != layout.bits_per_sample {
        return Err(ImportError::Invalid(
            "FLAC frame bit depth differs from STREAMINFO",
        ));
    }

    let expected_crc8 = bits.crc8();
    let stored_crc8 = bits.read_byte_raw()?;
    if stored_crc8 != expected_crc8 {
        return Err(ImportError::Invalid("FLAC frame header CRC-8 mismatch"));
    }

    // Channel assignment
    match (layout.channels, ch_assign) {
        (1, 0) => {
            decode_subframe(bits, block_size, frame_bps, &mut scratch.ch0[..block_size])?;
        }
        (2, 1) => {
            decode_subframe(bits, block_size, frame_bps, &mut scratch.ch0[..block_size])?;
            decode_subframe(bits, block_size, frame_bps, &mut scratch.ch1[..block_size])?;
        }
        (2, 8) => {
            // Left-side: ch0 = left (bps), ch1 = side = left - right (bps + 1)
            decode_subframe(bits, block_size, frame_bps, &mut scratch.ch0[..block_size])?;
            decode_subframe(
                bits,
                block_size,
                frame_bps + 1,
                &mut scratch.ch1[..block_size],
            )?;
            for i in 0..block_size {
                scratch.ch1[i] = scratch.ch0[i].wrapping_sub(scratch.ch1[i]);
            }
        }
        (2, 9) => {
            // Right-side: ch0 = side = left - right (bps + 1), ch1 = right (bps)
            decode_subframe(
                bits,
                block_size,
                frame_bps + 1,
                &mut scratch.ch0[..block_size],
            )?;
            decode_subframe(bits, block_size, frame_bps, &mut scratch.ch1[..block_size])?;
            for i in 0..block_size {
                scratch.ch0[i] = scratch.ch0[i].wrapping_add(scratch.ch1[i]);
            }
        }
        (2, 10) => {
            // Mid-side: ch0 = mid (bps), ch1 = side (bps + 1)
            decode_subframe(bits, block_size, frame_bps, &mut scratch.ch0[..block_size])?;
            decode_subframe(
                bits,
                block_size,
                frame_bps + 1,
                &mut scratch.ch1[..block_size],
            )?;
            for i in 0..block_size {
                let side = scratch.ch1[i];
                let mid = (scratch.ch0[i] << 1) | (side & 1);
                scratch.ch0[i] = (mid + side) >> 1;
                scratch.ch1[i] = (mid - side) >> 1;
            }
        }
        _ => {
            return Err(ImportError::Invalid("unsupported FLAC channel assignment"));
        }
    }

    bits.align_to_byte()?;
    let expected_crc16 = bits.crc16();
    let crc_hi = u16::from(bits.read_byte_raw()?);
    let crc_lo = u16::from(bits.read_byte_raw()?);
    let stored_crc16 = (crc_hi << 8) | crc_lo;
    if stored_crc16 != expected_crc16 {
        return Err(ImportError::Invalid("FLAC frame footer CRC-16 mismatch"));
    }

    let sample_number = if variable_block_size {
        Some(coded_num)
    } else if layout.min_block_size == layout.max_block_size {
        coded_num.checked_mul(u64::from(layout.min_block_size))
    } else {
        None
    };

    Ok(DecodedFrameInfo {
        block_size,
        bits_per_sample: frame_bps,
        sample_number,
    })
}

fn decode_subframe(
    bits: &mut impl BitSource,
    block_size: usize,
    mut bps: u16,
    out: &mut [i64],
) -> Result<(), ImportError> {
    let header = bits.read_bits(8)? as u8;
    if (header & 0x80) != 0 {
        return Err(ImportError::Invalid("invalid FLAC subframe padding bit"));
    }
    let subframe_type = (header >> 1) & 0x3F;
    let has_wasted = (header & 0x01) != 0;
    let wasted = if has_wasted {
        let k = bits.read_unary()? + 1;
        if k >= u32::from(bps) {
            return Err(ImportError::Invalid("FLAC wasted bits exceed sample width"));
        }
        bps -= k as u16;
        k
    } else {
        0
    };

    match subframe_type {
        0 => {
            // CONSTANT
            let value = bits.read_signed_bits(u32::from(bps))?;
            out[..block_size].fill(value);
        }
        1 => {
            // VERBATIM
            for sample in &mut out[..block_size] {
                *sample = bits.read_signed_bits(u32::from(bps))?;
            }
        }
        8..=12 => {
            // FIXED predictor, order 0..=4
            let order = usize::from(subframe_type - 8);
            if order > block_size {
                return Err(ImportError::Invalid("FLAC fixed order exceeds block size"));
            }
            for sample in &mut out[..order] {
                *sample = bits.read_signed_bits(u32::from(bps))?;
            }
            decode_residual(bits, block_size, order, out)?;
            restore_fixed(order, out);
        }
        32..=63 => {
            // LPC predictor, order 1..=32
            let order = usize::from(subframe_type - 31);
            if order > block_size || order > MAX_SUBFRAME_ORDER {
                return Err(ImportError::Invalid("FLAC LPC order exceeds block size"));
            }
            for sample in &mut out[..order] {
                *sample = bits.read_signed_bits(u32::from(bps))?;
            }
            let precision_raw = bits.read_bits(4)? as u32;
            if precision_raw == 15 {
                return Err(ImportError::Invalid("invalid FLAC LPC precision"));
            }
            let precision = precision_raw + 1;
            let shift = bits.read_signed_bits(5)?;
            if shift < 0 {
                return Err(ImportError::Invalid("negative FLAC LPC shift"));
            }
            let mut coeffs = [0_i64; MAX_SUBFRAME_ORDER];
            for coeff in &mut coeffs[..order] {
                *coeff = bits.read_signed_bits(precision)?;
            }
            decode_residual(bits, block_size, order, out)?;
            restore_lpc(order, &coeffs[..order], shift as u32, out);
        }
        _ => return Err(ImportError::Invalid("reserved FLAC subframe type")),
    }

    if wasted > 0 {
        for sample in &mut out[..block_size] {
            *sample <<= wasted;
        }
    }
    Ok(())
}

fn decode_residual(
    bits: &mut impl BitSource,
    block_size: usize,
    predictor_order: usize,
    out: &mut [i64],
) -> Result<(), ImportError> {
    let method = bits.read_bits(2)?;
    let (param_bits, escape_code) = match method {
        0 => (4_u32, 15_u32),
        1 => (5_u32, 31_u32),
        _ => return Err(ImportError::Invalid("reserved FLAC residual coding method")),
    };
    let partition_order = bits.read_bits(4)? as u32;
    let partitions = 1_usize << partition_order;
    if !block_size.is_multiple_of(partitions) {
        return Err(ImportError::Invalid(
            "FLAC block size not divisible by partition count",
        ));
    }
    let partition_samples = block_size / partitions;
    if partition_samples < predictor_order {
        return Err(ImportError::Invalid(
            "FLAC partition smaller than predictor order",
        ));
    }

    let mut idx = predictor_order;
    for part in 0..partitions {
        let count = if part == 0 {
            partition_samples - predictor_order
        } else {
            partition_samples
        };
        let param = bits.read_bits(param_bits)? as u32;
        if param == escape_code {
            let raw_bits = bits.read_bits(5)? as u32;
            for _ in 0..count {
                out[idx] = if raw_bits == 0 {
                    0
                } else {
                    bits.read_signed_bits(raw_bits)?
                };
                idx += 1;
            }
        } else {
            for _ in 0..count {
                let q = u64::from(bits.read_unary()?);
                let r = bits.read_bits(param)?;
                let folded = (q << param) | r;
                let signed = ((folded >> 1) as i64) ^ -((folded & 1) as i64);
                out[idx] = signed;
                idx += 1;
            }
        }
    }
    Ok(())
}

fn restore_fixed(order: usize, out: &mut [i64]) {
    match order {
        0 => {}
        1 => {
            for i in 1..out.len() {
                out[i] = out[i].wrapping_add(out[i - 1]);
            }
        }
        2 => {
            for i in 2..out.len() {
                let pred = 2_i64.wrapping_mul(out[i - 1]).wrapping_sub(out[i - 2]);
                out[i] = out[i].wrapping_add(pred);
            }
        }
        3 => {
            for i in 3..out.len() {
                let pred = 3_i64
                    .wrapping_mul(out[i - 1])
                    .wrapping_sub(3_i64.wrapping_mul(out[i - 2]))
                    .wrapping_add(out[i - 3]);
                out[i] = out[i].wrapping_add(pred);
            }
        }
        4 => {
            for i in 4..out.len() {
                let pred = 4_i64
                    .wrapping_mul(out[i - 1])
                    .wrapping_sub(6_i64.wrapping_mul(out[i - 2]))
                    .wrapping_add(4_i64.wrapping_mul(out[i - 3]))
                    .wrapping_sub(out[i - 4]);
                out[i] = out[i].wrapping_add(pred);
            }
        }
        _ => {}
    }
}

fn restore_lpc(order: usize, coeffs: &[i64], shift: u32, out: &mut [i64]) {
    for i in order..out.len() {
        let mut sum = 0_i64;
        for j in 0..order {
            sum = sum.wrapping_add(coeffs[j].wrapping_mul(out[i - 1 - j]));
        }
        out[i] = out[i].wrapping_add(sum >> shift);
    }
}

fn read_utf8_u64(bits: &mut impl BitSource) -> Result<u64, ImportError> {
    let first = bits.read_byte_raw()?;
    if first & 0x80 == 0 {
        return Ok(u64::from(first));
    }
    let ones = first.leading_ones();
    if !(2..=7).contains(&ones) {
        return Err(ImportError::Invalid("invalid FLAC UTF-8 coded number"));
    }
    let mask = if ones == 7 {
        0
    } else {
        (1_u8 << (7 - ones)) - 1
    };
    let mut val = u64::from(first & mask);
    for _ in 1..ones {
        let cont = bits.read_byte_raw()?;
        if (cont & 0xC0) != 0x80 {
            return Err(ImportError::Invalid("invalid FLAC UTF-8 continuation byte"));
        }
        val = (val << 6) | u64::from(cont & 0x3F);
    }
    Ok(val)
}

fn update_crc8(mut crc: u8, byte: u8) -> u8 {
    crc ^= byte;
    for _ in 0..8 {
        crc = if (crc & 0x80) != 0 {
            (crc << 1) ^ 0x07
        } else {
            crc << 1
        };
    }
    crc
}

fn update_crc16(mut crc: u16, byte: u8) -> u16 {
    crc ^= u16::from(byte) << 8;
    for _ in 0..8 {
        crc = if (crc & 0x8000) != 0 {
            (crc << 1) ^ 0x8005
        } else {
            crc << 1
        };
    }
    crc
}

fn read_exact_invalid<R: Read>(
    reader: &mut R,
    bytes: &mut [u8],
    reason: &'static str,
) -> Result<(), ImportError> {
    match reader.read_exact(bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            Err(ImportError::Invalid(reason))
        }
        Err(error) => Err(ImportError::Io(error)),
    }
}

/// Minimal RFC 9639 FLAC encoder for deterministic unit/streaming/relink tests.
#[cfg(test)]
pub(crate) fn encode_test_flac(
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    block_size: usize,
    samples: &[Vec<i32>],
    include_seektable: bool,
    use_fixed_predictor: bool,
) -> Vec<u8> {
    assert!((1..=2).contains(&channels));
    assert_eq!(samples.len(), usize::from(channels));
    let total_samples = samples[0].len();
    assert!(total_samples > 0);
    assert!((16..=65_535).contains(&block_size));

    let mut frame_bytes = Vec::new();
    let mut seekpoints: Vec<(u64, u64, u16)> = Vec::new();
    let mut frame_idx = 0_u64;
    let mut offset = 0_usize;

    while offset < total_samples {
        let bs = (total_samples - offset).min(block_size);
        let stream_offset = frame_bytes.len() as u64;
        seekpoints.push((offset as u64, stream_offset, bs as u16));
        let frame = encode_test_frame(
            sample_rate,
            channels,
            bits_per_sample,
            bs,
            frame_idx,
            offset as u64,
            bs != block_size,
            samples,
            offset,
            use_fixed_predictor,
        );
        frame_bytes.extend_from_slice(&frame);
        offset += bs;
        frame_idx += 1;
    }

    let mut out = b"fLaC".to_vec();
    // STREAMINFO header
    let si_flag = if include_seektable { 0x00_u8 } else { 0x80_u8 };
    out.push(si_flag);
    out.extend_from_slice(&[0, 0, 34]);
    let min_bs = (block_size.min(total_samples)) as u16;
    let max_bs = block_size as u16;
    out.extend_from_slice(&min_bs.to_be_bytes());
    out.extend_from_slice(&max_bs.to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // min/max frame size unknown
    let sr = sample_rate;
    let ch = u32::from(channels - 1);
    let bps = u32::from(bits_per_sample - 1);
    let total = total_samples as u64;
    out.push((sr >> 12) as u8);
    out.push(((sr >> 4) & 0xFF) as u8);
    out.push((((sr & 0x0F) << 4) | (ch << 1) | (bps >> 4)) as u8);
    out.push((((bps & 0x0F) << 4) as u8) | ((total >> 32) as u8 & 0x0F));
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(&[0_u8; 16]); // MD5 = 0 (unset)

    if include_seektable {
        let st_len = (seekpoints.len() * 18) as u32;
        out.push(0x80 | 3);
        out.push(((st_len >> 16) & 0xFF) as u8);
        out.push(((st_len >> 8) & 0xFF) as u8);
        out.push((st_len & 0xFF) as u8);
        for (sample, off, count) in seekpoints {
            out.extend_from_slice(&sample.to_be_bytes());
            out.extend_from_slice(&off.to_be_bytes());
            out.extend_from_slice(&count.to_be_bytes());
        }
    }

    out.extend_from_slice(&frame_bytes);
    out
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn encode_test_frame(
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    bs: usize,
    frame_idx: u64,
    sample_idx: u64,
    variable_bs: bool,
    samples: &[Vec<i32>],
    offset: usize,
    use_fixed_predictor: bool,
) -> Vec<u8> {
    struct BitWriter {
        bytes: Vec<u8>,
        cur: u8,
        bits: u8,
    }
    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                cur: 0,
                bits: 0,
            }
        }
        fn push_byte(&mut self, b: u8) {
            assert_eq!(self.bits, 0);
            self.bytes.push(b);
        }
        fn push_bits(&mut self, val: u64, mut n: u32) {
            while n > 0 {
                let space = 8 - u32::from(self.bits);
                let take = n.min(space);
                let shift = n - take;
                let chunk = ((val >> shift) & ((1_u64 << take) - 1)) as u8;
                self.cur = if take == 8 {
                    chunk
                } else {
                    (self.cur << take) | chunk
                };
                self.bits += take as u8;
                n -= take;
                if self.bits == 8 {
                    self.bytes.push(self.cur);
                    self.cur = 0;
                    self.bits = 0;
                }
            }
        }
        fn push_unary(&mut self, q: u32) {
            for _ in 0..q {
                self.push_bits(0, 1);
            }
            self.push_bits(1, 1);
        }
        fn align(&mut self) {
            if self.bits > 0 {
                let pad = 8 - u32::from(self.bits);
                self.push_bits(0, pad);
            }
        }
    }

    let mut w = BitWriter::new();
    w.push_byte(0xFF);
    w.push_byte(if variable_bs { 0xF9 } else { 0xF8 });
    // bs_code = 7 (16-bit explicit block size - 1), sr_code = 0 (from STREAMINFO)
    w.push_byte(0x70);
    let ch_assign: u8 = if channels == 1 { 0 } else { 1 };
    w.push_byte(ch_assign << 4); // ss_code = 0 (from STREAMINFO)

    // UTF-8 coded number
    let num = if variable_bs { sample_idx } else { frame_idx };
    if num < 0x80 {
        w.push_byte(num as u8);
    } else if num < 0x800 {
        w.push_byte(0xC0 | ((num >> 6) as u8));
        w.push_byte(0x80 | ((num & 0x3F) as u8));
    } else if num < 0x10000 {
        w.push_byte(0xE0 | ((num >> 12) as u8));
        w.push_byte(0x80 | (((num >> 6) & 0x3F) as u8));
        w.push_byte(0x80 | ((num & 0x3F) as u8));
    } else {
        w.push_byte(0xF0 | ((num >> 18) as u8));
        w.push_byte(0x80 | (((num >> 12) & 0x3F) as u8));
        w.push_byte(0x80 | (((num >> 6) & 0x3F) as u8));
        w.push_byte(0x80 | ((num & 0x3F) as u8));
    }

    let bs_minus_1 = (bs - 1) as u16;
    w.push_byte((bs_minus_1 >> 8) as u8);
    w.push_byte((bs_minus_1 & 0xFF) as u8);
    let _ = sample_rate;

    let mut crc8 = 0_u8;
    for &b in &w.bytes {
        crc8 = update_crc8(crc8, b);
    }
    w.push_byte(crc8);

    let bps = u32::from(bits_per_sample);
    let mask = if bps == 32 {
        u32::MAX as u64
    } else {
        (1_u64 << bps) - 1
    };

    for channel in samples.iter().take(usize::from(channels)) {
        let slice = &channel[offset..offset + bs];
        if slice.iter().all(|&s| s == slice[0]) {
            // CONSTANT subframe
            w.push_bits(0, 8);
            w.push_bits((slice[0] as i64 as u64) & mask, bps);
        } else if use_fixed_predictor && bs >= 2 {
            // FIXED order 1 with Rice2 coding (param = 12)
            w.push_bits((9_u64) << 1, 8);
            w.push_bits((slice[0] as i64 as u64) & mask, bps);
            w.push_bits(1, 2); // Rice2
            w.push_bits(0, 4); // partition order 0 (1 partition)
            let param = 12_u32;
            w.push_bits(u64::from(param), 5);
            for i in 1..bs {
                let res = i64::from(slice[i]) - i64::from(slice[i - 1]);
                let folded = if res >= 0 {
                    (res as u64) << 1
                } else {
                    ((-res - 1) as u64) << 1 | 1
                };
                let q = (folded >> param) as u32;
                let r = folded & ((1_u64 << param) - 1);
                w.push_unary(q);
                w.push_bits(r, param);
            }
        } else {
            // VERBATIM subframe
            w.push_bits(1 << 1, 8);
            for &s in slice {
                w.push_bits((s as i64 as u64) & mask, bps);
            }
        }
    }

    w.align();
    let mut crc16 = 0_u16;
    for &b in &w.bytes {
        crc16 = update_crc16(crc16, b);
    }
    w.push_byte((crc16 >> 8) as u8);
    w.push_byte((crc16 & 0xFF) as u8);
    w.bytes
}
