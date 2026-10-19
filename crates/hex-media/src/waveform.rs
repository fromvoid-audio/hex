//! Control-side peak summaries. Compute once after import, cache for visible UI.
use crate::{DeviceSourceImportError, ImportError};
use hex_engine::native_device_resources::OriginalSourceLease;
use hex_engine::{Audio, OwnedAudioSource};

pub const MAX_BINS: usize = 65_536;
const PYRAMID_LEVEL_CAPACITY: usize = 16;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub min: f32,
    pub max: f32,
}
/// A bin summarizes a nonempty, disjoint interval of the original PCM. The full
/// source is covered exactly once, even when its length is not divisible by bins.
#[derive(Debug)]
pub struct Waveform {
    pub source_frames: usize,
    pub bins: Vec<[Peak; 2]>,
    // Pairwise extrema above the leaf bins; built once on the control worker.
    levels: Vec<Vec<[Peak; 2]>>,
    samples: Option<SampleBacking>,
}

/// Canonical shared summary with its fresh outer-constructor receipt retained
/// beside the actual owner. No raw Arc adoption or mutable shared access.
pub type SharedWaveform = hex_engine::SharedMetadata<Waveform>;

mod read;
pub use read::{WaveformRead, read_shared};
mod native_original;
pub use native_original::{
    NativeOriginalWaveformLimits, native_original_metadata_bytes, summarize_native_audio_on_worker,
    summarize_native_source_on_worker,
};

#[cfg(test)]
mod shared_storage_tests;

/// Actual outer field/body, pyramid capacities and retained PCM child. Every
/// physical field is deduplicated in the existing ledger, even across exported
/// aliases. A header-only prior visit cannot hide an unvisited nested child.
/// This is selected storage, not whole-root custody/peak/retirement admission.
pub fn borrow_shared_storage_on_control<'a>(
    owner: &'a SharedWaveform,
    ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
) -> Result<(), hex_engine::AudioStorageError> {
    ledger.borrow_sampler_waveform_fields_on_control(&sampler_fields_on_control(owner))
}

/// Internal audited actual owner projection; neither an admission certificate
/// nor a source clone. Used by the same worker/family inventory consumers.
/// ```compile_fail
/// fn escape(owner: &hex_media::waveform::SharedWaveform)
///     -> hex_engine::sampler_host_fields::WaveformFields<'static> {
///     hex_media::waveform::sampler_fields_on_control(owner)
/// }
/// ```
pub fn sampler_fields_on_control(
    owner: &SharedWaveform,
) -> hex_engine::sampler_host_fields::WaveformFields<'_> {
    hex_engine::sampler_host_fields::WaveformFields {
        owner: hex_engine::sampler_host_fields::SharedMetadataRef::of(owner),
        payload: owner.as_ref(),
    }
}
// Exhaustive leaf declaration: adding an owning Peak field requires review of
// this finite schema; the Engine physical Vec view certifies footprint only.
const _: fn(Peak) = |Peak { min: _, max: _ }| {};
impl hex_engine::sampler_host_fields::WaveformPayload for Waveform {
    fn body(&self) -> hex_engine::sampler_host_fields::BodyRef<'_> {
        hex_engine::sampler_host_fields::BodyRef::of(self)
    }
    fn bins(&self) -> hex_engine::sampler_host_fields::VecRef<'_> {
        let Self {
            source_frames: _,
            bins,
            levels: _,
            samples: _,
        } = self;
        hex_engine::sampler_host_fields::VecRef::of(bins)
    }
    fn levels(&self) -> hex_engine::sampler_host_fields::WaveformLevelsRef<'_> {
        let Self {
            source_frames: _,
            bins: _,
            levels,
            samples: _,
        } = self;
        hex_engine::sampler_host_fields::WaveformLevelsRef::of(levels)
    }
    fn samples(&self) -> hex_engine::sampler_host_fields::WaveformSamples<'_> {
        use hex_engine::sampler_host_fields::WaveformSamples;
        let Self {
            source_frames: _,
            bins: _,
            levels: _,
            samples,
        } = self;
        match samples {
            None => WaveformSamples::None,
            Some(SampleBacking::Audio(audio)) => WaveformSamples::Audio(audio),
            Some(SampleBacking::Original(source)) => WaveformSamples::Original(source),
            Some(SampleBacking::NativeOriginal(source)) => WaveformSamples::NativeOriginal(source),
        }
    }
}

