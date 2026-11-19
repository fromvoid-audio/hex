//! # Purpose
//! Own bounded control-side WAV decoding, verified source readers, sample-rate conversion and
//! waveform summaries.
//!
//! # Dependencies
//! Uses hex-engine/hex-project data contracts and sha2 for source verification.
//!
//! # Does not own
//! Does not edit projects, schedule device playback or open callback-owned files. Original-rate
//! source coordinates remain distinct from project-rate derivatives.
use hex_engine::Audio;
use std::{
    fmt,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

mod adpcm;
mod file_identity;
pub use file_identity::SourceFileIdentity;
mod native_source;
pub use native_source::{NativeDecodedAudio, NativeSourceRefusal};
mod resample;
mod resample_kernel;
mod stream;
mod verified_stream;
mod wav;

#[cfg(test)]
mod wav_tests;

#[cfg(test)]
mod resample_stream_tests;

#[cfg(test)]
mod verified_stream_tests;

#[cfg(test)]
mod resample_plan_tests;

#[cfg(test)]
mod resample_quality_tests;

const READ_CHUNK_BYTES: usize = 16 * 1024;
/// Decoded PCM frames between cooperative cancellation checks.
const CANCEL_CHECK_FRAMES: usize = 64 * 1024;

/// Never-set flag for callers that do not offer cooperative cancellation.
static NOT_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Relaxed is sufficient: the flag carries no data, and a late observation
/// only costs one further bounded chunk of work.
fn cancelled(cancel: &AtomicBool) -> Result<(), ImportError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ImportError::Cancelled)
    } else {
        Ok(())
    }
}

pub use resample::{
    ResampleLimits, ResamplePlan, resample, resample_cancellable, resample_with_version,
};
pub use resample_kernel::{
    ResampleKernel, ResampleKernelDataLayout, ResampleVersion, V2_STOPBAND_DB,
};
pub use stream::{WavFrameReader, WavReaderLimits};
pub use verified_stream::{
    VERIFIED_WAV_PAGE_BYTES, VerifiedReaderDataLayout, VerifiedWavCoverage, VerifiedWavManifest,
    VerifiedWavReader, VerifiedWavReaderLimits,
};

/// Caller-owned resource ceilings for a single import operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_file_bytes: usize,
    pub max_frames: usize,
}

impl Limits {
    pub const fn new(max_file_bytes: usize, max_frames: usize) -> Self {
        Self {
            max_file_bytes,
            max_frames,
        }
    }
}

/// Imported media retains the path spelling supplied by the caller.
#[derive(Debug, Clone)]
pub struct DecodedAudio {
    path: PathBuf,
    audio: Audio,
    sample_rate: u32,
    frames: usize,
    file_identity: Option<SourceFileIdentity>,
}

impl DecodedAudio {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn audio(&self) -> &Audio {
        &self.audio
    }

    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub const fn frames(&self) -> usize {
        self.frames
    }

    /// The decoded output is always stereo; mono input is duplicated.
    pub const fn channels(&self) -> u16 {
        2
    }

    /// Length and modification time of the file that was read, or `None` when
    /// the platform reports no modification time. In-memory only; never saved.
    pub const fn file_identity(&self) -> Option<SourceFileIdentity> {
        self.file_identity
    }

    pub fn into_audio(self) -> Audio {
        self.audio
    }
}

/// Readable extent of a WAV whose audio data is truncated or ends in a partial
/// frame. Such files are refused by default; [`recover_wav_file`] imports the
/// readable whole frames only after an explicit recovery choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavTruncation {
    /// Complete frames present in the file.
    pub readable_frames: u64,
    /// Declared data bytes absent before physical end of file.
    pub missing_bytes: u64,
    /// Bytes of a trailing incomplete frame that recovery discards.
    pub partial_frame_bytes: u64,
}

