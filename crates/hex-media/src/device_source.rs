//! Worker-only decode into existing sealed native source custody, before Audio sharing.
use crate::{ImportError, Limits, adpcm, wav};
use hex_engine::{AudioStorageError, Frame, OwnedAudioSource};
use std::{
    fmt,
    io::Cursor,
    sync::atomic::{AtomicBool, Ordering},
};

/// Existing import ceilings plus explicit RIFF work and temporary buffer bounds.
/// Working bytes include encoded input length, actual decoded Vec capacity and
/// ADPCM scratch capacity. Root/owner headers and coexistence admission remain
/// the host ledger's responsibility; this is not a graph-publication permit.
#[derive(Clone, Copy, Debug)]
pub struct DeviceSourceDecodeLimits {
    pub media: Limits,
    pub max_chunk_headers: usize,
    /// Includes decoded ADPCM block padding, even when a fact chunk trims output.
    pub max_decode_work_frames: usize,
    pub max_working_buffer_bytes: usize,
}
#[derive(Debug)]
pub enum DeviceSourceImportError {
    Media(ImportError),
    Storage(AudioStorageError),
}
impl From<ImportError> for DeviceSourceImportError {
    fn from(error: ImportError) -> Self {
        Self::Media(error)
    }
}
impl fmt::Display for DeviceSourceImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Media(error) => error.fmt(f),
            Self::Storage(error) => write!(f, "native source custody refused: {error:?}"),
        }
    }
}
impl std::error::Error for DeviceSourceImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Media(error) => Some(error),
            Self::Storage(_) => None,
        }
    }
}
fn check(cancel: &AtomicBool) -> Result<(), ImportError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ImportError::Cancelled)
    } else {
        Ok(())
    }
}
fn bytes_for<T>(count: usize) -> Result<usize, ImportError> {
    count
        .checked_mul(std::mem::size_of::<T>())
        .ok_or(ImportError::LimitExceeded("native decode working buffers"))
}
fn within(bytes: usize, maximum: usize) -> Result<(), ImportError> {
    if bytes > maximum {
        Err(ImportError::LimitExceeded("native decode working buffers"))
    } else {
        Ok(())
    }
}
fn reserve<T>(count: usize) -> Result<Vec<T>, ImportError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| ImportError::AllocationFailed)?;
    Ok(values)
}
enum Output {
    Mono(Vec<f32>),
    Stereo(Vec<Frame>),
}
impl Output {
    fn len(&self) -> usize {
        match self {
            Self::Mono(v) => v.len(),
            Self::Stereo(v) => v.len(),
        }
    }
    fn capacity_bytes(&self) -> Result<usize, ImportError> {
        match self {
            Self::Mono(v) => bytes_for::<f32>(v.capacity()),
            Self::Stereo(v) => bytes_for::<Frame>(v.capacity()),
        }
    }
    fn push(&mut self, frame: Frame) {
        match self {
            Self::Mono(v) => v.push(frame[0]),
            Self::Stereo(v) => v.push(frame),
        }
    }
}

/// Decode normalized original-rate Mono/Stereo directly into the existing native
/// constructor family. No raw Audio alias, PCM cache, resampling or registry gate.
/// Input is immutable; error/cancel drops only unpublished worker-owned buffers.
/// Cancellation is checked per RIFF header, every256 fixed frames and before/after
/// each ADPCM block (WAV format bounds each block); final native validation is
/// bounded by media.max_frames and remains entirely on this control worker.
pub fn decode_device_source(
    bytes: &[u8],
    limits: DeviceSourceDecodeLimits,
    cancel: &AtomicBool,
) -> Result<OwnedAudioSource, DeviceSourceImportError> {
    check(cancel)?;
    if bytes.len() > limits.media.max_file_bytes {
        return Err(ImportError::LimitExceeded("input bytes").into());
    }
    within(bytes.len(), limits.max_working_buffer_bytes)?;
    let layout = wav::scan_checked(
        &mut Cursor::new(bytes),
        bytes.len() as u64,
        limits.max_chunk_headers,
        || check(cancel),
    )?;
    let count =
        usize::try_from(layout.frames).map_err(|_| ImportError::LimitExceeded("decoded frames"))?;
    if count > limits.media.max_frames {
        return Err(ImportError::LimitExceeded("decoded frames").into());
    }
    let work = if layout.format.samples_per_block == 0 {
        count
    } else {
        usize::try_from(layout.data_bytes)
            .ok()
            .and_then(|n| n.checked_div(layout.format.block_align))
            .and_then(|n| n.checked_mul(layout.format.samples_per_block))
            .ok_or(ImportError::LimitExceeded("native decoded work frames"))?
    };
    if work > limits.max_decode_work_frames {
        return Err(ImportError::LimitExceeded("native decoded work frames").into());
    }
    let scratch_count = layout.format.samples_per_block;
    let output_bytes = if layout.format.channels == 1 {
        bytes_for::<f32>(count)?
    } else {
        bytes_for::<Frame>(count)?
    };
    let requested = bytes
        .len()
        .checked_add(output_bytes)
        .and_then(|n| n.checked_add(bytes_for::<Frame>(scratch_count).ok()?))
        .ok_or(ImportError::LimitExceeded("native decode working buffers"))?;
    within(requested, limits.max_working_buffer_bytes)?;
    check(cancel)?;
    let mut output = if layout.format.channels == 1 {
        Output::Mono(reserve(count)?)
    } else {
        Output::Stereo(reserve(count)?)
    };
    // Check actual allocator-returned capacities before requesting the next buffer.
    let base = bytes
        .len()
        .checked_add(output.capacity_bytes()?)
        .ok_or(ImportError::LimitExceeded("native decode working buffers"))?;
    within(
        base.checked_add(bytes_for::<Frame>(scratch_count)?)
            .ok_or(ImportError::LimitExceeded("native decode working buffers"))?,
        limits.max_working_buffer_bytes,
    )?;
    let mut scratch = reserve::<Frame>(scratch_count)?;
    within(
        base.checked_add(bytes_for::<Frame>(scratch.capacity())?)
            .ok_or(ImportError::LimitExceeded("native decode working buffers"))?,
        limits.max_working_buffer_bytes,
    )?;
    scratch.resize(scratch_count, [0.0; 2]);
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
    if scratch_count == 0 {
        for (index, frame) in data.chunks_exact(layout.format.block_align).enumerate() {
            if index % 256 == 0 {
                check(cancel)?;
            }
            output.push(wav::decode_frame(frame, layout.format)?);
        }
    } else {
        adpcm::decode_into(data, layout.format, count, &mut scratch, |frames| {
            check(cancel)?;
            for frame in frames {
                output.push(*frame);
            }
            Ok(())
        })?;
    }
    if output.len() != count {
        return Err(ImportError::Invalid("inconsistent native decoded length").into());
    }
    check(cancel)?;
    let source = match output {
        Output::Mono(samples) => {
            OwnedAudioSource::try_resident_mono(layout.format.sample_rate, samples)
                .map_err(|refusal| DeviceSourceImportError::Storage(refusal.reason))?
        }
        Output::Stereo(frames) => OwnedAudioSource::try_resident(layout.format.sample_rate, frames)
            .map_err(|refusal| DeviceSourceImportError::Storage(refusal.reason))?,
    };
    check(cancel)?;
    Ok(source)
}
