//! Bounded, control-owned display summaries of prepared audio.
use super::budget::{Budget, Reservation};
use crate::{Frame, Invalid, PreparedAudioPeak};
use hex_project::{AudioClipEdit, AudioViewOccurrence, BeatTime, TempoMap, audio_projection_frame};
use std::sync::{Arc, OnceLock};
mod directed;
pub use directed::{PreparedDirectedEventRead, PreparedDirectedEventSpan};

const EVENT_SOURCE_BINS: usize = 1_024;
const EVENT_SOURCE_PEAK_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn event_source_budget() -> &'static Arc<Budget> {
    static BUDGET: OnceLock<Arc<Budget>> = OnceLock::new();
    BUDGET.get_or_init(|| Arc::new(Budget::new(EVENT_SOURCE_PEAK_BYTES)))
}

/// Peak summaries in the independently prepared event source clocks.
/// Group children retain their own processing. Marker forks share the
/// original musical clock and have a distinct pre-event waveform.
#[derive(Debug)]
pub struct PreparedEventSources {
    sources: Vec<(u64, Arc<PreparedEventSourcePeaks>)>,
    _reservation: Reservation,
}

impl PreparedEventSources {
    pub(super) fn admit(count: usize, budget: &Arc<Budget>) -> Option<Self> {
        let storage = count
            .checked_mul(std::mem::size_of::<(u64, Arc<PreparedEventSourcePeaks>)>())?
            .checked_add(std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>())?;
        let reservation = budget.reserve(storage).ok()?;
        let mut sources = Vec::new();
        sources.try_reserve_exact(count).ok()?;
        Some(Self {
            sources,
            _reservation: reservation,
        })
    }

    pub(super) fn push(&mut self, id: u64, peaks: Arc<PreparedEventSourcePeaks>) {
        self.sources.push((id, peaks));
    }

    pub(super) fn finish(mut self) -> Arc<Self> {
        self.sources.sort_unstable_by_key(|(id, _)| *id);
        Arc::new(self)
    }

    /// Entries are ordered by retained source ID.
    pub fn sources(&self) -> &[(u64, Arc<PreparedEventSourcePeaks>)] {
        &self.sources
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.sources.binary_search_by_key(&id, |(id, _)| *id).ok()
    }
}

/// The neutral source of one materialized forward musical occurrence.
#[derive(Debug)]
pub struct PreparedEventOccurrence {
    pub placement: AudioViewOccurrence,
    pub source: Arc<PreparedEventSourcePeaks>,
}

/// Complete, budgeted source reads for a finite musical placement. The
/// entries are in forward root order, using virtual base frames for directed
/// content. They own summaries and clocks, without
/// retaining the source or composed PCM for each occurrence.
#[derive(Debug)]
pub struct PreparedEventOccurrences {
    reads: Vec<PreparedEventOccurrence>,
    pub(super) directed: Option<PreparedDirectedEventRead>,
    _reservation: Reservation,
}

impl PreparedEventOccurrences {
    pub(super) fn admit(count: u64, budget: &Arc<Budget>) -> Option<Self> {
        let count = usize::try_from(count).ok()?;
        let storage = count
            .checked_mul(std::mem::size_of::<PreparedEventOccurrence>())?
            .checked_add(std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>())?;
        let reservation = budget.reserve(storage).ok()?;
        let mut reads = Vec::new();
        reads.try_reserve_exact(count).ok()?;
        Some(Self {
            reads,
            directed: None,
            _reservation: reservation,
        })
    }

    pub(super) fn push(
        &mut self,
        placement: AudioViewOccurrence,
        source: Arc<PreparedEventSourcePeaks>,
    ) {
        // Playback's admitted occurrence count bounds the copy loop.
        self.reads
            .push(PreparedEventOccurrence { placement, source });
    }

    pub fn reads(&self) -> &[PreparedEventOccurrence] {
        &self.reads
    }

    pub fn directed(&self) -> Option<&PreparedDirectedEventRead> {
        self.directed.as_ref()
    }
}