enum SampleBacking {
    Audio(Audio),
    Original(OriginalSourceLease),
    NativeOriginal(OwnedAudioSource),
}
impl std::fmt::Debug for SampleBacking {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Audio(source) => f.debug_tuple("Audio").field(source).finish(),
            Self::Original(source) => f.debug_tuple("Original").field(source).finish(),
            Self::NativeOriginal(source) => f
                .debug_struct("NativeOriginal")
                .field("sample_rate", &source.sample_rate())
                .field("frames", &source.frames())
                .finish(),
        }
    }
}
#[derive(Clone, Copy)]
enum BorrowedSamples<'a> {
    Mono(&'a [f32]),
    Stereo(&'a [[f32; 2]]),
}
impl BorrowedSamples<'_> {
    fn len(self) -> usize {
        match self {
            Self::Mono(v) => v.len(),
            Self::Stereo(v) => v.len(),
        }
    }
    fn frame(self, index: usize) -> Option<[f32; 2]> {
        match self {
            Self::Mono(v) => v.get(index).map(|v| [*v; 2]),
            Self::Stereo(v) => v.get(index).copied(),
        }
    }
}
impl SampleBacking {
    fn borrow(&self) -> Option<BorrowedSamples<'_>> {
        match self {
            Self::Audio(audio) => Some(BorrowedSamples::Stereo(audio.frames())),
            Self::Original(source) => borrow_original(source).ok(),
            Self::NativeOriginal(source) => borrow_native(source).ok(),
        }
    }
}
fn borrow_native(
    source: &OwnedAudioSource,
) -> Result<BorrowedSamples<'_>, DeviceSourceImportError> {
    let view = source.view();
    if let Some(mono) = view.resident_mono_samples() {
        Ok(BorrowedSamples::Mono(mono))
    } else if let Some(stereo) = view.resident_frames() {
        Ok(BorrowedSamples::Stereo(stereo))
    } else {
        Err(ImportError::Invalid("waveform requires resident original PCM").into())
    }
}
fn borrow_original(
    source: &OriginalSourceLease,
) -> Result<BorrowedSamples<'_>, DeviceSourceImportError> {
    let view = source.view().map_err(DeviceSourceImportError::Storage)?;
    if let Some(mono) = view.resident_mono_samples() {
        Ok(BorrowedSamples::Mono(mono))
    } else if let Some(stereo) = view.resident_frames() {
        Ok(BorrowedSamples::Stereo(stereo))
    } else {
        Err(ImportError::Invalid("waveform requires resident original PCM").into())
    }
}

impl Waveform {
    /// Exact retained PCM identity, for control-side summary reuse across revisions.
    pub fn shares_audio_with(&self, audio: &Audio) -> bool {
        match &self.samples {
            Some(SampleBacking::Audio(samples)) => samples.shares_pcm_with(audio),
            Some(SampleBacking::NativeOriginal(samples)) => {
                samples.sample_rate() == audio.sample_rate()
                    && samples
                        .view()
                        .resident_frames()
                        .is_some_and(|frames| std::ptr::eq(frames, audio.frames()))
            }
            _ => false,
        }
    }
    pub fn shares_original_with(&self, source: &OriginalSourceLease) -> bool {
        matches!(&self.samples, Some(SampleBacking::Original(samples)) if samples.shares_source_with(source))
    }
    /// Borrow the actual retained field for control-side source-plan/union
    /// inventory. This neither clones an owner nor grants publication rights.
    pub fn original_source(&self) -> Option<&OriginalSourceLease> {
        match &self.samples {
            Some(SampleBacking::Original(source)) => Some(source),
            _ => None,
        }
    }
    /// Borrow the actual stereo backing field for control-side union inventory.
    /// Audio native lineage is retained; this does not create an owning alias.
    pub fn audio_source(&self) -> Option<&Audio> {
        match &self.samples {
            Some(SampleBacking::Audio(audio)) => Some(audio),
            _ => None,
        }
    }
    /// Actual retained field for control-side inventory only. Scoped renderer
    /// reads expose no source. This borrow grants no assignment/publication right.
    pub fn native_original_source(&self) -> Option<&OwnedAudioSource> {
        match &self.samples {
            Some(SampleBacking::NativeOriginal(source)) => Some(source),
            _ => None,
        }
    }

