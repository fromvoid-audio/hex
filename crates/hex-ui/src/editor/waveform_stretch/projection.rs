//! Project prospective stretch frames onto the accepted source root.
use super::accepted_root_frame;
use crate::editor::waveform_source::{SourceRanges, push_source_range};
use hex_project::{Clip, ClipGain, TempoMap};
use std::ops::Range;

/// Retained, value-comparable metadata makes cache identity cover both maps and
/// the tempo map. It cannot become stale through pointer reuse or root replacement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::editor) struct StretchProjection {
    pub(super) original: Clip,
    pub(super) preview: Clip,
    pub(super) tempo: TempoMap,
    pub(super) prepared: bool,
}

impl StretchProjection {
    pub(in crate::editor) fn gain(&self) -> ClipGain {
        self.preview.gain()
    }

    /// Inverse prospective frame -> accepted root/source frame, with Q32 source
    /// interpolation shared with audio preparation. Work is per visible column.
    fn position(&self, timeline: u64) -> Option<u64> {
        let edit = self.preview.audio_edit()?;
        let (origin, _, _, _) = edit.projected_domain(&self.tempo).ok()?;
        let root_frame = u64::try_from(i128::from(timeline) - origin).ok()?;
        let beat = edit.root_position_at(root_frame, &self.tempo).ok()?;
        accepted_root_frame(
            &self.original,
            &self.preview,
            &self.tempo,
            self.prepared,
            beat,
        )
    }

