//! Unified container dispatch for RIFF/WAVE, RF64/BW64, AIFF/AIFC, and FLAC.
use crate::{ImportError, WavTruncation, aiff, flac, wav};
use hex_engine::Audio;
use std::{
    io::{Read, Seek, SeekFrom},
    sync::atomic::AtomicBool,
};

/// Detected audio container format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaFormat {
    Wav,
    Rf64,
    Bw64,
    Aiff,
    Aifc,
    Flac,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MediaLayout {
    Wav(wav::Layout),
    Aiff(aiff::AiffLayout),
    Flac(flac::FlacLayout),
}

impl MediaLayout {
    pub(crate) fn require_seekable(&self) -> Result<(), ImportError> {
        match self {
            Self::Wav(layout) => layout.format.require_fixed_frames(),
            Self::Aiff(_) | Self::Flac(_) => Ok(()),
        }
    }

    pub(crate) const fn sample_rate(&self) -> u32 {
        match self {
            Self::Wav(l) => l.format.sample_rate,
            Self::Aiff(l) => l.format.sample_rate,
            Self::Flac(l) => l.sample_rate,
        }
    }

    pub(crate) const fn frames(&self) -> u64 {
        match self {
            Self::Wav(l) => l.frames,
            Self::Aiff(l) => l.frames,
            Self::Flac(l) => l.total_samples,
        }
    }

    pub(crate) const fn source_channels(&self) -> u16 {
        match self {
            Self::Wav(l) => l.format.channels,
            Self::Aiff(l) => l.format.channels,
            Self::Flac(l) => l.channels,
        }
    }

    pub(crate) const fn bits_per_sample(&self) -> u16 {
        match self {
            Self::Wav(l) => l.format.bits_per_sample,
            Self::Aiff(l) => l.format.bits_per_sample,
            Self::Flac(l) => l.bits_per_sample,
        }
    }

    pub(crate) const fn compat_encoding(&self) -> u16 {
        match self {
            Self::Wav(l) => l.format.encoding,
            Self::Aiff(l) => l.format.compat_encoding(),
            Self::Flac(_) => 1,
        }
    }
}

/// Detect the container format from the first 12 header bytes.
pub fn detect_format(prefix: &[u8]) -> Option<MediaFormat> {
    if prefix.len() >= 4 && &prefix[..4] == b"fLaC" {
        return Some(MediaFormat::Flac);
    }
    if prefix.len() >= 12 {
        match (&prefix[..4], &prefix[8..12]) {
            (b"RIFF", b"WAVE") => return Some(MediaFormat::Wav),
            (b"RF64", b"WAVE") => return Some(MediaFormat::Rf64),
            (b"BW64", b"WAVE") => return Some(MediaFormat::Bw64),
            (b"FORM", b"AIFF") => return Some(MediaFormat::Aiff),
            (b"FORM", b"AIFC") => return Some(MediaFormat::Aifc),
            _ => {}
        }
    }
    None
}

/// Scan any supported audio container (RIFF/WAV, RF64/BW64, AIFF/AIFC, FLAC).
pub(crate) fn scan<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
    header_limit: usize,
) -> Result<MediaLayout, ImportError> {
    reader.seek(SeekFrom::Start(0))?;
    let mut magic = [0_u8; 4];
    match reader.read_exact(&mut magic) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(ImportError::Invalid("truncated RIFF/WAVE header"));
        }
        Err(error) => return Err(ImportError::Io(error)),
    }
    match &magic {
        b"RIFF" | b"RF64" | b"BW64" => {
            wav::scan(reader, file_len, header_limit).map(MediaLayout::Wav)
        }
        b"FORM" => aiff::scan(reader, file_len, header_limit).map(MediaLayout::Aiff),
        b"fLaC" => flac::scan(reader, file_len, header_limit).map(MediaLayout::Flac),
        _ => Err(ImportError::Invalid("missing RIFF/WAVE header")),
    }
}

/// Decode any supported in-memory audio container into stereo `f32`.
pub(crate) fn decode(
    bytes: &[u8],
    max_frames: usize,
    cancel: &AtomicBool,
) -> Result<(Audio, u32, usize), ImportError> {
    if bytes.len() < 4 {
        return Err(ImportError::Invalid("truncated RIFF/WAVE header"));
    }
    match &bytes[..4] {
        b"RIFF" | b"RF64" | b"BW64" => wav::decode(bytes, max_frames, cancel),
        b"FORM" => aiff::decode(bytes, max_frames, cancel),
        b"fLaC" => flac::decode(bytes, max_frames, cancel),
        _ => Err(ImportError::Invalid("missing RIFF/WAVE header")),
    }
}

/// Decode any supported in-memory audio container with explicit truncation recovery.
pub(crate) fn decode_recoverable(
    bytes: &[u8],
    max_frames: usize,
    cancel: &AtomicBool,
) -> Result<(wav::Decoded, Option<WavTruncation>), ImportError> {
    if bytes.len() < 4 {
        return Err(ImportError::Invalid("truncated RIFF/WAVE header"));
    }
    match &bytes[..4] {
        b"RIFF" | b"RF64" | b"BW64" => wav::decode_recoverable(bytes, max_frames, cancel),
        b"FORM" => aiff::decode_recoverable(bytes, max_frames, cancel),
        b"fLaC" => flac::decode(bytes, max_frames, cancel).map(|decoded| (decoded, None)),
        _ => Err(ImportError::Invalid("missing RIFF/WAVE header")),
    }
}
