//! Bounded visual projection of accepted processing-root peaks through the
//! canonical placement query. This never prepares audio or owns a DSP clock.
use super::waveform::{
    WaveformGrid, WaveformPreview, aggregate_peaks, aggregate_ranges, timeline_source_ranges_at,
    waveform_canvas,
};
use super::waveform_geometry::{WaveformGeometryCache, WaveformMapping};
use gpui::Div;
use hex_project::{
    AudioDomainClock, AudioViewProjection, AudioViewResizeOperation, Clip, TempoMap,
    audio_projection_frame, preview_audio_view_resize,
};
use hex_session::{CachedPeak, CachedWaveform};
use std::{cell::RefCell, ops::Range, rc::Rc, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LoopProjection {
    original: Clip,
    preview: Clip,
    tempo: TempoMap,
    prepared: bool,
    accepted_origin: i128,
    retimed: bool,
}

pub(super) fn loop_waveform_view(
    request: WaveformPreview<'_>,
    start: u64,
    end: u64,
    cache: Rc<RefCell<WaveformGeometryCache>>,
    grid: Option<WaveformGrid>,
) -> Option<Div> {
    request.preview.audio_edit()?.placement?;
    let same_root = request
        .original
        .audio_edit()
        .zip(request.preview.audio_edit())
        .is_some_and(|(a, b)| Arc::ptr_eq(&a.domain, &b.domain));
    let mut retimed = false;
    if !same_root {
        let endpoint = request
            .preview
            .start()
            .checked_add(request.preview.duration())?;
        let asset_frames = request.asset.map_or(
            request
                .original
                .source_offset()
                .checked_add(request.original.source_length())?,
            |source| source.source_frames as u64,
        );
        let repeated = preview_audio_view_resize(
            request.original,
            request.tempo,
            asset_frames,
            AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: endpoint,
            },
        )
        .ok();
        if repeated.as_ref() != Some(request.preview) {
            let edge = if request.original.start() == request.preview.start() {
                hex_project::TrimEdge::Right
            } else {
                hex_project::TrimEdge::Left
            };
            let frame = if edge == hex_project::TrimEdge::Right {
                endpoint
            } else {
                request.preview.start()
            };
            let stretched = hex_project::preview_audio_clip_stretch(
                request.original,
                edge,
                frame,
                request.tempo,
                asset_frames,
            )
            .ok()?;
            if &stretched != request.preview {
                return None;
            }
            retimed = true;
        }
    }
    let (source, accepted_origin, prepared) = if let Some(old) = request.original.audio_edit() {
        let accepted = request.prepared?;
        if old.placement.is_some() {
            let root = accepted.preview_root.as_ref()?;
            (Arc::clone(&root.waveform), root.origin, true)
        } else {
            let (_, length, offset, _) = old.projected_domain(request.tempo).ok()?;
            if accepted.offset != offset
                || accepted.length != request.original.duration()
                || accepted.waveform.source_frames as u64 != length
            {
                return None;
            }
            (
                Arc::clone(&accepted.waveform),
                if old.reference_start_raw().is_some() {
                    old.processing_origin().ok()?
                } else {
                    0
                },
                true,
            )
        }
    } else {
        (Arc::clone(request.asset?), 0, false)
    };
    let projection = Arc::new(LoopProjection {
        original: request.original.clone(),
        preview: request.preview.clone(),
        tempo: request.tempo.clone(),
        prepared,
        accepted_origin,
        retimed,
    });
    Some(waveform_canvas(
        source,
        WaveformMapping::Loop(projection),
        start,
        end,
        Some(cache),
        grid,
    ))
}