    /// Construct immutable cached peaks without resident PCM (e.g. fixtures).
    pub fn from_peaks(source_frames: usize, bins: Vec<[Peak; 2]>) -> Result<Self, ImportError> {
        if source_frames == 0
            || bins.is_empty()
            || bins.len() > MAX_BINS
            || bins.len() > source_frames
        {
            return Err(ImportError::LimitExceeded("invalid waveform extent"));
        }
        if bins
            .iter()
            .flatten()
            .any(|p| !p.min.is_finite() || !p.max.is_finite() || p.min > p.max)
        {
            return Err(ImportError::LimitExceeded("invalid waveform peaks"));
        }
        let mut levels = Vec::new();
        levels
            .try_reserve_exact(PYRAMID_LEVEL_CAPACITY)
            .map_err(|_| ImportError::AllocationFailed)?;
        let mut previous = bins.as_slice();
        while previous.len() > 1 {
            let mut next = Vec::new();
            next.try_reserve_exact(previous.len().div_ceil(2))
                .map_err(|_| ImportError::AllocationFailed)?;
            for pair in previous.chunks(2) {
                let mut peak = pair[0];
                if let Some(other) = pair.get(1) {
                    merge(&mut peak, *other);
                }
                next.push(peak);
            }
            levels.push(next);
            previous = levels.last().map(Vec::as_slice).unwrap_or(&[]);
        }
        Ok(Self {
            source_frames,
            bins,
            levels,
            samples: None,
        })
    }

    /// Data bins retained by all resolutions, for control-side resident accounting.
    pub fn cached_bin_count(&self) -> usize {
        self.levels.iter().fold(self.bins.len(), |count, level| {
            count.saturating_add(level.len())
        })
    }

    /// Signed channel extrema for a half-open source range, with no allocation.
    /// Deep zoom reads at most 64 shared PCM frames; wider queries use cached
    /// pairwise extrema and conservatively include intersecting boundary bins.
    pub fn peaks_for_frames(&self, start: usize, end: usize) -> Option<[Peak; 2]> {
        let start = start.min(self.source_frames);
        let end = end.min(self.source_frames);
        if start >= end || self.bins.is_empty() || self.bins.len() > self.source_frames {
            return None;
        }
        if end - start <= 64 {
            if let Some(backing) = &self.samples {
                let frames = backing.borrow()?;
                if end > frames.len() {
                    return None;
                }
                let first = frames.frame(start)?;
                let mut peaks = first.map(|sample| Peak {
                    min: sample,
                    max: sample,
                });
                for index in start + 1..end {
                    let frame = frames.frame(index)?;
                    merge(
                        &mut peaks,
                        frame.map(|sample| Peak {
                            min: sample,
                            max: sample,
                        }),
                    );
                }
                return Some(peaks);
            }
        }
        let count = self.bins.len();
        let width = self.source_frames / count;
        let extra = self.source_frames % count;
        let wide_frames = (width + 1).saturating_mul(extra);
        let bin_at = |frame: usize| {
            if frame < wide_frames {
                frame / (width + 1)
            } else {
                extra + (frame - wide_frames) / width
            }
        };
        let mut left = bin_at(start);
        let mut right = bin_at(end - 1) + 1;
        let mut level = 0;
        let mut result = None;
        while left < right {
            let nodes = if level == 0 {
                self.bins.as_slice()
            } else {
                self.levels.get(level - 1)?.as_slice()
            };
            if left % 2 == 1 {
                include(&mut result, *nodes.get(left)?);
                left += 1;
            }
            if right % 2 == 1 {
                right -= 1;
                include(&mut result, *nodes.get(right)?);
            }
            left /= 2;
            right /= 2;
            level += 1;
        }
        result
    }
}
fn merge(target: &mut [Peak; 2], other: [Peak; 2]) {
    for (target, other) in target.iter_mut().zip(other) {
        target.min = target.min.min(other.min);
        target.max = target.max.max(other.max);
    }
}
fn include(result: &mut Option<[Peak; 2]>, peaks: [Peak; 2]) {
    if let Some(current) = result {
        merge(current, peaks);
    } else {
        *result = Some(peaks);
    }
}