/// Stereo extrema of the transformed source before event composition, event
/// gain, and any container envelope. Frames are in the processing root clock.
#[derive(Debug)]
pub struct PreparedEventSourcePeaks {
    frames: usize,
    bins: Vec<PreparedAudioPeak>,
    musical_clock: Option<PreparedMusicalEventSourceClock>,
    _reservation: Reservation,
}

/// Clock used by the warped PCM before event composition. The source may
/// retain a different origin from the destination after a signed event move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedMusicalEventSourceClock {
    tempo: Arc<TempoMap>,
    origin: i128,
    first_frame: i128,
    length: BeatTime,
}

impl PreparedMusicalEventSourceClock {
    pub(super) fn shared_tempo(&self) -> Arc<TempoMap> {
        Arc::clone(&self.tempo)
    }

    /// Affine composition can use a sample-for-sample read at the retained
    /// zero. Exact constant-tempo translations may share the cached clock.
    pub(super) fn aligned_destination_zero(
        &self,
        destination: &AudioClipEdit,
        tempo: &TempoMap,
        zero: u64,
    ) -> Result<Option<u64>, Invalid> {
        let destination_tempo = destination.processing_tempo(tempo);
        if self.tempo.as_ref() != destination_tempo {
            return Ok(None);
        }
        let origin = destination.processing_origin()?;
        let cached_destination = self
            .origin
            .checked_sub(i128::from(zero))
            .ok_or(Invalid("audio event source origin overflow"))?;
        let aligned = origin == cached_destination
            || (super::translation_free_origin(destination_tempo, origin)?
                && super::translation_free_origin(destination_tempo, cached_destination)?);
        Ok(aligned
            .then(|| destination.root_frame_at(zero, tempo))
            .transpose()?)
    }

    pub(super) fn new(
        tempo: Arc<TempoMap>,
        origin: i128,
        first_frame: i128,
        length: BeatTime,
    ) -> Self {
        Self {
            tempo,
            origin,
            first_frame,
            length,
        }
    }

    /// Resolve a source beat boundary to a peak-relative PCM frame, including
    /// the exclusive end. Uses the same signed projection as event playback.
    pub fn frame_at(&self, position: BeatTime) -> Result<u64, Invalid> {
        if position > self.length {
            return Err(Invalid(
                "audio event source position exceeds prepared clock",
            ));
        }
        let absolute = self
            .origin
            .checked_add(i128::from(position.raw()))
            .ok_or(Invalid("audio event source beat overflow"))?;
        let frame =
            audio_projection_frame(&self.tempo, absolute).map_err(|error| Invalid(error.0))?;
        u64::try_from(frame - self.first_frame)
            .map_err(|_| Invalid("audio event source frame overflow"))
    }
}

impl PreparedEventSourcePeaks {
    pub(super) fn capture(
        frames: &[Frame],
        musical_clock: Option<PreparedMusicalEventSourceClock>,
        budget: &Arc<Budget>,
        cancelled: impl FnMut() -> bool,
    ) -> Option<Arc<Self>> {
        Self::capture_with(
            frames.len(),
            musical_clock,
            budget,
            |index| frames.get(index).copied(),
            cancelled,
        )
    }

    /// Summarize the existing read law directly, without retaining another PCM buffer.
    pub(super) fn capture_with(
        frames: usize,
        musical_clock: Option<PreparedMusicalEventSourceClock>,
        budget: &Arc<Budget>,
        mut sample: impl FnMut(usize) -> Option<Frame>,
        mut cancelled: impl FnMut() -> bool,
    ) -> Option<Arc<Self>> {
        if frames == 0 || frames as u64 > hex_project::MAX_AUDIO_DOMAIN_FRAMES || cancelled() {
            return None;
        }
        let count = EVENT_SOURCE_BINS.min(frames);
        let reservation = budget.reserve(Self::storage_bytes(count)).ok()?;
        let mut bins = PeakBins::new(frames, count)?;
        for index in 0..frames {
            if index % 4_096 == 0 && cancelled() {
                return None;
            }
            bins.observe(index, sample(index)?);
        }
        Some(Arc::new(Self {
            frames,
            bins: bins.into_peaks(),
            musical_clock,
            _reservation: reservation,
        }))
    }

