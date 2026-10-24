use super::*;
use std::ops::Range;

impl AudioViewProjection<'_> {
    /// Source-root intervals visited by a half-open project-frame selection,
    /// intersected with this finite placement. Coordinates use `domain.clock`
    /// (frames or raw BeatTime), matching the audio-event editor, not original
    /// media sample offsets. Repeated visits merge into at most two intervals.
    /// Retained/frozen tempo, displacement, crop and repeat phase use this same
    /// canonical projection; source warp processing is never recomputed here.
    /// Constant work, no allocation; silence/empty intersection returns no ranges.
    pub fn root_ranges_for_frames(
        &self,
        range: Range<u64>,
    ) -> Result<[Option<Range<u64>>; 2], Invalid> {
        if range.start > range.end {
            return Err(Invalid("audio selection range is reversed"));
        }
        let first = range.start.max(self.placed_frames.0);
        let last = range.end.min(self.placed_frames.1);
        if first >= last || self.count == 0 {
            return Ok([None, None]);
        }
        if let Some((window, _, _)) = self.content {
            let first = self.elapsed_at_frame(first)?.min(window.length()?);
            let last = self.elapsed_at_frame(last)?.min(window.length()?);
            let read = window.read_range(first..last)?;
            if let Some(edit) = self
                .clip
                .audio_edit()
                .filter(|e| e.processing_projection.is_some())
            {
                let occurrence = self.occurrence(0)?;
                let position = |elapsed| -> Result<u64, Invalid> {
                    let frame = occurrence
                        .root_start
                        .checked_add(
                            self.base_frame_at_elapsed(elapsed)?
                                .saturating_sub(occurrence.destination_start),
                        )
                        .ok_or(Invalid("directed retained source range overflow"))?;
                    edit.root_position_at(frame, self.tempo)
                };
                let a = position(read.start)?;
                let b = position(read.end)?;
                return Ok([(a < b).then_some(a..b), None]);
            }
            let end = self
                .view_start
                .checked_add(self.cycle)
                .ok_or(Invalid("audio source selection overflow"))?;
            let looping = self
                .clip
                .audio_edit()
                .and_then(|e| e.content_view)
                .and_then(|v| v.base_placement)
                .filter(|p| p.mode == AudioViewMode::Repeat)
                .map(|_| (self.view_start, end));
            return crate::source_ranges::source_ranges(
                u128::from(self.phase) + u128::from(read.start),
                u128::from(self.phase) + u128::from(read.end),
                self.view_start,
                end,
                looping,
            );
        }
        if let Some(edit) = self
            .clip
            .audio_edit()
            .filter(|edit| edit.processing_projection.is_some())
        {
            // This retained root is played sample-for-sample at its new destination.
            // Equal whole-window durations do not imply equal interior tempo maps.
            let occurrence = self.occurrence(0)?;
            let position = |frame: u64| -> Result<u64, Invalid> {
                let offset = frame
                    .checked_sub(occurrence.destination_start)
                    .ok_or(Invalid("audio selection precedes retained occurrence"))?;
                let frame = occurrence
                    .root_start
                    .checked_add(offset)
                    .ok_or(Invalid("audio retained selection frame overflow"))?;
                Ok(edit
                    .root_position_at(frame, self.tempo)?
                    .clamp(edit.view_start, edit.view_end))
            };
            let a = position(first)?;
            let b = position(last)?;
            return Ok([(a < b).then_some(a..b), None]);
        }
        let position = |frame: u64| -> Result<i128, Invalid> {
            let frame = i128::from(frame)
                .checked_sub(self.displacement)
                .ok_or(Invalid("audio selection displacement overflow"))?;
            let raw = if self.beat_clock {
                audio_projection_beat(self.tempo, frame)?
            } else {
                frame
            };
            Ok(raw.clamp(self.start, self.end))
        };
        let elapsed = |frame| -> Result<u128, Invalid> {
            u128::try_from(position(frame)? - self.start)
                .map_err(|_| Invalid("audio selection clock overflow"))
        };
        let start = u128::from(self.phase) + elapsed(first)?;
        let end = u128::from(self.phase) + elapsed(last)?;
        let source_end = self
            .view_start
            .checked_add(self.cycle)
            .ok_or(Invalid("audio selection root end overflow"))?;
        let looping = self
            .clip
            .audio_edit()
            .and_then(|edit| edit.placement)
            .is_some_and(|placement| placement.mode == AudioViewMode::Repeat)
            .then_some((self.view_start, source_end));
        crate::source_ranges::source_ranges(start, end, self.view_start, source_end, looping)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Created, Editor, Project, Tempo};
    use std::sync::Arc;
    fn fixture(frames: u64) -> (Editor, crate::ClipId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames,
            })
            .unwrap()
        else {
            panic!("audio fixture")
        };
        (editor, clip)
    }
    fn b(quarters: u64) -> BeatTime {
        BeatTime::from_quarters(quarters).unwrap()
    }

    #[test]
    fn audio_root_ranges_crop_offset_repeat_and_exhausted_once_are_not_pcm_offsets() {
        let (editor, id) = fixture(200);
        let tempo = editor.project().tempo_map();
        let mut clip = editor.project().clip(id).unwrap().clone();
        clip.start = 1_000;
        clip.duration = 100;
        clip.source_offset = 17;
        clip.source_length = 100;
        clip.fade = crate::ClipFade::neutral(100).unwrap();
        let mut edit = AudioClipEdit::capture(&clip);
        edit.view_start = 20;
        edit.view_end = 60;
        edit.placement = Some(AudioViewPlacement {
            mode: AudioViewMode::Repeat,
            outer_length: AudioDomainClock::Frames(100),
            cycle_phase: 10,
        });
        clip.edit = Some(Arc::new(edit.clone()));
        edit.validate(&clip, tempo, 200).unwrap();
        let projection = AudioViewProjection::new(&clip, tempo).unwrap();
        assert_eq!(
            projection.root_ranges_for_frames(1015..1035).unwrap(),
            [Some(20..25), Some(45..60)]
        );
        assert_eq!(
            projection.root_ranges_for_frames(1000..1100).unwrap(),
            [Some(20..60), None]
        );
        assert_eq!(
            projection.root_ranges_for_frames(0..999).unwrap(),
            [None, None]
        );
        assert!(
            projection
                .root_ranges_for_frames(Range {
                    start: 1050,
                    end: 1000
                })
                .is_err()
        );
        edit.placement.as_mut().unwrap().mode = AudioViewMode::Once;
        clip.edit = Some(Arc::new(edit.clone()));
        edit.validate(&clip, tempo, 200).unwrap();
        let projection = AudioViewProjection::new(&clip, tempo).unwrap();
        assert_eq!(
            projection.root_ranges_for_frames(1025..1060).unwrap(),
            [Some(55..60), None]
        );
        assert_eq!(
            projection.root_ranges_for_frames(1030..1100).unwrap(),
            [None, None]
        );
    }

    #[test]
    fn audio_root_ranges_follow_musical_and_frozen_tempo_with_crop_and_phase() {
        let (mut editor, id) = fixture(192_000);
        editor
            .execute(Command::SetAudioClipMusical {
                clip: id,
                enabled: true,
            })
            .unwrap();
        let tempo = editor.project().tempo_map();
        let mut clip = editor.project().clip(id).unwrap().clone();
        let mut edit = clip.audio_edit().unwrap().clone();
        edit.view_start = b(2).raw();
        edit.view_end = b(6).raw();
        edit.timing = AudioClipTiming::Musical(b(4));
        edit.placement = Some(AudioViewPlacement {
            mode: AudioViewMode::Repeat,
            outer_length: AudioDomainClock::Beats(b(6)),
            cycle_phase: b(1).raw(),
        });
        clip.start = tempo.beat_to_frame(b(4)).unwrap();
        clip.duration = tempo.beat_to_frame(b(10)).unwrap() - clip.start;
        clip.edit = Some(Arc::new(edit.clone()));
        edit.validate(&clip, tempo, 192_000).unwrap();
        let projection = AudioViewProjection::new(&clip, tempo).unwrap();
        assert_eq!(
            projection
                .root_ranges_for_frames(
                    tempo.beat_to_frame(b(6)).unwrap()..tempo.beat_to_frame(b(8)).unwrap()
                )
                .unwrap(),
            [Some(b(2).raw()..b(3).raw()), Some(b(5).raw()..b(6).raw())]
        );
        edit.timing = AudioClipTiming::Frozen {
            reference_start_raw: i128::from(b(4).raw()),
            tempo: Arc::new(tempo.clone()),
        };
        clip.start = 500_000;
        clip.edit = Some(Arc::new(edit.clone()));
        let changed = TempoMap::constant(48_000, Tempo::new(90_000_000).unwrap()).unwrap();
        edit.validate(&clip, &changed, 192_000).unwrap();
        let projection = AudioViewProjection::new(&clip, &changed).unwrap();
        assert_eq!(
            projection.root_ranges_for_frames(548_000..596_000).unwrap(),
            [Some(b(2).raw()..b(3).raw()), Some(b(5).raw()..b(6).raw())]
        );
    }
    #[test]
    fn audio_root_ranges_use_retained_processing_clock_when_total_lengths_match() {
        use crate::TempoPoint;
        let (editor, id) = fixture(72_000);
        let old = TempoMap::new(
            48_000,
            vec![
                TempoPoint::new(b(0), Tempo::new(60_000_000).unwrap()),
                TempoPoint::new(b(1), Tempo::DEFAULT),
            ],
        )
        .unwrap();
        let current = TempoMap::new(
            48_000,
            vec![
                TempoPoint::new(b(0), Tempo::DEFAULT),
                TempoPoint::new(b(5), Tempo::new(60_000_000).unwrap()),
            ],
        )
        .unwrap();
        let mut clip = editor.project().clip(id).unwrap().clone();
        let mut edit = AudioClipEdit::capture(&clip);
        Arc::make_mut(&mut edit.domain).clock = AudioDomainClock::Beats(b(2));
        Arc::make_mut(&mut edit.domain).fade_projection = Some(AudioFadeProjection {
            tempo: old.clone(),
            origin_beat_raw: 0,
            local_start_raw: 0,
        });
        edit.view_end = b(2).raw();
        edit.timing = AudioClipTiming::Musical(b(4));
        edit.processing_projection = Some(Arc::new(AudioProcessingProjection {
            tempo: Arc::new(old),
            origin_beat_raw: 0,
        }));
        clip.start = current.beat_to_frame(b(4)).unwrap();
        clip.edit = Some(Arc::new(edit.clone()));
        edit.validate(&clip, &current, 72_000).unwrap();
        let projection = AudioViewProjection::new(&clip, &current).unwrap();
        assert_eq!(
            projection.root_ranges_for_frames(96_000..120_000).unwrap(),
            [Some(0..BeatTime::from_ratio(1, 2).unwrap().raw()), None]
        );
    }
}