pub fn summarize(audio: &Audio, requested_bins: usize) -> Result<Waveform, ImportError> {
    let mut waveform = summarize_borrowed(
        BorrowedSamples::Stereo(audio.frames()),
        requested_bins,
        || Ok(()),
    )?;
    waveform.samples = Some(SampleBacking::Audio(audio.clone()));
    Ok(waveform)
}

/// Worker-only summary retaining the actual closed original source for deep
/// zoom. Share the finished immutable summary with SharedWaveform; no infallible
/// Waveform clone can create an unreserved native source handle.
pub fn summarize_original(
    source: &OriginalSourceLease,
    requested_bins: usize,
    max_scan_frames: usize,
    max_source_metadata_bytes: usize,
    cancel: &dyn hex_engine::CancellationQuery,
) -> Result<Waveform, DeviceSourceImportError> {
    let check = || {
        if cancel.is_cancelled() {
            Err(ImportError::Cancelled)
        } else {
            Ok(())
        }
    };
    check()?;
    if max_source_metadata_bytes < std::mem::size_of::<OriginalSourceLease>() {
        return Err(
            hex_engine::native_device_resources::ResourcePreparationError::MetadataCapacity.into(),
        );
    }
    let frames = borrow_original(source)?;
    if frames.len() > max_scan_frames {
        return Err(ImportError::LimitExceeded("waveform source scan frames").into());
    }
    let mut waveform = summarize_borrowed(frames, requested_bins, check)?;
    check()?;
    waveform.samples = Some(SampleBacking::Original(
        source.try_clone_on_control(max_source_metadata_bytes)?,
    ));
    Ok(waveform)
}