    fn storage_bytes(count: usize) -> usize {
        count * std::mem::size_of::<PreparedAudioPeak>()
            + std::mem::size_of::<Self>()
            + 2 * std::mem::size_of::<usize>()
    }

    pub fn source_frames(&self) -> usize {
        self.frames
    }

    /// Frame-clock sources have no beat projection.
    pub fn musical_clock(&self) -> Option<&PreparedMusicalEventSourceClock> {
        self.musical_clock.as_ref()
    }

    /// Exact extrema per bin. The first `source_frames() % bins().len()` bins
    /// each cover one more frame than the others.
    pub fn bins(&self) -> &[PreparedAudioPeak] {
        &self.bins
    }
}

pub(super) struct PeakBins {
    peaks: Vec<PreparedAudioPeak>,
    end: usize,
    width: usize,
    extra: usize,
}

impl PeakBins {
    pub(super) fn into_peaks(self) -> Vec<PreparedAudioPeak> {
        self.peaks
    }

    pub(super) fn new(frames: usize, limit: usize) -> Option<Self> {
        let count = limit.min(frames);
        if count == 0 {
            return None;
        }
        let mut peaks = Vec::new();
        peaks.try_reserve_exact(count).ok()?;
        Some(Self {
            peaks,
            end: 0,
            width: frames / count,
            extra: frames % count,
        })
    }

    pub(super) fn observe(&mut self, index: usize, sample: Frame) {
        if index == self.end {
            self.peaks.push(PreparedAudioPeak {
                min: sample,
                max: sample,
            });
            self.end += self.width + usize::from(self.peaks.len() <= self.extra);
        } else if let Some(peak) = self.peaks.last_mut() {
            for (channel, value) in sample.into_iter().enumerate() {
                peak.min[channel] = peak.min[channel].min(value);
                peak.max[channel] = peak.max[channel].max(value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_musical_occurrences_keep_neutral_source_clocks_and_cache_identity() {
        use crate::audio_transform::{
            MusicalEventFadePreview,
            tests::{fixture, isolated, prepare},
        };
        use hex_project::{
            AudioEventEdit, AudioEventFade, AudioViewProjection, AudioViewResizeOperation,
            ClipFade, Command, Tempo, WarpMode, default_audio_warp,
        };

        let (mut editor, clip, media) = fixture(false);
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        let project = editor.project();
        let settings = default_audio_warp(
            project.clip(clip).unwrap(),
            project.tempo_map(),
            WarpMode::RePitch,
        )
        .unwrap();
        for command in [
            Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            },
            Command::SetTempoPoint {
                beat: BeatTime::ONE,
                tempo: Tempo::new(240_000_000).unwrap(),
            },
            Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: 20_000,
                },
            },
            Command::SetClipFadeEnvelope {
                clip,
                fade: ClipFade::neutral(20_000).unwrap(),
            },
        ] {
            editor.execute(command).unwrap();
        }
        let context = isolated(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024);
        let original = prepare(&editor, &media, &context);
        let view = &original.arrangement().audio_clip_views()[0];
        assert!(view.repeat.is_none());
        let table = view
            .event_source_occurrences
            .clone()
            .expect("neutral sources for the distinct prepared roots");
        let project = editor.project();
        let projection =
            AudioViewProjection::new(project.clip(clip).unwrap(), project.tempo_map()).unwrap();
        let reads = table.reads();
        assert_eq!(reads.len() as u64, projection.occurrence_count());
        for (index, read) in reads.iter().enumerate() {
            let occurrence = projection.occurrence(index as u64).unwrap();
            assert_eq!(read.placement, occurrence);
            assert_eq!(
                read.source.source_frames() as u64,
                occurrence.root_frame_length
            );
            let clock = read.source.musical_clock().unwrap();
            assert_eq!(
                clock.frame_at(BeatTime::ONE).unwrap(),
                occurrence.root_frame_length
            );
        }
        assert_eq!(reads[0].source.source_frames(), 8_000);
        assert_eq!(reads[1].source.source_frames(), 4_000);
        let reused = prepare(&editor, &media, &context);
        assert!(Arc::ptr_eq(
            &table,
            reused.arrangement().audio_clip_views()[0]
                .event_source_occurrences
                .as_ref()
                .unwrap()
        ));

        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::SetFade {
                    event: 1,
                    fade: Some(
                        AudioEventFade::new(
                            0,
                            ClipFade::new(BeatTime::ONE.raw(), 0, BeatTime::ONE.raw() / 4, 0)
                                .unwrap(),
                        )
                        .unwrap(),
                    ),
                },
            })
            .unwrap();
        let faded = prepare(&editor, &media, &context);
        let faded_view = &faded.arrangement().audio_clip_views()[0];
        let event_table = faded_view.event_source_occurrences.clone().unwrap();
        for (before, after) in table.reads().iter().zip(event_table.reads()) {
            assert_eq!(before.placement, after.placement);
            assert_eq!(before.source.bins(), after.source.bins());
            assert_eq!(before.source.musical_clock(), after.source.musical_clock());
        }
        assert_ne!(faded_view.audio.frames(), view.audio.frames());

