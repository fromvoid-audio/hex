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
    let (source, accepted_origin, prepared) = accepted_loop_source(request)?;
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
        request.ink,
    ))
}

/// A directed outer waveform already contains reflection. Loop gestures must
/// sample its explicitly accepted forward processing root instead of mapping
/// those finite output peaks as if they were untouched source content.
fn accepted_loop_source(request: WaveformPreview<'_>) -> Option<(Arc<CachedWaveform>, i128, bool)> {
    Some(if let Some(old) = request.original.audio_edit() {
        let preview = request.preview.audio_edit()?;
        // Accepted root peaks already include the forward container fade.
        // Reflecting them would move that fade to the wrong side. Until a
        // destination-fade-aware preview exists, keep real resize geometry
        // and wait for the admitted candidate's waveform instead.
        if preview.content_view.is_some()
            && (!old.domain.fade.is_neutral() || !preview.domain.fade.is_neutral())
        {
            return None;
        }
        let accepted = request.prepared?;
        if old.placement.is_some() || old.content_view.is_some() {
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
    })
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
        if edit.placement.is_some_and(|p| p.mode.leading_ticks() > 0) {
            return occurrence
                .root_start
                .checked_add(frame.checked_sub(occurrence.destination_start)?);
        }
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
        if self.preview.audio_edit()?.content_view.is_some() {
            let mut peaks: Option<[CachedPeak; 2]> = None;
            // Directed occurrences retain base-placement coordinates. The
            // canonical interval query reflects/crops/wraps in constant work
            // and returns at most two processing-root ranges.
            for range in p
                .root_ranges_for_frames(start..end)
                .ok()?
                .into_iter()
                .flatten()
            {
                let a = self.root_frame(range.start)?;
                let b = self.root_frame(range.end)?.max(a.checked_add(1)?);
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
            return peaks;
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

    fn directed_fixture() -> (Editor, hex_project::ClipId, Arc<CachedWaveform>) {
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
            panic!("clip");
        };
        // Capture a retained full root before trimming; peaks below represent
        // the accepted forward processing root, including its hidden prefix.
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 1 })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 0 })
            .unwrap();
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 2,
            })
            .unwrap();
        let waveform = Arc::new(
            CachedWaveform::from_peaks(
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
            .unwrap(),
        );
        (editor, clip, waveform)
    }

    #[test]
    fn leading_musical_waveform_starts_at_the_retained_read() {
        let (mut editor, clip, waveform) = directed_fixture();
        let track = editor.project().tracks()[0].id();
        editor
            .execute(Command::MoveClip {
                clip,
                track,
                start: 12,
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })
            .unwrap();
        let project = editor.project();
        let original = project.clip(clip).unwrap().clone();
        let tempo = project.tempo_map().clone();
        let accepted_origin = original.audio_edit().unwrap().processing_origin().unwrap();
        let preview = original
            .preview_edge_trim_with_tempo(TrimEdge::Left, 7, 8, &tempo)
            .unwrap();
        let view = AudioViewProjection::new(&preview, &tempo).unwrap();
        assert_eq!(view.outer_frame_bounds(), (7, 18));
        assert_eq!(view.occurrence(0).unwrap().destination_start, 10);
        let projection = LoopProjection {
            original,
            preview,
            tempo,
            prepared: true,
            accepted_origin,
            retimed: false,
        };
        assert!(projection.peaks(&waveform, 7..10).is_none());
        let first = projection.peaks(&waveform, 10..11).unwrap()[0];
        assert_eq!((first.min, first.max), (0.0, 0.0));
        let last = projection.peaks(&waveform, 17..18).unwrap()[0];
        assert_eq!((last.min, last.max), (0.7, 0.7));
        assert!(projection.peaks(&waveform, 18..19).is_none());
    }

    #[test]
    fn directed_phased_repeat_peaks_follow_reflected_trimmed_source_ranges() {
        let (mut editor, clip, waveform) = directed_fixture();
        editor
            .execute(Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::RepeatOuterEnd { project_frame: 26 },
            })
            .unwrap();
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 3,
            })
            .unwrap();
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let original = editor.project().clip(clip).unwrap().clone();
        assert_eq!(
            original
                .audio_edit()
                .unwrap()
                .content_view
                .unwrap()
                .base_placement
                .unwrap()
                .cycle_phase,
            1
        );
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 4,
            })
            .unwrap();
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Right,
                frame: 24,
            })
            .unwrap();
        let projection = LoopProjection {
            original,
            preview: editor.project().clip(clip).unwrap().clone(),
            tempo: editor.project().tempo_map().clone(),
            prepared: true,
            accepted_origin: 0,
            retimed: false,
        };
        // Literal forward cycle [2,3,4,5,6,7] at phase1, reversed and then
        // cropped by one frame at the left: [6,5,4,3,2,7,...].
        for (frame, expected) in [(4, 0.6), (5, 0.5), (8, 0.2), (9, 0.7), (10, 0.6)] {
            let peak = projection.peaks(&waveform, frame..frame + 1).unwrap()[0];
            assert_eq!((peak.min, peak.max), (expected, expected), "frame {frame}");
        }
        let peak = projection.peaks(&waveform, 4..24).unwrap()[0];
        assert_eq!((peak.min, peak.max), (0.2, 0.7));
        assert!(projection.peaks(&waveform, 0..4).is_none());
        assert!(projection.peaks(&waveform, 24..30).is_none());
    }

    #[test]
    fn first_directed_repeat_preview_selects_explicit_forward_root() {
        let (mut editor, clip, root) = directed_fixture();
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let original = editor.project().clip(clip).unwrap();
        assert!(original.audio_edit().unwrap().placement.is_none());
        let tempo = editor.project().tempo_map();
        let preview = preview_audio_view_resize(
            original,
            tempo,
            8,
            AudioViewResizeOperation::RepeatOuterEnd { project_frame: 26 },
        )
        .unwrap();
        // Accepted outer PCM is already reversed and cropped. It must never be
        // relabeled as the full forward processing root during the first loop.
        let outer = Arc::new(
            CachedWaveform::from_peaks(
                6,
                vec![
                    [CachedPeak {
                        min: -1.0,
                        max: -1.0
                    }; 2];
                    6
                ],
            )
            .unwrap(),
        );
        let accepted = hex_session::service::AudioClipWaveform {
            waveform: outer,
            offset: 0,
            length: original.duration(),
            preview_root: Some(hex_session::service::AudioPreviewWaveform {
                waveform: Arc::clone(&root),
                origin: 0,
            }),
        };
        let request = WaveformPreview {
            original,
            preview: &preview,
            tempo,
            asset: None,
            prepared: Some(&accepted),
            ink: crate::theme::color(crate::theme::PaletteRole::Waveform),
        };
        let (chosen, origin, prepared) = accepted_loop_source(request).unwrap();
        assert!(Arc::ptr_eq(&chosen, &root));
        let projection = LoopProjection {
            original: original.clone(),
            preview: preview.clone(),
            tempo: tempo.clone(),
            prepared,
            accepted_origin: origin,
            retimed: false,
        };
        assert_eq!(projection.peaks(&chosen, 2..3).unwrap()[0].max, 0.7);
        let missing = hex_session::service::AudioClipWaveform {
            preview_root: None,
            ..accepted.clone()
        };
        assert!(
            accepted_loop_source(WaveformPreview {
                preview: &projection.preview,
                prepared: Some(&missing),
                ..request
            })
            .is_none()
        );
    }

    #[test]
    fn directed_loop_preview_refuses_to_reflect_an_asymmetric_container_fade() {
        let (mut editor, clip, _) = directed_fixture();
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 4,
                fade_out: 0,
            })
            .unwrap();
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let original = editor.project().clip(clip).unwrap();
        let preview = preview_audio_view_resize(
            original,
            editor.project().tempo_map(),
            8,
            AudioViewResizeOperation::RepeatOuterEnd { project_frame: 26 },
        )
        .unwrap();
        // A constant source with a left fade has these forward root peaks.
        // Reflecting them would incorrectly put the fade on the right.
        let root = Arc::new(
            CachedWaveform::from_peaks(
                8,
                [0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0]
                    .into_iter()
                    .map(|value| {
                        [CachedPeak {
                            min: value,
                            max: value,
                        }; 2]
                    })
                    .collect(),
            )
            .unwrap(),
        );
        let accepted = hex_session::service::AudioClipWaveform {
            waveform: Arc::clone(&root),
            offset: 0,
            length: original.duration(),
            preview_root: Some(hex_session::service::AudioPreviewWaveform {
                waveform: root,
                origin: 0,
            }),
        };
        assert!(
            accepted_loop_source(WaveformPreview {
                original,
                preview: &preview,
                tempo: editor.project().tempo_map(),
                asset: None,
                prepared: Some(&accepted),
                ink: crate::theme::color(crate::theme::PaletteRole::Waveform),
            })
            .is_none(),
            "post-fade root peaks cannot represent destination-faded reversed output"
        );
    }

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
