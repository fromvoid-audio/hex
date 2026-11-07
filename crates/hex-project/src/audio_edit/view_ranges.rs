use super::*;
use std::ops::Range;

impl AudioViewProjection<'_> {
    /// First source cursor in the retained root's declared clock, independent
    /// of the destination frame and original media offset. Directed reads use
    /// their first sample/beat quantum; an exhausted once read has no cursor.
    /// This metadata query neither prepares audio nor transfers source custody.
    pub fn play_start_position(&self) -> Result<Option<u64>, Invalid> {
        if self.count == 0 {
            return Ok(None);
        }
        let Some((window, _, _)) = self.content else {
            return self
                .view_start
                .checked_add(if self.has_subframe_intro() {
                    0
                } else {
                    self.phase
                })
                .map(Some)
                .ok_or(Invalid("audio play start overflow"));
        };
        let read = window.read_position(0)?;
        if self.has_subframe_intro() && read < self.occurrence(0)?.elapsed_start {
            return Ok(None);
        }
        if let Some(edit) = self.clip.audio_edit().filter(|e| {
            e.processing_projection.is_some()
                && e.placement.is_none()
                && e.content_view.is_some_and(|v| v.base_placement.is_none())
        }) {
            let occurrence = self.occurrence(0)?;
            let frame = occurrence
                .root_start
                .checked_add(
                    self.base_frame_at_elapsed(read)?
                        .saturating_sub(occurrence.destination_start),
                )
                .ok_or(Invalid("audio play start frame overflow"))?;
            return edit.root_position_at(frame, self.tempo).map(Some);
        }
        let end = self
            .view_start
            .checked_add(self.cycle)
            .ok_or(Invalid("audio play start root overflow"))?;
        let looping = self
            .clip
            .audio_edit()
            .and_then(|e| e.content_view)
            .and_then(|v| v.base_placement)
            .filter(|p| p.mode == AudioViewMode::Repeat)
            .map(|_| (self.view_start, end));
        let at = u128::from(self.phase) + u128::from(read);
        Ok(
            crate::source_ranges::source_ranges(at, at + 1, self.view_start, end, looping)?
                .into_iter()
                .flatten()
                .next()
                .map(|range| range.start),
        )
    }

    /// Source-root position of one project frame in this finite placement.
    /// Returns no position outside its half-open bounds or in exhausted Once
    /// silence. Uses the retained root clock (not a media sample offset), with
    /// live/frozen tempo, repeat phase and directed reads matching range queries.
    /// Constant work, no allocation, DSP preparation or source custody transfer.
    pub fn root_position_for_frame(&self, frame: u64) -> Result<Option<u64>, Invalid> {
        if frame < self.placed_frames.0 || frame >= self.placed_frames.1 || self.count == 0 {
            return Ok(None);
        }
        let elapsed = self.elapsed_at_frame(frame)?;
        let leading = u64::try_from(self.first_read - self.start)
            .map_err(|_| Invalid("audio leading extent exceeds placement"))?;
        let audible_start = if self.has_subframe_intro() {
            self.occurrence(0)?.elapsed_start
        } else {
            0
        };
        let read = match self.content {
            Some((window, _, _)) => {
                // A rounded endpoint may invert one raw tick past its finite
                // window. It has no source quantum; never extrapolate a cursor.
                if elapsed >= window.length()? {
                    return Ok(None);
                }
                let read = window.read_position(elapsed)?;
                if read < audible_start {
                    return Ok(None);
                }
                read
            }
            None if leading > 0 => {
                let occurrence = self.occurrence(0)?;
                if !(occurrence.destination_start..occurrence.destination_end).contains(&frame) {
                    return Ok(None);
                }
                elapsed.saturating_sub(leading)
            }
            None => elapsed.max(audible_start),
        };
        if let Some(edit) = self.clip.audio_edit().filter(|edit| {
            edit.processing_projection.is_some()
                && edit.placement.is_none()
                && edit
                    .content_view
                    .is_none_or(|view| view.base_placement.is_none())
        }) {
            // A retained plain root reads its old output sample-for-sample,
            // even when the destination tempo has equal duration but different
            // interior timing. Directed reads first map back to that base.
            let occurrence = self.occurrence(0)?;
            let base = if self.content.is_some() {
                self.base_frame_at_elapsed(read)?
            } else {
                frame
            };
            let offset = base
                .checked_sub(occurrence.destination_start)
                .ok_or(Invalid("audio playback precedes retained occurrence"))?;
            let root_frame = occurrence
                .root_start
                .checked_add(offset)
                .ok_or(Invalid("audio playback root frame overflow"))?;
            return edit.root_position_at(root_frame, self.tempo).map(Some);
        }
        let end = self
            .view_start
            .checked_add(self.cycle)
            .ok_or(Invalid("audio playback root end overflow"))?;
        let placement = self.clip.audio_edit().and_then(|edit| {
            edit.content_view
                .map_or(edit.placement, |view| view.base_placement)
        });
        let looping = placement
            .filter(|p| p.mode == AudioViewMode::Repeat)
            .map(|_| (self.view_start, end));
        let at = u128::from(self.phase) + u128::from(read);
        Ok(
            crate::source_ranges::source_ranges(at, at + 1, self.view_start, end, looping)?
                .into_iter()
                .flatten()
                .next()
                .map(|range| range.start),
        )
    }

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
        let (read_start, read_end) = if self.first_read != self.start {
            let read = self.occurrence(0)?;
            (read.destination_start, read.destination_end)
        } else {
            self.placed_frames
        };
        let first = range.start.max(read_start);
        let last = range.end.min(read_end);
        if first >= last || self.count == 0 {
            return Ok([None, None]);
        }
        // A skipped sub-frame intro has no source visits, even though its
        // authored beat cursor precedes the first audible cycle.
        let audible_start = if self.has_subframe_intro() {
            self.occurrence(0)?.elapsed_start
        } else {
            0
        };
        if let Some((window, _, _)) = self.content {
            let first = self.elapsed_at_frame(first)?.min(window.length()?);
            let last = self.elapsed_at_frame(last)?.min(window.length()?);
            let read = window.read_range(first..last)?;
            let read = read.start.max(audible_start)..read.end;
            if read.start >= read.end {
                return Ok([None, None]);
            }
            if let Some(edit) = self.clip.audio_edit().filter(|e| {
                e.processing_projection.is_some()
                    && e.placement.is_none()
                    && e.content_view
                        .is_some_and(|view| view.base_placement.is_none())
            }) {
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
            .filter(|edit| edit.processing_projection.is_some() && edit.placement.is_none())
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
            Ok(raw.clamp(self.first_read, self.end))
        };
        let elapsed = |frame| -> Result<u128, Invalid> {
            u128::try_from(position(frame)? - self.first_read)
                .map_err(|_| Invalid("audio selection clock overflow"))
        };
        let start = u128::from(self.phase) + elapsed(first)?.max(u128::from(audible_start));
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
        assert_eq!(projection.play_start_position().unwrap(), Some(30));
        for (frame, expected) in [
            (999, None),
            (1000, Some(30)),
            (1015, Some(45)),
            (1030, Some(20)),
            (1099, Some(49)),
            (1100, None),
            (u64::MAX, None),
        ] {
            assert_eq!(projection.root_position_for_frame(frame).unwrap(), expected);
        }
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
        assert_eq!(projection.root_position_for_frame(1029).unwrap(), Some(59));
        assert_eq!(projection.root_position_for_frame(1030).unwrap(), None);
        assert_eq!(
            projection.root_ranges_for_frames(1025..1060).unwrap(),
            [Some(55..60), None]
        );
        assert_eq!(
            projection.root_ranges_for_frames(1030..1100).unwrap(),
            [None, None]
        );
        // Reversing an outer once placement can start in its silent tail.
        // A display cursor must not invent a source position there.
        edit.content_view = Some(crate::AudioContentView {
            restore_plain: false,
            base_view_start: edit.view_start,
            base_view_end: edit.view_end,
            base_placement: edit.placement,
            base_start_delta: 0,
            window: crate::ContentWindow::new(0, 100, true).unwrap(),
            read_resampler: crate::WarpResampler::default(),
        });
        edit.view_start = 0;
        edit.view_end = 100;
        edit.placement = None;
        clip.edit = Some(Arc::new(edit.clone()));
        edit.validate(&clip, tempo, 200).unwrap();
        assert_eq!(
            AudioViewProjection::new(&clip, tempo)
                .unwrap()
                .play_start_position()
                .unwrap(),
            None
        );
        let reversed = AudioViewProjection::new(&clip, tempo).unwrap();
        assert_eq!(reversed.root_position_for_frame(1000).unwrap(), None);
        assert_eq!(reversed.root_position_for_frame(1070).unwrap(), Some(59));
        assert_eq!(reversed.root_position_for_frame(1071).unwrap(), Some(58));
        assert_eq!(reversed.root_position_for_frame(1099).unwrap(), Some(30));
        edit.content_view.as_mut().unwrap().window.reversed = false;
        clip.edit = Some(Arc::new(edit));
        assert_eq!(
            AudioViewProjection::new(&clip, tempo)
                .unwrap()
                .play_start_position()
                .unwrap(),
            Some(30)
        );
    }

    #[test]
    fn play_start_follows_directed_first_quantum_without_media_offset_or_destination() {
        let (mut editor, id) = fixture(200);
        editor
            .execute(Command::TrimClip {
                clip: id,
                start: 1000,
                duration: 100,
                source_offset: 17,
                source_length: 100,
            })
            .unwrap();
        let query = |editor: &crate::Editor| {
            AudioViewProjection::new(
                editor.project().clip(id).unwrap(),
                editor.project().tempo_map(),
            )
            .unwrap()
            .play_start_position()
            .unwrap()
        };
        assert_eq!(query(&editor), Some(0));
        editor.execute(Command::ReverseClips(vec![id])).unwrap();
        assert_eq!(query(&editor), Some(99));
        let p = AudioViewProjection::new(
            editor.project().clip(id).unwrap(),
            editor.project().tempo_map(),
        )
        .unwrap();
        assert_eq!(p.root_position_for_frame(1025).unwrap(), Some(74));
        assert_eq!(p.root_position_for_frame(1099).unwrap(), Some(0));
        assert_eq!(p.root_position_for_frame(1100).unwrap(), None);
        editor.execute(Command::ReverseClips(vec![id])).unwrap();
        assert_eq!(query(&editor), Some(0));
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
        assert_eq!(projection.play_start_position().unwrap(), Some(b(3).raw()));
        for (frame, expected) in [
            (tempo.beat_to_frame(b(6)).unwrap(), b(5).raw()),
            (tempo.beat_to_frame(b(7)).unwrap(), b(2).raw()),
        ] {
            assert_eq!(
                projection.root_position_for_frame(frame).unwrap(),
                Some(expected)
            );
        }
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
            projection.root_position_for_frame(548_000).unwrap(),
            Some(b(5).raw())
        );
        assert_eq!(
            projection.root_position_for_frame(572_000).unwrap(),
            Some(b(2).raw())
        );
        assert_eq!(
            projection.root_ranges_for_frames(548_000..596_000).unwrap(),
            [Some(b(2).raw()..b(3).raw()), Some(b(5).raw()..b(6).raw())]
        );
    }
    #[test]
    fn retained_outer_ranges_wrap_later_cycles_and_exclude_once_silence() {
        let (mut editor, id) = fixture(192_000);
        editor
            .execute(Command::SetAudioClipMusical {
                clip: id,
                enabled: true,
            })
            .unwrap();
        let old = editor.project().tempo_map().clone();
        let current = TempoMap::new(
            48_000,
            vec![
                crate::TempoPoint::new(b(0), Tempo::new(90_000_000).unwrap()),
                crate::TempoPoint::new(b(20), Tempo::DEFAULT),
            ],
        )
        .unwrap();
        let mut clip = editor.project().clip(id).unwrap().clone();
        let mut edit = clip.audio_edit().unwrap().clone();
        edit.view_start = b(2).raw();
        edit.view_end = b(6).raw();
        edit.timing = AudioClipTiming::Musical(b(20));
        edit.placement = Some(AudioViewPlacement {
            mode: AudioViewMode::Repeat,
            outer_length: AudioDomainClock::Beats(b(6)),
            cycle_phase: b(1).raw(),
        });
        edit.processing_projection = Some(Arc::new(AudioProcessingProjection {
            tempo: Arc::new(old),
            origin_beat_raw: i128::from(b(1).raw()),
        }));
        clip.start = current.beat_to_frame(b(20)).unwrap();
        clip.duration = 144_000;
        clip.fade = crate::ClipFade::neutral(clip.duration).unwrap();
        clip.edit = Some(Arc::new(edit.clone()));
        edit.validate(&clip, &current, 192_000).unwrap();
        let projection = AudioViewProjection::new(&clip, &current).unwrap();
        assert_eq!(
            projection
                .root_ranges_for_frames(clip.start + 48_000..clip.start + 96_000)
                .unwrap(),
            [Some(b(2).raw()..b(3).raw()), Some(b(5).raw()..b(6).raw())]
        );
        assert_eq!(
            projection
                .root_position_for_frame(clip.start + 48_000)
                .unwrap(),
            Some(b(5).raw())
        );
        assert_eq!(
            projection
                .root_position_for_frame(clip.start + 72_000)
                .unwrap(),
            Some(b(2).raw())
        );
        edit.placement.as_mut().unwrap().mode = AudioViewMode::Once;
        clip.edit = Some(Arc::new(edit.clone()));
        let projection = AudioViewProjection::new(&clip, &current).unwrap();
        assert_eq!(
            projection
                .root_position_for_frame(clip.start + 72_000)
                .unwrap(),
            None
        );
        assert_eq!(
            projection
                .root_ranges_for_frames(clip.start + 72_000..clip.start + 144_000)
                .unwrap(),
            [None, None]
        );
        let gate = current.beat_to_frame(b(19)).unwrap();
        let extended = clip.with_musical_leading_silence(gate, &current).unwrap();
        extended
            .audio_edit()
            .unwrap()
            .validate(&extended, &current, 192_000)
            .unwrap();
        let leading = AudioViewProjection::new(&extended, &current).unwrap();
        assert_eq!(
            leading.outer_frame_bounds(),
            (gate, clip.start + clip.duration)
        );
        let old = projection.occurrence(0).unwrap();
        let new = leading.occurrence(0).unwrap();
        assert_eq!(
            new,
            AudioViewOccurrence {
                elapsed_start: new.elapsed_start,
                elapsed_end: old.elapsed_end + new.elapsed_start,
                ..old
            }
        );
        let gate = current.beat_to_frame(b(16)).unwrap();
        let trimmed =
            super::super::commands::trim_view(&clip, crate::TrimEdge::Left, gate, &current)
                .unwrap();
        trimmed
            .audio_edit()
            .unwrap()
            .validate(&trimmed, &current, 192_000)
            .unwrap();
        let view = AudioViewProjection::new(&trimmed, &current).unwrap();
        let read = view.occurrence(0).unwrap();
        assert_eq!(
            view.outer_frame_bounds(),
            (gate, clip.start + clip.duration)
        );
        assert_eq!(
            read.destination_start,
            old.destination_start - old.root_start
        );
        assert_eq!(read.destination_end, old.destination_end);
        assert_eq!(read.root_origin, old.root_origin);
        assert_eq!(read.root_start, 0);
        edit.placement_end_offset = 1;
        clip.edit = Some(Arc::new(edit.clone()));
        assert!(
            edit.validate(&clip, &current, 192_000).is_err(),
            "saved destination residuals remain authoritative"
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
            projection.root_position_for_frame(108_000).unwrap(),
            Some(BeatTime::from_ratio(1, 4).unwrap().raw())
        );
        assert_eq!(
            projection.root_ranges_for_frames(96_000..120_000).unwrap(),
            [Some(0..BeatTime::from_ratio(1, 2).unwrap().raw()), None]
        );
    }
}