fn summarize_borrowed(
    frames: BorrowedSamples<'_>,
    requested_bins: usize,
    mut check: impl FnMut() -> Result<(), ImportError>,
) -> Result<Waveform, ImportError> {
    check()?;
    if requested_bins == 0 || requested_bins > MAX_BINS {
        return Err(ImportError::LimitExceeded("waveform bins must be 1..65536"));
    }
    if frames.len() == 0 {
        return Err(ImportError::Invalid("waveform source is empty"));
    }
    let count = requested_bins.min(frames.len());
    let mut bins = Vec::new();
    bins.try_reserve_exact(count)
        .map_err(|_| ImportError::AllocationFailed)?;
    // Quotient/remainder avoids multiplication overflow for long resident sources.
    let width = frames.len() / count;
    let extra = frames.len() % count;
    let mut start = 0;
    for bin in 0..count {
        if start % 256 == 0 {
            check()?;
        }
        let end = start + width + usize::from(bin < extra);
        let first = frames
            .frame(start)
            .ok_or(ImportError::Invalid("waveform source is incomplete"))?;
        let mut peaks = first.map(|sample| Peak {
            min: sample,
            max: sample,
        });
        for index in start + 1..end {
            if index % 256 == 0 {
                check()?;
            }
            let frame = frames
                .frame(index)
                .ok_or(ImportError::Invalid("waveform source is incomplete"))?;
            for (peak, sample) in peaks.iter_mut().zip(frame) {
                peak.min = peak.min.min(sample);
                peak.max = peak.max.max(sample);
            }
        }
        bins.push(peaks);
        start = end;
    }
    check()?;
    let waveform = Waveform::from_peaks(frames.len(), bins)?;
    check()?;
    Ok(waveform)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_original_summary_does_not_return_a_partial_cache() {
        let samples = vec![0.25; 1025];
        let mut checks = 0;
        let result = summarize_borrowed(BorrowedSamples::Mono(&samples), 3, || {
            checks += 1;
            if checks == 4 {
                Err(ImportError::Cancelled)
            } else {
                Ok(())
            }
        });
        assert!(matches!(result, Err(ImportError::Cancelled)));
        assert_eq!(checks, 4);
        assert_eq!(samples, [0.25; 1025]);
    }
    #[test]
    fn nondivisible_source_keeps_every_transient_and_stereo_extrema() {
        let audio = Audio::new(
            48_000,
            vec![
                [0.5, -0.5],
                [-0.8, 0.2],
                [0.0, 0.9],
                [2.0, -2.0],
                [-0.2, 0.1],
            ],
        )
        .unwrap();
        let waveform = summarize(&audio, 2).unwrap();
        assert_eq!(waveform.source_frames, 5);
        assert_eq!(
            waveform.bins,
            vec![
                [
                    Peak {
                        min: -0.8,
                        max: 0.5
                    },
                    Peak {
                        min: -0.5,
                        max: 0.9
                    }
                ],
                [
                    Peak {
                        min: -0.2,
                        max: 2.0
                    },
                    Peak {
                        min: -2.0,
                        max: 0.1
                    }
                ],
            ]
        );
    }
    #[test]
    fn tiny_sources_and_limits_are_explicit() {
        let audio = Audio::new(48_000, vec![[0.25, -0.25]]).unwrap();
        assert_eq!(summarize(&audio, 1024).unwrap().bins.len(), 1);
        assert!(summarize(&audio, 0).is_err());
        assert!(summarize(&audio, MAX_BINS + 1).is_err());
    }
    #[test]
    fn cached_range_tree_matches_direct_signed_extrema_for_odd_leaf_counts() {
        let bins: Vec<_> = (0..97)
            .map(|i| {
                let x = ((i * 37) % 97) as f32 / 97.0;
                [
                    Peak {
                        min: -x,
                        max: x / 2.0,
                    },
                    Peak {
                        min: -x / 3.0,
                        max: x,
                    },
                ]
            })
            .collect();
        let waveform = Waveform::from_peaks(97, bins.clone()).expect("cache");
        for start in 0..97 {
            for end in start + 1..=97 {
                let mut expected = bins[start];
                for bin in &bins[start + 1..end] {
                    merge(&mut expected, *bin);
                }
                assert_eq!(waveform.peaks_for_frames(start, end), Some(expected));
            }
        }
        assert!(waveform.peaks_for_frames(97, usize::MAX).is_none());
        assert!(waveform.cached_bin_count() <= 2 * 97 + 16);
    }

    #[test]
    fn deep_zoom_reads_real_samples_without_bleeding_neighboring_transients() {
        let mut frames = vec![[0.0; 2]; 257];
        frames[10] = [-1.0, 0.75];
        let audio = Audio::new(48_000, frames).expect("audio");
        let waveform = summarize(&audio, 2).expect("summary");
        assert_eq!(
            waveform.peaks_for_frames(10, 11),
            Some([
                Peak {
                    min: -1.0,
                    max: -1.0
                },
                Peak {
                    min: 0.75,
                    max: 0.75
                }
            ])
        );
        assert_eq!(
            waveform.peaks_for_frames(11, 12),
            Some([Peak { min: 0.0, max: 0.0 }; 2])
        );
        let full = waveform.peaks_for_frames(0, 257).expect("overview");
        assert_eq!(full[0].min, -1.0);
        assert_eq!(full[1].max, 0.75);
    }

    #[test]
    fn peak_fixture_validation_refuses_invalid_numbers_and_extents() {
        assert!(Waveform::from_peaks(0, Vec::new()).is_err());
        assert!(
            Waveform::from_peaks(
                1,
                vec![
                    [Peak {
                        min: f32::NAN,
                        max: 1.0
                    }; 2]
                ]
            )
            .is_err()
        );
        assert!(
            Waveform::from_peaks(
                1,
                vec![
                    [Peak {
                        min: 1.0,
                        max: -1.0
                    }; 2]
                ]
            )
            .is_err()
        );
    }
}
