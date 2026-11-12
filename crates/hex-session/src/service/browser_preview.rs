//! Browser preview preparation on control/background threads: bounded source
//! decoding, source-rate waveform summaries for drag feedback and output-rate
//! conversion for audition.
//!
//! One [`PreviewSources`] handle is shared by every preview job of a browser.
//! It decodes each source once and shares the decoded PCM through a small
//! least-recently-used cache, keyed by path, file length and modification
//! time, so a waveform summary followed by audition of the same unchanged file
//! decodes it once. Concurrent requests for one path wait for the single
//! in-flight decode instead of starting another. The waveform never resamples:
//! its bins summarise the source at its own rate and span the converted
//! playback length.
//!
//! Every job takes a cooperative cancellation flag. Decoding polls it every
//! 16 KiB read and 65,536 PCM frames, conversion every 65,536 output frames and
//! a waiter every [`LOADING_POLL`], so a superseded job stops within that
//! bounded work and reports [`PreviewFailure::Cancelled`]. Waveform peak scanning
//! also polls at most every 65,536 frames, including for an already cached source.
//! A cancelled or failed decode caches nothing.
//!
//! Does not own audition playback, publication to the audio callback or the
//! browser's request sequencing; `hex-ui` decides which job is current.
use crate::resident_import::decode_resident_import_cancellable;
use hex_engine::Audio;
use hex_media::{
    ImportError, Limits, ResampleLimits, ResampleVersion, SourceFileIdentity,
    waveform::{Waveform, summarize_peaks_cancellable},
};
use std::{
    collections::VecDeque,
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

mod window;
pub use window::{BROWSER_PREVIEW_WINDOW_FRAMES, PreviewWindowSource};

// Browser audition is independent of canonical resident-import admission. Bound
// encoded input to 512 MiB and each decoded/converted stereo buffer to 256 MiB.
// Source and converted PCM can coexist during conversion (512 MiB total);
// resident decoding can temporarily hold encoded bytes alongside source PCM.
const BROWSER_PREVIEW_FILE_LIMIT: usize = 512 * 1024 * 1024;
const BROWSER_PREVIEW_FRAME_LIMIT: usize = 32 * 1024 * 1024;
const BROWSER_PREVIEW_BINS: usize = 4096;
/// Decoded sources retained for reuse; the oldest is evicted first.
const SOURCE_CACHE_ENTRIES: usize = 4;
/// Retained decoded PCM across all entries: one maximum-length preview source.
const SOURCE_CACHE_BYTES: usize = BROWSER_PREVIEW_FRAME_LIMIT * std::mem::size_of::<[f32; 2]>();
/// Longest a request waiting on another job's decode goes between cancel checks.
const LOADING_POLL: Duration = Duration::from_millis(10);

/// Bounded audio metadata prepared away from the UI for browser drag feedback.
#[derive(Clone, Debug)]
pub struct AudioPreview {
    pub playback_frames: u64,
    pub sample_rate: u32,
    pub waveform: Arc<Waveform>,
}

/// Why a preview job produced no result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewFailure {
    /// The caller's cancellation flag was observed; nothing was cached.
    Cancelled,
    /// A lowercase, user-presentable explanation.
    Failed(String),
}

impl PreviewFailure {
    fn from_import(error: ImportError) -> Self {
        match error {
            ImportError::Cancelled => Self::Cancelled,
            other => Self::Failed(hex_storage::text::lowercase_outside_verbatim(
                &other.to_string(),
            )),
        }
    }
}

impl fmt::Display for PreviewFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("sample preview cancelled"),
            Self::Failed(reason) => f.write_str(reason),
        }
    }
}

/// Shared, bounded decoded-source cache for one browser's preview jobs.
/// Cloning shares the cache. Its lock is held only for bookkeeping, never
/// while decoding, and is never taken by the audio callback.
#[derive(Clone, Default)]
pub struct PreviewSources {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    cache: Mutex<Cache>,
    /// Signalled whenever an in-flight decode finishes, successfully or not.
    loaded: Condvar,
}

