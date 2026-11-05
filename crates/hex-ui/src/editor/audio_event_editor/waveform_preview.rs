//! Clipboard display snapshots contain bounded peaks, never retained PCM. Project
//! edits still use canonical event coordinates; this only projects cached peaks
//! into a pending paste's destination using the owning root-clock APIs.
use super::*;
use crate::editor::waveform::{
    aggregate_peaks, aggregate_ranges, fraction_u64, timeline_source_ranges_at,
};
use hex_project::TempoMap;
use hex_session::{CachedWaveform, service::ViewState};
use std::sync::Arc;

const MAX_BINS: usize = 1_024;

pub(super) struct Source {
    waveform: Arc<CachedWaveform>,
    root: AudioClipEdit,
    tempo: TempoMap,
    first: u64,
    frame_origin: u64,
}
pub(super) struct Preview {
    pub(super) waveform: Arc<CachedWaveform>,
    pub(super) frame_start: u64,
    pub(super) start: u64,
    pub(super) end: u64,
}
impl Source {
    pub(super) fn capture(
        clip: ClipId,
        root: &AudioClipEdit,
        state: &ViewState,
        first: u64,
        end: u64,
    ) -> Option<Self> {
        let tempo = state.project.tempo_map();
        let frame_origin = root.root_frame_at(first, tempo).ok()?;
        let frames = root
            .root_frame_at(end, tempo)
            .ok()?
            .checked_sub(frame_origin)?;
        let frame_count = usize::try_from(frames).ok()?;
        let count = frame_count.min(MAX_BINS);
        if count == 0 {
            return None;
        }
        let clip_data = state.project.clip(clip)?;
        let prepared = state.audio_clip_waveforms.get(&clip);
        let original = state.waveforms.get(&clip_data.asset());
        if prepared.is_none() && (clip_data.audio_edit().is_some() || original.is_none()) {
            return None;
        }
        let mut bins = Vec::with_capacity(count);
        for i in 0..count {
            let a = frame_origin.checked_add(fraction_u64(frames, i, count))?;
            let b = frame_origin.checked_add(fraction_u64(frames, i + 1, count))?;
            let peaks = if let Some(prepared) = prepared {
                aggregate_peaks(
                    &prepared.waveform,
                    usize::try_from(a).ok()?,
                    usize::try_from(b).ok()?,
                )
            } else {
                let original = original?;
                let a = clip_data.start().checked_add(a)?;
                let b = clip_data.start().checked_add(b)?;
                let ranges = timeline_source_ranges_at(
                    clip_data,
                    clip_data.start(),
                    a..b,
                    original.source_frames,
                );
                aggregate_ranges(original, ranges.as_slice())
            };
            bins.push(peaks.unwrap_or([hex_session::CachedPeak { min: 0.0, max: 0.0 }; 2]));
        }
        Some(Self {
            waveform: Arc::new(CachedWaveform::from_peaks(frame_count, bins).ok()?),
            root: root.clone(),
            tempo: root.processing_tempo(tempo).clone(),
            first,
            frame_origin,
        })
    }
    pub(super) fn preview(
        &self,
        root: &AudioClipEdit,
        tempo: &TempoMap,
        start: u64,
        length: u64,
    ) -> Option<Preview> {
        // Changed processing must wait for its own worker summary. Old peaks
        // cannot stand in for a newly warped or retimed processing root.
        if self.root.domain.clock != root.domain.clock {
            return None;
        }
        if matches!(root.domain.clock, hex_project::AudioDomainClock::Beats(_))
            && (self.tempo != *root.processing_tempo(tempo)
                || self.root.processing_origin().ok()? != root.processing_origin().ok()?)
        {
            return None;
        }
        let end = start.checked_add(length)?;
        let first = root.root_frame_at(start, tempo).ok()?;
        let last = root.root_frame_at(end, tempo).ok()?;
        let frames = last.checked_sub(first)?;
        let frame_count = usize::try_from(frames).ok()?;
        let count = frame_count.min(MAX_BINS);
        if count == 0 {
            return None;
        }
        let mut bins = Vec::with_capacity(count);
        for i in 0..count {
            let frame = |index: usize| first.checked_add(fraction_u64(frames, index, count));
            let source_frame = |frame| {
                let position = root.root_position_at(frame, tempo).ok()?;
                let source = self
                    .first
                    .checked_add(position.saturating_sub(start).min(length))?;
                usize::try_from(
                    self.root
                        .root_frame_at(source, &self.tempo)
                        .ok()?
                        .saturating_sub(self.frame_origin),
                )
                .ok()
            };
            let a = source_frame(frame(i)?)?;
            let b = source_frame(frame(i + 1)?)?
                .max(a.checked_add(1)?)
                .min(self.waveform.source_frames);
            bins.push(self.waveform.peaks_for_frames(a, b)?);
        }
        Some(Preview {
            waveform: Arc::new(CachedWaveform::from_peaks(frame_count, bins).ok()?),
            frame_start: first,
            start,
            end,
        })
    }
}