impl LoopProjection {
    pub(super) fn gain(&self) -> hex_project::ClipGain {
        self.preview.gain()
    }
    fn root_frame(&self, clock: u64) -> Option<u64> {
        if self.retimed {
            return super::waveform_stretch::accepted_root_frame(
                &self.original,
                &self.preview,
                &self.tempo,
                self.prepared,
                clock,
            );
        }
        let edit = self.preview.audio_edit()?;
        match edit.domain.clock {
            AudioDomainClock::Frames(_) => Some(clock),
            AudioDomainClock::Beats(_) => {
                let tempo = self.original.audio_edit()?.processing_tempo(&self.tempo);
                u64::try_from(
                    audio_projection_frame(tempo, self.accepted_origin + i128::from(clock)).ok()?
                        - audio_projection_frame(tempo, self.accepted_origin).ok()?,
                )
                .ok()
            }
        }
    }
    fn at(&self, occurrence: hex_project::AudioViewOccurrence, frame: u64) -> Option<u64> {
        let edit = self.preview.audio_edit()?;
        match edit.domain.clock {
            AudioDomainClock::Frames(_) => occurrence
                .root_start
                .checked_add(frame.checked_sub(occurrence.destination_start)?),
            AudioDomainClock::Beats(_) => self.root_frame(
                u64::try_from(
                    edit.reference_beat_at_destination(&self.preview, frame, &self.tempo)
                        .ok()?
                        - occurrence.root_origin,
                )
                .ok()?,
            ),
        }
    }
    fn peaks_for_root(
        &self,
        waveform: &CachedWaveform,
        range: Range<u64>,
    ) -> Option<[CachedPeak; 2]> {
        if self.prepared {
            aggregate_peaks(
                waveform,
                usize::try_from(range.start).ok()?,
                usize::try_from(range.end).ok()?,
            )
        } else {
            let ranges =
                timeline_source_ranges_at(&self.original, 0, range, waveform.source_frames);
            aggregate_ranges(waveform, ranges.as_slice())
        }
    }
    pub(super) fn peaks(
        &self,
        waveform: &CachedWaveform,
        visible: Range<u64>,
    ) -> Option<[CachedPeak; 2]> {
        let p = AudioViewProjection::new(&self.preview, &self.tempo).ok()?;
        let start = visible.start.max(self.preview.start());
        let end = visible
            .end
            .min(self.preview.start().checked_add(self.preview.duration())?);
        if start >= end {
            return None;
        }
        let mut lo = 0;
        let mut hi = p.occurrence_count();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if p.occurrence(mid).ok()?.destination_end <= start {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let first = lo;
        // Three intersected occurrences include a complete cycle. Summarize
        // its source interval once, regardless of repeat density or zoom.
        if first.checked_add(2)? < p.occurrence_count()
            && p.occurrence(first + 2).ok()?.destination_start < end
        {
            let edit = self.preview.audio_edit()?;
            return self.peaks_for_root(
                waveform,
                self.root_frame(edit.view_start)?..self.root_frame(edit.view_end)?,
            );
        }
        let mut peaks: Option<[CachedPeak; 2]> = None;
        for index in first..p.occurrence_count().min(first.saturating_add(2)) {
            let occurrence = p.occurrence(index).ok()?;
            let a = start.max(occurrence.destination_start);
            let b = end.min(occurrence.destination_end);
            if a >= b {
                continue;
            }
            let a = self.at(occurrence, a)?;
            let b = self.at(occurrence, b)?.max(a.saturating_add(1));
            if let Some(next) = self.peaks_for_root(waveform, a..b) {
                if let Some(peaks) = peaks.as_mut() {
                    for c in 0..2 {
                        peaks[c].min = peaks[c].min.min(next[c].min);
                        peaks[c].max = peaks[c].max.max(next[c].max);
                    }
                } else {
                    peaks = Some(next);
                }
            }
        }
        peaks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project, TrimEdge};

    #[test]
    fn loop_preview_wraps_the_captured_nonzero_source_crop_and_aggregates_dense_cycles() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 2,
            })
            .unwrap();
        let original = editor.project().clip(clip).unwrap().clone();
        let tempo = editor.project().tempo_map().clone();
        let repeated = preview_audio_view_resize(
            &original,
            &tempo,
            8,
            AudioViewResizeOperation::RepeatOuterEnd { project_frame: 26 },
        )
        .unwrap();
        let waveform = CachedWaveform::from_peaks(
            8,
            (0..8)
                .map(|n| {
                    [CachedPeak {
                        min: n as f32 / 10.0,
                        max: n as f32 / 10.0,
                    }; 2]
                })
                .collect(),
        )
        .unwrap();
        let projection = LoopProjection {
            original: original.clone(),
            preview: repeated.clone(),
            tempo: tempo.clone(),
            prepared: false,
            accepted_origin: 0,
            retimed: false,
        };
        for range in [2..3, 8..9, 14..15] {
            assert_eq!(projection.peaks(&waveform, range).unwrap()[0].max, 0.2);
        }
        let dense = projection.peaks(&waveform, 2..26).unwrap()[0];
        assert_eq!((dense.min, dense.max), (0.2, 0.7));
        let resized = preview_audio_view_resize(
            &repeated,
            &tempo,
            8,
            AudioViewResizeOperation::CycleEnd { project_frame: 6 },
        )
        .unwrap();
        let projection = LoopProjection {
            preview: resized,
            ..projection
        };
        assert_eq!(projection.peaks(&waveform, 6..7).unwrap()[0].max, 0.2);
        let dense = projection.peaks(&waveform, 2..26).unwrap()[0];
        assert_eq!(
            (dense.min, dense.max),
            (0.2, 0.5),
            "shorter cycles do not paint hidden tail samples"
        );
        assert!(projection.peaks(&waveform, 0..2).is_none());
        assert!(projection.peaks(&waveform, 26..30).is_none());
    }
    #[test]
    fn musical_loop_preview_uses_absolute_beats_across_a_tempo_seam() {
        use hex_project::{BeatTime, Tempo, WarpMode, default_audio_warp};
        let beat = |n| BeatTime::from_quarters(n).unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let warp = default_audio_warp(
            editor.project().clip(clip).unwrap(),
            editor.project().tempo_map(),
            WarpMode::RePitch,
        )
        .unwrap();
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(warp),
            })
            .unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: beat(2),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        let original = editor.project().clip(clip).unwrap().clone();
        let tempo = editor.project().tempo_map().clone();
        assert_eq!(original.duration(), 144_000);
        let preview = preview_audio_view_resize(
            &original,
            &tempo,
            96_000,
            AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: tempo.beat_to_frame(beat(10)).unwrap(),
            },
        )
        .unwrap();
        let waveform = CachedWaveform::from_peaks(
            144_000,
            (0..6)
                .map(|n| {
                    [CachedPeak {
                        min: n as f32 / 10.0,
                        max: n as f32 / 10.0,
                    }; 2]
                })
                .collect(),
        )
        .unwrap();
        let projection = LoopProjection {
            original,
            preview,
            tempo: tempo.clone(),
            prepared: true,
            accepted_origin: 0,
            retimed: false,
        };
        for (a, b, expected) in [
            (0, 1, 0.0),
            (1, 2, 0.1),
            (4, 5, 0.0),
            (5, 6, 0.1),
            (8, 9, 0.0),
        ] {
            let actual = projection
                .peaks(
                    &waveform,
                    tempo.beat_to_frame(beat(a)).unwrap()..tempo.beat_to_frame(beat(b)).unwrap(),
                )
                .unwrap()[0];
            assert_eq!(
                (actual.min, actual.max),
                (expected, expected),
                "beats {a}..{b}: later periods differ in frames but retain source phase"
            );
        }
    }
    #[test]
    fn stretched_loop_preview_repeats_selected_root_and_excludes_hidden_tail_each_cycle() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 320,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 64,
            })
            .unwrap();
        let tempo = editor.project().tempo_map().clone();
        let repeated = preview_audio_view_resize(
            editor.project().clip(clip).unwrap(),
            &tempo,
            320,
            AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 1100,
            },
        )
        .unwrap();
        let original = preview_audio_view_resize(
            &repeated,
            &tempo,
            320,
            AudioViewResizeOperation::CycleEnd { project_frame: 192 },
        )
        .unwrap();
        let preview =
            hex_project::preview_audio_clip_stretch(&original, TrimEdge::Right, 2112, &tempo, 320)
                .unwrap();
        let waveform = CachedWaveform::from_peaks(
            256,
            (0..256)
                .map(|n| {
                    let value = if n < 128 { 0.75 } else { 0.25 };
                    [CachedPeak {
                        min: value,
                        max: value,
                    }; 2]
                })
                .collect(),
        )
        .unwrap();
        let projection = LoopProjection {
            original,
            preview,
            tempo,
            prepared: true,
            accepted_origin: 64,
            retimed: true,
        };
        let placement = AudioViewProjection::new(&projection.preview, &projection.tempo).unwrap();
        assert!(placement.occurrence_count() >= 4);
        for index in [0, 1, 3, placement.occurrence_count() - 1] {
            let at = placement.occurrence(index).unwrap().destination_start + 8;
            assert_eq!(
                projection.peaks(&waveform, at..at + 1).unwrap()[0],
                CachedPeak {
                    min: 0.75,
                    max: 0.75
                },
                "cycle {index}"
            );
        }
        let end = projection.preview.start() + projection.preview.duration();
        assert_eq!(
            projection
                .peaks(&waveform, projection.preview.start()..end)
                .unwrap()[0],
            CachedPeak {
                min: 0.75,
                max: 0.75
            },
            "dense columns summarize the selected cycle, not the hidden root tail"
        );
    }
}