#[derive(Default)]
struct Cache {
    /// Most recently used first.
    entries: VecDeque<Entry>,
    /// Paths with a decode in flight; at most one decode per path.
    loading: Vec<PathBuf>,
    #[cfg(test)]
    decodes: usize,
}

struct Entry {
    path: PathBuf,
    identity: SourceFileIdentity,
    audio: Audio,
}

impl Entry {
    fn bytes(&self) -> usize {
        self.audio
            .frames()
            .len()
            .saturating_mul(std::mem::size_of::<[f32; 2]>())
    }
}

impl PreviewSources {
    /// Waveform metadata for drag feedback without sample-rate conversion: the
    /// source-rate peak summary, spanning the converted playback length at
    /// `project_sample_rate`.
    pub fn prepare_waveform(
        &self,
        path: impl AsRef<Path>,
        project_sample_rate: u32,
        cancel: &AtomicBool,
    ) -> Result<AudioPreview, PreviewFailure> {
        let source = self.source(path.as_ref(), cancel)?;
        waveform_from_source(source, project_sample_rate, cancel)
    }

    /// Finite stereo PCM at the actual output rate for a browser one-shot.
    /// Supports streamable (fixed-frame) WAV files up to 512 MiB, with at most
    /// 33,554,432 frames both before and after conversion (256 MiB per stereo
    /// f32 buffer); import can stream those when a source reservation is
    /// available. Resident-only WAV encodings such as ADPCM keep the resident
    /// import caps. Other resource constraints can still refuse an import.
    /// Never truncates a source.
    pub fn prepare_playback(
        &self,
        path: impl AsRef<Path>,
        output_sample_rate: u32,
        cancel: &AtomicBool,
    ) -> Result<Audio, PreviewFailure> {
        let source = self.source(path.as_ref(), cancel)?;
        hex_media::resample_cancellable(
            &source,
            output_sample_rate,
            ResampleLimits {
                max_source_frames: BROWSER_PREVIEW_FRAME_LIMIT,
                max_output_frames: BROWSER_PREVIEW_FRAME_LIMIT,
            },
            ResampleVersion::V1,
            cancel,
        )
        .map_err(PreviewFailure::from_import)
    }

    /// Decoded source PCM, from the cache when the file is unchanged.
    fn source(&self, path: &Path, cancel: &AtomicBool) -> Result<Audio, PreviewFailure> {
        // A stat failure is not final: the decode reports the actual error.
        let identity = SourceFileIdentity::observe(path).ok().flatten();
        let mut cache = self.lock();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(PreviewFailure::Cancelled);
            }
            if let Some(audio) = identity.and_then(|identity| cache.take_hit(path, identity)) {
                return Ok(audio);
            }
            if !cache.loading.iter().any(|loading| loading == path) {
                cache.loading.push(path.to_path_buf());
                #[cfg(test)]
                {
                    cache.decodes += 1;
                }
                break;
            }
            cache = self
                .shared
                .loaded
                .wait_timeout(cache, LOADING_POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        drop(cache);
        let decoded = decode_preview_source(path, cancel);
        let mut cache = self.lock();
        cache.loading.retain(|loading| loading != path);
        if let Ok((audio, Some(identity))) = &decoded {
            cache.insert(Entry {
                path: path.to_path_buf(),
                identity: *identity,
                audio: audio.clone(),
            });
        }
        drop(cache);
        self.shared.loaded.notify_all();
        decoded.map(|(audio, _)| audio)
    }

