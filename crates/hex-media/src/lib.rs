//! Bounded control-side WAV import and sample-rate conversion for validated stereo audio.
use hex_engine::Audio;
use std::{
    fmt,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

mod adpcm;
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

const READ_CHUNK_BYTES: usize = 16 * 1024;

pub use resample::{ResampleLimits, ResamplePlan, resample};
pub use resample_kernel::{ResampleKernel, ResampleKernelDataLayout};
pub use stream::{WavFrameReader, WavReaderLimits};
pub use verified_stream::{
    VERIFIED_WAV_PAGE_BYTES, VerifiedReaderDataLayout, VerifiedWavReader, VerifiedWavReaderLimits,
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

    pub fn into_audio(self) -> Audio {
        self.audio
    }
}

#[derive(Debug)]
pub enum ImportError {
    Io(io::Error),
    Invalid(&'static str),
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
            Self::LimitExceeded(reason) => write!(f, "media limit exceeded: {reason}"),
            Self::AllocationFailed => f.write_str("could not allocate media buffer"),
            Self::SourceChanged => f.write_str("source changed during verified media access"),
            Self::Cancelled => f.write_str("media verification cancelled"),
        }
    }
}

impl std::error::Error for ImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(_)
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
pub fn decode_wav_file(
    path: impl AsRef<Path>,
    limits: Limits,
) -> Result<DecodedAudio, ImportError> {
    let path = path.as_ref().to_path_buf();
    let mut file = File::open(&path)?;
    let bytes = read_bounded(&mut file, limits.max_file_bytes)?;
    let (audio, sample_rate, frames) = decode_wav_inner(&bytes, limits.max_frames)?;
    Ok(DecodedAudio {
        path,
        audio,
        sample_rate,
        frames,
    })
}

/// Decode an in-memory RIFF/WAVE file under caller-provided byte and frame limits.
pub fn decode_wav(bytes: &[u8], limits: Limits) -> Result<Audio, ImportError> {
    if bytes.len() > limits.max_file_bytes {
        return Err(ImportError::LimitExceeded("input bytes"));
    }
    decode_wav_inner(bytes, limits.max_frames).map(|(audio, _, _)| audio)
}

fn decode_wav_inner(bytes: &[u8], max_frames: usize) -> Result<(Audio, u32, usize), ImportError> {
    wav::decode(bytes, max_frames)
}

fn read_bounded(reader: &mut impl Read, max_bytes: usize) -> Result<Vec<u8>, ImportError> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; READ_CHUNK_BYTES];
    loop {
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