        editor
            .execute(Command::SetClipFadeEnvelope {
                clip,
                fade: ClipFade::new(20_000, 0, 1_241, 2_681).unwrap(),
            })
            .unwrap();
        let baked = prepare(&editor, &media, &context);
        let baked_view = &baked.arrangement().audio_clip_views()[0];
        assert!(Arc::ptr_eq(
            &event_table,
            baked_view.event_source_occurrences.as_ref().unwrap()
        ));
        assert_ne!(baked_view.audio.frames(), faded_view.audio.frames());
        let project = editor.project();
        let row = project.clip(clip).unwrap();
        let events = row.audio_events();
        for occurrence in event_table.reads() {
            let preview = MusicalEventFadePreview::for_occurrence(
                row,
                project.tempo_map(),
                occurrence,
                &events.events,
                row.gain(),
            )
            .unwrap()
            .unwrap();
            let read = occurrence.placement;
            for frame in read.destination_start..read.destination_end {
                let mut sample = None;
                let coverage = preview
                    .visit_arrangement(frame..frame + 1, &mut 1, |part| {
                        assert!(sample.is_none());
                        assert_eq!(
                            part.sample.source_frame(),
                            read.root_start + frame - read.destination_start
                        );
                        sample = Some(part.sample);
                    })
                    .unwrap()
                    .unwrap();
                assert!(!coverage.has_silence);
                let index = frame as usize;
                assert_eq!(
                    sample.unwrap().apply_to(view.audio.frames()[index]),
                    baked_view.audio.frames()[index],
                    "destination frame {frame} in occurrence {:?}",
                    read.root_origin
                );
            }
        }

        let uncached_context = isolated(1_000_000, 0, 0);
        let uncached = prepare(&editor, &media, &uncached_context);
        let uncached_view = &uncached.arrangement().audio_clip_views()[0];
        assert!(uncached_view.event_source_occurrences.is_none());
        assert_eq!(uncached_view.audio.frames(), baked_view.audio.frames());

        let pcm_budget = Arc::clone(&context.budget);
        drop((original, reused, faded, baked, context));
        assert_eq!(pcm_budget.used(), 0);
        assert_eq!(event_table.reads()[0].source.source_frames(), 8_000);
    }

    #[test]
    fn event_source_summary_releases_its_independent_budget_on_cancel_and_drop() {
        let frames = [[0.125, -0.5]; 128];
        let bytes = PreparedEventSourcePeaks::storage_bytes(frames.len());
        let budget = Arc::new(Budget::new(bytes));
        let mut polls = 0;
        assert!(
            PreparedEventSourcePeaks::capture(&frames, None, &budget, || {
                polls += 1;
                polls == 2
            })
            .is_none()
        );
        assert_eq!(budget.used(), 0);
        let summary = PreparedEventSourcePeaks::capture(&frames, None, &budget, || false).unwrap();
        assert_eq!(budget.used(), bytes);
        assert!(PreparedEventSourcePeaks::capture(&frames, None, &budget, || false).is_none());
        let retained = Arc::clone(&summary);
        drop(summary);
        assert_eq!(budget.used(), bytes);
        drop(retained);
        assert_eq!(budget.used(), 0);
    }
}