    pub(in crate::editor) fn ranges(&self, visible: Range<u64>, available: usize) -> SourceRanges {
        let mut ranges = SourceRanges::default();
        let start = visible.start.max(self.preview.start());
        let end = visible
            .end
            .min(self.preview.start().saturating_add(self.preview.duration()));
        if start >= end {
            return ranges;
        }
        let Some(a) = self.position(start) else {
            return ranges;
        };
        let Some(b) = self.position(end) else {
            return ranges;
        };
        // Rounding may collapse a sub-frame column. Include its real source frame.
        let b = b.max(a.saturating_add(1));
        if self.prepared {
            push_source_range(
                &mut ranges,
                a.min(available as u64),
                b.min(available as u64),
                available,
            );
        } else {
            let offset = self.original.source_offset();
            let length = self.original.source_length();
            if length == 0 {
                return ranges;
            }
            if !self.original.looping() {
                push_source_range(
                    &mut ranges,
                    offset + a.min(length),
                    offset + b.min(length),
                    available,
                );
            } else if b - a >= length {
                push_source_range(&mut ranges, offset, offset + length, available);
            } else {
                let first = a % length;
                let span = b - a;
                let count = span.min(length - first);
                push_source_range(
                    &mut ranges,
                    offset + first,
                    offset + first + count,
                    available,
                );
                if count < span {
                    push_source_range(&mut ranges, offset, offset + span - count, available);
                }
            }
        }
        ranges
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_project::{
        BeatTime, BeatsPreserve, BeatsTail, Command, Created, Editor, Project, Tempo, WarpMarker,
        WarpMode, WarpSettings,
    };
    use hex_project::{TrimEdge, preview_audio_clip_stretch};
    use hex_session::{CachedPeak, CachedWaveform};

    fn fixture() -> (Editor, hex_project::ClipId) {
        let mut editor = Editor::new(Project::new(44_100).unwrap(), 32).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "peaks".into(),
                path: "original.wav".into(),
                sample_rate: 44_100,
                frames: 88_200,
            })
            .unwrap()
        else {
            panic!("import")
        };
        (editor, clip)
    }
    fn projection(
        editor: &Editor,
        id: hex_project::ClipId,
        edge: TrimEdge,
        frame: u64,
    ) -> StretchProjection {
        let original = editor.project().clip(id).unwrap().clone();
        let tempo = editor.project().tempo_map().clone();
        let preview = preview_audio_clip_stretch(&original, edge, frame, &tempo, 88_200).unwrap();
        StretchProjection {
            prepared: original.audio_edit().is_some(),
            original,
            preview,
            tempo,
        }
    }

    #[test]
    fn raw_peak_location_uses_source_markers_across_tempo_points() {
        let (mut editor, id) = fixture();
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_raw(2 * BeatTime::ONE.raw()),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        let p = projection(&editor, id, TrimEdge::Right, 132_300);
        // Independent physical oracle: the prospective 4-beat root maps beat 1 to
        // source quarter (22050). An inverse old beat bitmap yields 16538 instead.
        assert_eq!(p.position(22_050), Some(22_050));
        let mut peaks = vec![[CachedPeak { min: 0.0, max: 0.0 }; 2]; 882];
        peaks[220] = [
            CachedPeak {
                min: -0.75,
                max: 0.5,
            },
            CachedPeak {
                min: -0.25,
                max: 0.875,
            },
        ];
        let source = CachedWaveform::from_peaks(88_200, peaks).unwrap();
        let ranges = p.ranges(22_050..22_136, 88_200);
        let actual =
            crate::editor::waveform_source::aggregate_ranges(&source, ranges.as_slice()).unwrap();
        assert_eq!(actual[0].min, -0.75);
        assert_eq!(actual[1].max, 0.875);
        let full = p.ranges(0..132_300, 88_200);
        assert_eq!(full.as_slice().len(), 1);
        assert_eq!(full.as_slice()[0], 0..88_200);
    }

    #[test]
    fn raw_loop_phase_wrap_and_partial_source_ranges_are_real() {
        let (mut editor, id) = fixture();
        editor
            .execute(Command::SetClipLooping {
                clip: id,
                looping: true,
            })
            .unwrap();
        editor
            .execute(Command::SetClipSourcePhase {
                clip: id,
                phase: 701,
            })
            .unwrap();
        let p = projection(&editor, id, TrimEdge::Right, 176_400);
        // Q32 beat phase floors to source700 (within one frame of authored701).
        // Two output frames per source frame; a window crossing source wrap must
        // read the actual tail and head, in stereo, without a full-asset substitute.
        let ranges = p.ranges(174_990..175_010, 88_200);
        assert_eq!(ranges.as_slice(), &[88_195..88_200, 0..5]);
        let full = p.ranges(0..176_400, 88_200);
        assert_eq!(full.as_slice().len(), 1);
        assert_eq!(full.as_slice()[0], 0..88_200);
        assert!(p.ranges(176_400..180_000, 88_200).as_slice().is_empty());
    }

    #[test]
    fn prepared_root_mapping_keeps_old_tempo_clock_and_crop() {
        let (mut editor, id) = fixture();
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_raw(2 * BeatTime::ONE.raw()),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipWarp {
                clip: id,
                settings: Some(WarpSettings {
                    mode: WarpMode::Beats {
                        preserve: BeatsPreserve::Transients,
                        tail: BeatsTail::Forward,
                        decay: 100,
                    },
                    markers: vec![
                        WarpMarker {
                            source_frame: 0,
                            beat: BeatTime::ZERO,
                        },
                        WarpMarker {
                            source_frame: 22_050,
                            beat: BeatTime::ONE,
                        },
                        WarpMarker {
                            source_frame: 88_200,
                            beat: BeatTime::from_raw(3 * BeatTime::ONE.raw()),
                        },
                    ],
                }),
            })
            .unwrap();
        editor
            .execute(Command::TrimClipEdge {
                clip: id,
                edge: TrimEdge::Left,
                frame: 22_050,
            })
            .unwrap();
        let p = projection(&editor, id, TrimEdge::Right, 132_300);
        // Original crop is [1,3] beats; prospective [1,4] gives ratio 3/2.
        // New root beat 3 maps to old root beat 2: frame44100 at the tempo point.
        let new_root_beat3 = p
            .preview
            .audio_edit()
            .unwrap()
            .root_frame_at(3 * BeatTime::ONE.raw(), &p.tempo)
            .unwrap();
        let origin = p
            .preview
            .audio_edit()
            .unwrap()
            .projected_domain(&p.tempo)
            .unwrap()
            .0;
        let timeline = u64::try_from(origin + i128::from(new_root_beat3)).unwrap();
        assert!(p.position(timeline).unwrap().abs_diff(44_100) <= 1);
        let first = p.ranges(p.preview.start()..p.preview.start() + 1, 88_200);
        assert!(first.as_slice()[0].start.abs_diff(22_050) <= 1);
    }

    #[test]
    fn projection_identity_tracks_the_real_preview_and_tempo() {
        let (editor, id) = fixture();
        let original = projection(&editor, id, TrimEdge::Right, 132_300);
        assert_eq!(original, original.clone());
        assert_ne!(original, projection(&editor, id, TrimEdge::Right, 132_301));
    }
}