#[derive(Debug)]
pub enum ImportError {
    Io(io::Error),
    Invalid(&'static str),
    /// Audio data is truncated or ends in a partial frame; readable frames remain.
    Truncated(WavTruncation),
    LimitExceeded(&'static str),
    AllocationFailed,
    SourceChanged,
    Cancelled,
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "could not read media file: {error}"),
            Self::Invalid(reason) => write!(f, "invalid or unsupported media: {reason}"),
            Self::Truncated(truncation) => write!(
                f,
                "wav audio data is incomplete ({} bytes missing, {} bytes of a partial frame); \
                 recover readable frames to import its {} complete frames",
                truncation.missing_bytes,
                truncation.partial_frame_bytes,
                truncation.readable_frames
            ),
            Self::LimitExceeded(reason) => write!(f, "media limit exceeded: {reason}"),
            Self::AllocationFailed => f.write_str("could not allocate media buffer"),
            Self::SourceChanged => f.write_str("source file changed while it was being read"),
            Self::Cancelled => f.write_str("media operation cancelled"),
        }
    }
}

impl std::error::Error for ImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(_)
            | Self::Truncated(_)
            | Self::LimitExceeded(_)
            | Self::AllocationFailed
            | Self::SourceChanged
            | Self::Cancelled => None,
        }
    }
}

impl From<io::Error> for ImportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Read and decode a WAV file, rejecting input above the caller's byte limit.
/// At most one sentinel byte beyond the limit is read to detect an oversized file.
/// The open handle's length and modification time must be identical before and
/// after the read, and match the bytes read, or the decode reports
/// [`ImportError::SourceChanged`]; see [`DecodedAudio::file_identity`].
pub fn decode_wav_file(
    path: impl AsRef<Path>,
    limits: Limits,
) -> Result<DecodedAudio, ImportError> {
    decode_wav_file_cancellable(path, limits, &NOT_CANCELLED)
}

/// [`decode_wav_file`] with cooperative cancellation for superseded control
/// work. `cancel` is polled before each 16 KiB read and every 65,536 decoded
/// PCM frames; once observed, the decode stops and reports
/// [`ImportError::Cancelled`]. Block-compressed (ADPCM) payloads are polled
/// before each window of whole blocks spanning at most 65,536 frames (one
/// block when a block is larger).
pub fn decode_wav_file_cancellable(
    path: impl AsRef<Path>,
    limits: Limits,
    cancel: &AtomicBool,
) -> Result<DecodedAudio, ImportError> {
    cancelled(cancel)?;
    let path = path.as_ref().to_path_buf();
    let mut file = File::open(&path)?;
    let before = file_identity::Observation::of(&file.metadata()?);
    let bytes = read_bounded(&mut file, limits.max_file_bytes, cancel)?;
    let after = file_identity::Observation::of(&file.metadata()?);
    let file_identity = file_identity::stable_identity(before, after, bytes.len())?;
    let (audio, sample_rate, frames) = wav::decode(&bytes, limits.max_frames, cancel)?;
    Ok(DecodedAudio {
        path,
        audio,
        sample_rate,
        frames,
        file_identity,
    })
}

/// Read and decode a WAV file, explicitly recovering the readable whole frames of
/// truncated or partial-frame audio data. Returns the truncation report when
/// frames were dropped so the caller can tell the user about the fidelity loss.
/// Malformed headers and block-compressed (ADPCM) truncation are still refused,
/// and the same file-identity check as [`decode_wav_file`] applies.
pub fn recover_wav_file(
    path: impl AsRef<Path>,
    limits: Limits,
) -> Result<(DecodedAudio, Option<WavTruncation>), ImportError> {
    let path = path.as_ref().to_path_buf();
    let mut file = File::open(&path)?;
    let before = file_identity::Observation::of(&file.metadata()?);
    let bytes = read_bounded(&mut file, limits.max_file_bytes, &NOT_CANCELLED)?;
    let after = file_identity::Observation::of(&file.metadata()?);
    let file_identity = file_identity::stable_identity(before, after, bytes.len())?;
    let ((audio, sample_rate, frames), truncation) =
        wav::decode_recoverable(&bytes, limits.max_frames, &NOT_CANCELLED)?;
    Ok((
        DecodedAudio {
            path,
            audio,
            sample_rate,
            frames,
            file_identity,
        },
        truncation,
    ))
}