    fn lock(&self) -> MutexGuard<'_, Cache> {
        // Bookkeeping stays consistent at every unlock, so a poisoned lock
        // (impossible without a panic elsewhere) is still usable.
        self.shared
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Post-decode waveform work shares the job's cancellation flag.
fn waveform_from_source(
    source: Audio,
    project_sample_rate: u32,
    cancel: &AtomicBool,
) -> Result<AudioPreview, PreviewFailure> {
    let source_frames = u64::try_from(source.frames().len())
        .map_err(|_| PreviewFailure::Failed("source frame count exceeds range".into()))?;
    let playback_frames = hex_project::converted_frame_count(
        source_frames,
        source.sample_rate(),
        project_sample_rate,
    )
    .map_err(|error| PreviewFailure::Failed(error.0.into()))?;
    // The same refusal audition conversion applies to its output buffer.
    let playback_len = usize::try_from(playback_frames)
        .ok()
        .filter(|frames| *frames <= BROWSER_PREVIEW_FRAME_LIMIT)
        .ok_or_else(|| PreviewFailure::from_import(ImportError::LimitExceeded("output frames")))?;
    let peaks =
        summarize_peaks_cancellable(&source, BROWSER_PREVIEW_BINS.min(playback_len), cancel)
            .map_err(PreviewFailure::from_import)?;
    let identical_channels = peaks.has_identical_channels();
    let waveform = Waveform::from_peaks(playback_len, peaks.into_bins())
        .map_err(PreviewFailure::from_import)?;
    let waveform = if identical_channels {
        waveform
            .with_identical_channels()
            .map_err(PreviewFailure::from_import)?
    } else {
        waveform
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(PreviewFailure::Cancelled);
    }
    Ok(AudioPreview {
        playback_frames,
        sample_rate: project_sample_rate,
        waveform: Arc::new(waveform),
    })
}

impl Cache {
    fn take_hit(&mut self, path: &Path, identity: SourceFileIdentity) -> Option<Audio> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.path == path && entry.identity == identity)?;
        let entry = self.entries.remove(index)?;
        let audio = entry.audio.clone();
        self.entries.push_front(entry);
        Some(audio)
    }

    /// Replace any older entry for the path, then evict least-recently-used
    /// entries beyond the count and byte budgets (possibly the new one).
    fn insert(&mut self, entry: Entry) {
        self.entries.retain(|existing| existing.path != entry.path);
        self.entries.push_front(entry);
        let mut bytes = self
            .entries
            .iter()
            .fold(0_usize, |total, entry| total.saturating_add(entry.bytes()));
        while self.entries.len() > SOURCE_CACHE_ENTRIES || bytes > SOURCE_CACHE_BYTES {
            let Some(evicted) = self.entries.pop_back() else {
                break;
            };
            bytes = bytes.saturating_sub(evicted.bytes());
        }
    }
}

/// Decode under the browser limits: streamable WAV up to the preview caps,
/// anything else under the resident import caps. Returns the decoded file's
/// identity for caching; `None` when the platform reports no mtime.
fn decode_preview_source(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<(Audio, Option<SourceFileIdentity>), PreviewFailure> {
    // A failed fixed-frame probe takes the more restrictive resident path.
    let probe = hex_media::WavFrameReader::open(
        path,
        hex_media::WavReaderLimits::new(u64::MAX, u64::MAX, 32, 4096),
    )
    .ok();
    decode_probed_source(path, probe, cancel)
}

/// Decode after the fixed-frame probe, refusing a file replaced since probing.
pub(super) fn decode_probed_source(
    path: &Path,
    probe: Option<hex_media::WavFrameReader>,
    cancel: &AtomicBool,
) -> Result<(Audio, Option<SourceFileIdentity>), PreviewFailure> {
    let decoded = if probe.is_some() {
        hex_media::decode_wav_file_cancellable(
            path,
            Limits::new(BROWSER_PREVIEW_FILE_LIMIT, BROWSER_PREVIEW_FRAME_LIMIT),
            cancel,
        )
    } else {
        decode_resident_import_cancellable(path, cancel)
    }
    .map_err(PreviewFailure::from_import)?;
    if probe
        .as_ref()
        .is_some_and(|reader| !reader.matches_file_identity(decoded.file_identity()))
    {
        return Err(PreviewFailure::Failed(
            "audio file changed while preparing preview · retry".into(),
        ));
    }
    let identity = decoded.file_identity();
    Ok((decoded.into_audio(), identity))
}

#[cfg(test)]
mod tests;
