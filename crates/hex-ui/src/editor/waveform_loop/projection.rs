//! Pure projection of cached source peaks through a loop placement.
use super::super::waveform_source::{aggregate_ranges_with_detail, timeline_source_ranges_at};
use hex_project::{AudioDomainClock, AudioViewProjection, Clip, TempoMap, audio_projection_frame};
use hex_session::{CachedPeak, CachedWaveform};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::editor) struct LoopProjection {
    pub(super) original: Clip,
    pub(super) preview: Clip,
    pub(super) tempo: TempoMap,
    pub(super) prepared: bool,
    pub(super) accepted_origin: i128,
    pub(super) retimed: bool,
}

impl LoopProjection {
    pub(in crate::editor) fn gain(&self) -> hex_project::ClipGain {
        self.preview.gain()
    }
    fn root_frame(&self, clock: u64) -> Option<u64> {
        if self.retimed {
            return super::super::waveform_stretch::accepted_root_frame(
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
        budget: usize,
    ) -> Option<[CachedPeak; 2]> {
        if self.prepared {
            let range = usize::try_from(range.start).ok()?..usize::try_from(range.end).ok()?;
            aggregate_ranges_with_detail(waveform, std::slice::from_ref(&range), budget)
        } else {
            let ranges =
                timeline_source_ranges_at(&self.original, 0, range, waveform.source_frames());
            aggregate_ranges_with_detail(waveform, ranges.as_slice(), budget)
        }
    }
    #[cfg(all(test, feature = "ui-unit-tier"))]
    pub(in crate::editor) fn peaks(
        &self,
        waveform: &CachedWaveform,
        visible: Range<u64>,
    ) -> Option<[CachedPeak; 2]> {
        self.peaks_with_detail(waveform, visible, 0)
    }
    pub(in crate::editor) fn peaks_with_detail(
        &self,
        waveform: &CachedWaveform,
        visible: Range<u64>,
        budget: usize,
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
                if let Some(next) = self.peaks_for_root(waveform, a..b, budget / 2) {
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
            let mut peaks = self.peaks_for_root(
                waveform,
                self.root_frame(edit.view_start)?..self.root_frame(edit.view_end)?,
                budget,
            )?;
            if p.has_silent_repeat_gap() {
                for peak in &mut peaks {
                    peak.min = peak.min.min(0.0);
                    peak.max = peak.max.max(0.0);
                }
            }
            return Some(peaks);
        }
        let mut peaks: Option<[CachedPeak; 2]> = None;
        let mut covered = 0_u64;
        for index in first..p.occurrence_count().min(first.saturating_add(2)) {
            let occurrence = p.occurrence(index).ok()?;
            let a = start.max(occurrence.destination_start);
            let b = end.min(occurrence.destination_end);
            if a >= b {
                continue;
            }
            covered = covered.checked_add(b - a)?;
            let a = self.at(occurrence, a)?;
            let b = self.at(occurrence, b)?.max(a.saturating_add(1));
            if let Some(next) = self.peaks_for_root(waveform, a..b, budget / 2) {
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
        if p.has_silent_repeat_gap()
            && covered < end - start
            && let Some(peaks) = peaks.as_mut()
        {
            for peak in peaks {
                peak.min = peak.min.min(0.0);
                peak.max = peak.max.max(0.0);
            }
        }
        peaks
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_project::{
        AudioViewResizeOperation, Command, Created, Editor, Project, TrimEdge,
        preview_audio_view_resize,
    };
    use std::sync::Arc;

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
    fn held_loop_projection_keeps_a_transient_in_its_source_column() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 1_024_000,
            })
            .unwrap()
        else {
            panic!("clip");
        };
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 1 })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 0 })
            .unwrap();
        let original = editor.project().clip(clip).unwrap().clone();
        let projection = LoopProjection {
            preview: original.clone(),
            original,
            tempo: editor.project().tempo_map().clone(),
            prepared: true,
            accepted_origin: 0,
            retimed: false,
        };
        let mut samples = vec![[0.0, 0.0]; 1_024_000];
        samples[2_010] = [1.0, 1.0];
        let audio = hex_engine::Audio::new(48_000, samples).unwrap();
        let waveform = hex_media::waveform::summarize(&audio, 1_024).unwrap();
        assert_eq!(
            projection
                .peaks_with_detail(&waveform, 1_950..2_015, 4_096)
                .unwrap()[0]
                .max,
            1.0
        );
        assert_eq!(
            projection
                .peaks_with_detail(&waveform, 2_015..2_080, 4_096)
                .unwrap()[0]
                .max,
            0.0
        );
    }

    #[test]
    fn held_and_committed_repeat_gap_projects_silence_at_the_same_frames() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
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
            .execute(Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::RepeatOuterEnd { project_frame: 26 },
            })
            .unwrap();
        let original = editor.project().clip(clip).unwrap().clone();
        let tempo = editor.project().tempo_map().clone();
        let operation = AudioViewResizeOperation::CycleEnd { project_frame: 10 };
        let preview = preview_audio_view_resize(&original, &tempo, 8, operation).unwrap();
        editor
            .execute(Command::ResizeAudioClipView { clip, operation })
            .unwrap();
        let committed = editor.project().clip(clip).unwrap().clone();
        assert_eq!(preview, committed);
        let waveform =
            CachedWaveform::from_peaks(8, vec![[CachedPeak { min: 0.5, max: 0.5 }; 2]; 8]).unwrap();
        for preview in [preview, committed] {
            let projection = LoopProjection {
                original: original.clone(),
                preview,
                tempo: tempo.clone(),
                prepared: true,
                accepted_origin: 0,
                retimed: false,
            };
            assert!(projection.peaks(&waveform, 8..10).is_none());
            assert!(projection.peaks(&waveform, 18..20).is_none());
            assert_eq!(projection.peaks(&waveform, 10..11).unwrap()[0].max, 0.5);
            assert_eq!(projection.peaks(&waveform, 20..21).unwrap()[0].max, 0.5);
            assert_eq!(projection.peaks(&waveform, 7..11).unwrap()[0].min, 0.0);
        }
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