/// Decode an in-memory RIFF/WAVE file under caller-provided byte and frame limits.
pub fn decode_wav(bytes: &[u8], limits: Limits) -> Result<Audio, ImportError> {
    if bytes.len() > limits.max_file_bytes {
        return Err(ImportError::LimitExceeded("input bytes"));
    }
    wav::decode(bytes, limits.max_frames, &NOT_CANCELLED).map(|(audio, _, _)| audio)
}

fn read_bounded(
    reader: &mut impl Read,
    max_bytes: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, ImportError> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; READ_CHUNK_BYTES];
    loop {
        cancelled(cancel)?;
        let remaining = max_bytes.saturating_sub(bytes.len());
        let read_limit = remaining.saturating_add(1).min(chunk.len());
        let count = reader.read(&mut chunk[..read_limit])?;
        if count == 0 {
            return Ok(bytes);
        }
        let new_len = bytes
            .len()
            .checked_add(count)
            .ok_or(ImportError::LimitExceeded("input bytes"))?;
        if new_len > max_bytes {
            return Err(ImportError::LimitExceeded("input bytes"));
        }
        bytes
            .try_reserve(count)
            .map_err(|_| ImportError::AllocationFailed)?;
        bytes.extend_from_slice(&chunk[..count]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits::new(4096, 32);

    fn wav(encoding: u16, channels: u16, bits: u16, samples: &[u8]) -> Vec<u8> {
        let align = channels * (bits / 8);
        let rate = 48_000_u32 * u32::from(align);
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&encoding.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&48_000_u32.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&align.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        let mut chunks = Vec::new();
        push_chunk(&mut chunks, b"fmt ", &fmt);
        push_chunk(&mut chunks, b"data", samples);
        let riff_size = (4 + chunks.len()) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&chunks);
        out
    }

    fn push_chunk(out: &mut Vec<u8>, id: &[u8; 4], payload: &[u8]) {
        out.extend_from_slice(id);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        if payload.len() & 1 == 1 {
            out.push(0);
        }
    }

    fn floats(audio: &Audio, frame_count: usize) -> Vec<hex_engine::Frame> {
        // Audio intentionally exposes no mutable buffer; use renderer output to inspect samples.
        let project = hex_engine::Project::new(
            48_000,
            vec![
                hex_engine::Track::new(
                    vec![
                        hex_engine::Clip::new(audio.clone(), 0, frame_count as u64, false)
                            .expect("test clip"),
                    ],
                    1.0,
                    false,
                )
                .expect("test track"),
            ],
        )
        .expect("test project");
        let mut renderer = hex_engine::Renderer::new(&project);
        renderer.play();
        let mut frames = vec![[0.0; 2]; frame_count];
        renderer.render(&mut frames).expect("test render");
        frames
    }

    #[test]
    fn decodes_pcm16_stereo_and_duplicates_mono() {
        let stereo = wav(1, 2, 16, &[0, 128, 0, 64, 255, 127, 0, 192]);
        let audio = decode_wav(&stereo, LIMITS).expect("valid wav");
        let frames = floats(&audio, 2);
        assert_eq!(frames[0], [-1.0, 0.5]);
        assert_eq!(frames[1], [i16::MAX as f32 / 32768.0, -0.5]);

        let mono = wav(1, 1, 16, &[0, 64]);
        assert_eq!(
            floats(&decode_wav(&mono, LIMITS).expect("valid mono"), 1),
            [[0.5, 0.5]]
        );
    }

    #[test]
    fn decodes_pcm24_pcm32_and_float32() {
        let pcm24 = wav(1, 1, 24, &[0, 0, 0x80]);
        assert_eq!(
            floats(&decode_wav(&pcm24, LIMITS).expect("valid pcm24"), 1),
            [[-1.0, -1.0]]
        );
        let pcm32 = wav(1, 1, 32, &[0, 0, 0, 0x80]);
        assert_eq!(
            floats(&decode_wav(&pcm32, LIMITS).expect("valid pcm32"), 1),
            [[-1.0, -1.0]]
        );
        let float = wav(3, 2, 32, &[0, 0, 128, 63, 0, 0, 0, 191]);
        assert_eq!(
            floats(&decode_wav(&float, LIMITS).expect("valid float"), 1),
            [[1.0, -0.5]]
        );
    }

    #[test]
    fn accepts_unknown_odd_chunk_with_padding() {
        let mut file = wav(1, 1, 16, &[0, 64]);
        let chunks = file.split_off(12);
        let mut with_junk = Vec::new();
        push_chunk(&mut with_junk, b"JUNK", &[1, 2, 3]);
        with_junk.extend_from_slice(&chunks);
        let size = (4 + with_junk.len()) as u32;
        file[4..8].copy_from_slice(&size.to_le_bytes());
        file.extend_from_slice(&with_junk);
        assert_eq!(
            floats(&decode_wav(&file, LIMITS).expect("valid padded chunk"), 1),
            [[0.5, 0.5]]
        );
    }

    #[test]
    fn preserves_path_spelling_and_reports_decoded_metadata() {
        let path = std::env::temp_dir().join(format!("hex-media-MiXeD-{}.wav", std::process::id()));
        let bytes = wav(1, 1, 16, &[0, 64]);
        std::fs::write(&path, bytes).expect("write fixture");
        let decoded = decode_wav_file(&path, LIMITS);
        std::fs::remove_file(&path).expect("remove fixture");
        let decoded = decoded.expect("valid file");
        assert_eq!(decoded.path(), path);
        assert_eq!(decoded.sample_rate(), 48_000);
        assert_eq!(decoded.frames(), 1);
        assert_eq!(decoded.channels(), 2);
    }

    #[test]
    fn rejects_malformed_truncated_and_unsupported_inputs() {
        assert!(decode_wav(b"RIFF", LIMITS).is_err());
        let valid = wav(1, 1, 16, &[0, 64]);
        assert!(decode_wav(&valid[..valid.len() - 1], LIMITS).is_err());
        assert!(decode_wav(&wav(6, 1, 16, &[0, 0]), LIMITS).is_err());
        assert!(decode_wav(&wav(1, 1, 12, &[0]), LIMITS).is_err());
        assert!(decode_wav(&wav(1, 3, 16, &[0, 0, 0, 0, 0, 0]), LIMITS).is_err());
        assert!(decode_wav(&wav(3, 1, 32, &[0, 0, 192, 127]), LIMITS).is_err());

        let mut unsupported_rate = wav(1, 1, 16, &[0, 0]);
        unsupported_rate[24..28].copy_from_slice(&7_999_u32.to_le_bytes());
        unsupported_rate[28..32].copy_from_slice(&15_998_u32.to_le_bytes());
        assert!(decode_wav(&unsupported_rate, LIMITS).is_err());

        let mut missing_padding = b"RIFF".to_vec();
        missing_padding.extend_from_slice(&15_u32.to_le_bytes());
        missing_padding.extend_from_slice(b"WAVEJUNK");
        missing_padding.extend_from_slice(&3_u32.to_le_bytes());
        missing_padding.extend_from_slice(&[1, 2, 3]);
        assert!(decode_wav(&missing_padding, LIMITS).is_err());
    }

    #[test]
    fn enforces_input_and_frame_limits() {
        let valid = wav(1, 1, 16, &[0, 64, 0, 64]);
        assert!(matches!(
            decode_wav(&valid, Limits::new(valid.len() - 1, 32)),
            Err(ImportError::LimitExceeded("input bytes"))
        ));
        assert!(matches!(
            decode_wav(&valid, Limits::new(valid.len(), 1)),
            Err(ImportError::LimitExceeded("decoded frames"))
        ));
    }
}

pub mod waveform;
