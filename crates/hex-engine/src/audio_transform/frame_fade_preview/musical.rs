//! Forward musical event reads over the accepted pre-event peak clock.
use super::*;
use crate::PreparedAudioRepeat;
use crate::audio_transform::{
    PreparedEventOccurrence, PreparedMusicalEventSourceClock,
    events::event_source_frame,
    view::{bind_occurrence_clock, validate_copy},
};
use hex_project::{AudioClipEdit, AudioEventFade, BeatTime};
use std::borrow::Cow;
mod placement;
use placement::{ArrangementRead, repeat_ranges};

/// Control-only projection of a single forward musical processing root.
/// The arrangement reads project frames and the event editor reads frames in
/// the forward destination root. Both use composition's retained source clock.
pub struct MusicalEventFadePreview<'a> {
    clip: &'a Clip,
    edit: Cow<'a, AudioClipEdit>,
    tempo: &'a TempoMap,
    source: &'a PreparedMusicalEventSourceClock,
    root_frames: u64,
    read: ArrangementRead,
    clip_end: u64,
    events: Vec<MusicalEvent>,
    outer: Option<ProjectedOuterAudioFade<'a>>,
    gain: f32,
}

struct MusicalEvent {
    event: AudioEvent,
    frames: Range<u64>,
    affine_start: Option<u64>,
    fade: Option<AudioEventFade>,
    gain: f32,
}

impl<'a> MusicalEventFadePreview<'a> {
    pub fn new(
        clip: &'a Clip,
        tempo: &'a TempoMap,
        source: &'a PreparedMusicalEventSourceClock,
        events: &[AudioEvent],
        gain: ClipGain,
        repeat: Option<PreparedAudioRepeat>,
    ) -> Result<Option<Self>, Invalid> {
        let Some(edit) = Self::musical_edit(clip) else {
            return Ok(None);
        };
        let (_, root_frames, crop_start, crop_end) = edit.projected_domain(tempo)?;
        let clip_end = clip
            .start()
            .checked_add(clip.duration())
            .ok_or(Invalid("musical waveform clip end overflow"))?;
        let Some(read) = ArrangementRead::new(
            clip,
            edit,
            tempo,
            root_frames,
            crop_start..crop_end,
            clip_end,
            repeat,
        )?
        else {
            return Ok(None);
        };
        Self::from_root(clip, tempo, source, events, gain, Cow::Borrowed(edit), read)
    }

    /// Project the accepted forward root as it is laid out in the event pane.
    /// The other occurrences, and their Arrangement crop, are independent.
    pub fn for_editor(
        clip: &'a Clip,
        tempo: &'a TempoMap,
        source: &'a PreparedMusicalEventSourceClock,
        event: &AudioEvent,
        gain: ClipGain,
    ) -> Result<Option<Self>, Invalid> {
        let Some(edit) = Self::musical_edit(clip) else {
            return Ok(None);
        };
        Self::from_root(
            clip,
            tempo,
            source,
            std::slice::from_ref(event),
            gain,
            Cow::Borrowed(edit),
            ArrangementRead::EventOnly,
        )
    }

    /// Bind one destination occurrence to its accepted root clock and crop.
    /// The display owns metadata only and preserves the clip's outer fade.
    pub fn for_occurrence(
        clip: &'a Clip,
        tempo: &'a TempoMap,
        occurrence: &'a PreparedEventOccurrence,
        events: &[AudioEvent],
        gain: ClipGain,
    ) -> Result<Option<Self>, Invalid> {
        let Some(edit) = Self::musical_edit(clip) else {
            return Ok(None);
        };
        let Some(source) = occurrence.source.musical_clock() else {
            return Ok(None);
        };
        let placement = occurrence.placement;
        validate_copy(clip, placement)?;
        let edit =
            bind_occurrence_clock(edit, placement.root_origin, || Ok(source.shared_tempo()))?;
        if edit.projected_domain(tempo)?.1 != placement.root_frame_length {
            return Ok(None);
        }
        let read = ArrangementRead::Once {
            destination: placement.destination_start..placement.destination_end,
            root_start: placement.root_start,
        };
        Self::from_root(clip, tempo, source, events, gain, Cow::Owned(edit), read)
    }

    fn musical_edit(clip: &Clip) -> Option<&AudioClipEdit> {
        clip.audio_edit().filter(|edit| {
            matches!(edit.domain.clock, AudioDomainClock::Beats(_)) && edit.content_view.is_none()
        })
    }

    fn from_root(
        clip: &'a Clip,
        tempo: &'a TempoMap,
        source: &'a PreparedMusicalEventSourceClock,
        events: &[AudioEvent],
        gain: ClipGain,
        edit: Cow<'a, AudioClipEdit>,
        read: ArrangementRead,
    ) -> Result<Option<Self>, Invalid> {
        if events.len() > MAX_AUDIO_EVENTS
            || events.iter().any(|event| {
                event.source != 0
                    || event.start >= event.end
                    || event.end > edit.domain.clock.length()
            })
            || events.windows(2).any(|pair| pair[0].end > pair[1].start)
        {
            return Ok(None);
        }
        let (_, root_frames, _, _) = edit.projected_domain(tempo)?;
        let clip_end = clip
            .start()
            .checked_add(clip.duration())
            .ok_or(Invalid("musical waveform clip end overflow"))?;
        let outer = clip.projected_outer_fade(tempo)?;
        let zero = edit
            .domain
            .musical_event_source
            .map_or(0, |source| source.zero_in_destination);
        let aligned_zero = source.aligned_destination_zero(&edit, tempo, zero)?;
        if clip
            .audio_edit()
            .is_some_and(|edit| edit.placement.is_some())
            && aligned_zero.is_none()
        {
            return Ok(None);
        }
        let mut spans = Vec::new();
        spans
            .try_reserve_exact(events.len())
            .map_err(|_| Invalid("musical waveform event allocation failed"))?;
        for event in events.iter().copied().filter(|event| event.active) {
            let start = edit.root_frame_at(event.start, tempo)?;
            let end = edit.root_frame_at(event.end, tempo)?;
            if start == end {
                continue;
            }
            let affine_start = if zero.checked_add(event.source_start) == Some(event.start) {
                aligned_zero.and_then(|zero| start.checked_sub(zero))
            } else {
                None
            };
            let fade = event
                .fade
                .map(|fade| {
                    fade.reproject(|position| {
                        source
                            .frame_at(BeatTime::from_raw(position))
                            .map_err(|error| hex_project::Invalid(error.0))
                    })
                })
                .transpose()?;
            spans.push(MusicalEvent {
                event,
                frames: start..end,
                affine_start,
                fade,
                gain: event.gain.linear(),
            });
        }
        Ok(Some(Self {
            clip,
            edit,
            tempo,
            source,
            root_frames,
            read,
            clip_end,
            events: spans,
            outer,
            gain: gain.linear(),
        }))
    }

    /// Visit one visible column. Each event contributes one conservative source
    /// range and midpoint gains. Single-frame columns have the exact source
    /// coordinate and gains. Budget refusal requires discarding emitted ranges.
    pub fn visit_arrangement(
        &self,
        range: Range<u64>,
        remaining: &mut usize,
        visit: impl FnMut(FrameFadeRange),
    ) -> Result<Option<FrameFadeCoverage>, Invalid> {
        if range.start > range.end {
            return Err(Invalid("musical waveform range is reversed"));
        }
        let (read_start, read_end) = match &self.read {
            ArrangementRead::EventOnly => return Ok(None),
            ArrangementRead::Once { destination, .. } => (destination.start, destination.end),
            ArrangementRead::Repeat(_) => (self.clip.start(), self.clip_end),
        };
        let first = range.start.max(read_start);
        let last = range.end.min(read_end);
        if first >= last {
            return Ok(Some(FrameFadeCoverage {
                has_silence: !range.is_empty(),
            }));
        }
        let silence = first != range.start || last != range.end;
        match &self.read {
            ArrangementRead::EventOnly => Ok(None),
            ArrangementRead::Once { root_start, .. } => {
                let root_first = root_start + (first - read_start);
                let root = root_first..root_first + (last - first);
                self.visit(
                    root,
                    None,
                    silence,
                    remaining,
                    |midpoint| self.outer_gain(first + (midpoint - root_first)),
                    visit,
                )
            }
            ArrangementRead::Repeat(shape) => {
                // One coarse column can collapse several wraps. Its clip
                // envelope belongs to forward destination time.
                let gain = self.outer_gain(first + (last - first - 1) / 2)?;
                let elapsed = first - read_start..last - read_start;
                let mut has_silence = silence;
                let mut visit = visit;
                for root in repeat_ranges(*shape, elapsed).into_iter().flatten() {
                    let Some(coverage) =
                        self.visit(root, None, false, remaining, |_| Ok(gain), &mut visit)?
                    else {
                        return Ok(None);
                    };
                    has_silence |= coverage.has_silence;
                }
                Ok(Some(FrameFadeCoverage { has_silence }))
            }
        }
    }

    pub fn visit_event(
        &self,
        event: u64,
        range: Range<u64>,
        remaining: &mut usize,
        visit: impl FnMut(FrameFadeRange),
    ) -> Result<Option<FrameFadeCoverage>, Invalid> {
        if range.start > range.end {
            return Err(Invalid("musical waveform range is reversed"));
        }
        let end = range.end.min(self.root_frames);
        if range.start.min(end) == end {
            return Ok(Some(FrameFadeCoverage {
                has_silence: !range.is_empty(),
            }));
        }
        self.visit(
            range.start..end,
            Some(event),
            end != range.end,
            remaining,
            |_| Ok(1.0),
            visit,
        )
    }

    fn visit(
        &self,
        root: Range<u64>,
        event: Option<u64>,
        mut silence: bool,
        remaining: &mut usize,
        mut outer_gain: impl FnMut(u64) -> Result<f32, Invalid>,
        mut visit: impl FnMut(FrameFadeRange),
    ) -> Result<Option<FrameFadeCoverage>, Invalid> {
        let legacy = (self.outer.is_none() && !self.edit.domain.fade.is_neutral())
            .then(|| self.edit.projected_fade(self.tempo))
            .transpose()?;
        let mut cursor = root.start;
        let index = self
            .events
            .partition_point(|span| span.frames.end <= root.start);
        for span in self.events[index..]
            .iter()
            .take_while(|span| span.frames.start < root.end)
        {
            if event.is_some_and(|id| id != span.event.id) {
                continue;
            }
            if *remaining == 0 {
                return Ok(None);
            }
            *remaining -= 1;
            let first = root.start.max(span.frames.start);
            let last = root.end.min(span.frames.end);
            let midpoint = first + (last - first - 1) / 2;
            let source_first = self.source_frame(span, first)?;
            let source_end = self
                .source_frame(span, last - 1)?
                .checked_add(1)
                .ok_or(Invalid("musical waveform source end overflow"))?;
            let source_frame = self.source_frame(span, midpoint)?;
            let event_fade = match span.fade {
                Some(fade) => {
                    let age = source_frame
                        .checked_sub(fade.source_start())
                        .filter(|age| *age < fade.shape().span())
                        .ok_or(Invalid("musical waveform event exceeds its fade domain"))?;
                    fade.shape().gain_at(age)
                }
                None => 1.0,
            };
            let clip_fade = if let Some(legacy) = &legacy {
                legacy.gain_at(midpoint)?
            } else {
                outer_gain(midpoint)?
            };
            silence |= first > cursor;
            cursor = last;
            visit(FrameFadeRange {
                root: first..last,
                source: [Some(source_first..source_end), None],
                sample: FrameFadeSample {
                    source_frame,
                    event: Some(span.event.id),
                    gains: [event_fade, span.gain, clip_fade, self.gain],
                },
            });
        }
        Ok(Some(FrameFadeCoverage {
            has_silence: silence || cursor < root.end,
        }))
    }

    fn source_frame(&self, span: &MusicalEvent, frame: u64) -> Result<u64, Invalid> {
        if let Some(affine_start) = span.affine_start {
            return affine_start
                .checked_add(frame - span.frames.start)
                .ok_or(Invalid("audio event source position overflow"));
        }
        event_source_frame(&self.edit, self.tempo, &span.event, frame, |position| {
            self.source.frame_at(BeatTime::from_raw(position))
        })
    }

    fn outer_gain(&self, frame: u64) -> Result<f32, Invalid> {
        Ok(self
            .outer
            .as_ref()
            .map(|fade| fade.gain_at(frame))
            .transpose()?
            .unwrap_or(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_transform::{
        AudioTransformPreparation,
        tests::{fixture, prepare, render},
    };
    use hex_project::{AudioEventEdit, Command, WarpMode, default_audio_warp};

    #[test]
    fn musical_preview_matches_moved_event_pcm_across_a_tempo_seam() {
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
        let half = BeatTime::from_ratio(1, 2).unwrap();
        for command in [
            Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            },
            Command::SetTempoPoint {
                beat: half,
                tempo: hex_project::Tempo::new(240_000_000).unwrap(),
            },
        ] {
            editor.execute(command).unwrap();
        }
        let context = AudioTransformPreparation::default();
        let neutral = prepare(&editor, &media, &context);
        let neutral = &neutral.arrangement().audio_clip_views()[0];
        let source = neutral
            .event_source_peaks
            .as_ref()
            .unwrap()
            .musical_clock()
            .unwrap();
        let pcm = neutral.audio.frames();
        for operation in [
            AudioEventEdit::Split { at: half.raw() },
            AudioEventEdit::Move {
                event: 1,
                start: half.raw(),
            },
            AudioEventEdit::SetFade {
                event: 1,
                fade: Some(
                    AudioEventFade::new(
                        0,
                        ClipFade::new(half.raw(), 0, half.raw() / 2, 0).unwrap(),
                    )
                    .unwrap(),
                ),
            },
            AudioEventEdit::SetGain {
                event: 1,
                gain: ClipGain::new(3_000).unwrap(),
            },
        ] {
            editor
                .execute(Command::EditAudioEvents { clip, operation })
                .unwrap();
        }
        editor
            .execute(Command::SetClipFadeEnvelope {
                clip,
                fade: ClipFade::new(6_000, 0, 0, 357).unwrap(),
            })
            .unwrap();
        let project = editor.project();
        let clip = project.clip(clip).unwrap();
        let events = clip.audio_events();
        let query = MusicalEventFadePreview::new(
            clip,
            project.tempo_map(),
            source,
            &events.events,
            clip.gain(),
            None,
        )
        .unwrap()
        .unwrap();
        let accepted = prepare(&editor, &media, &context);
        let actual = render(&accepted, 0, 6_000, 79);
        for (frame, actual) in actual.into_iter().enumerate() {
            let frame = frame as u64;
            let mut sample = None;
            let coverage = query
                .visit_arrangement(frame..frame + 1, &mut 1, |part| {
                    assert!(sample.is_none());
                    assert_eq!(
                        part.source[0],
                        Some(part.sample.source_frame()..part.sample.source_frame() + 1)
                    );
                    sample = Some(part.sample);
                })
                .unwrap()
                .unwrap();
            assert_eq!(coverage.has_silence, frame < 4_000);
            assert_eq!(
                actual,
                sample.map_or([0.0; 2], |sample| sample
                    .apply_to(pcm[sample.source_frame() as usize])),
                "destination frame {frame}"
            );
        }
        let mut remaining = 1;
        let coverage = query
            .visit_arrangement(0..6_000, &mut remaining, |part| {
                assert_eq!(part.root, 4_000..6_000);
                assert_eq!(part.source[0], Some(0..3_999));
            })
            .unwrap()
            .unwrap();
        assert!(coverage.has_silence);
        assert_eq!(remaining, 0);
        assert!(
            query
                .visit_arrangement(4_000..6_000, &mut remaining, |_| panic!("budget exhausted"))
                .unwrap()
                .is_none()
        );
        query
            .visit_event(1, 4_001..4_002, &mut 1, |part| {
                assert_eq!(part.sample.source_frame(), 2);
            })
            .unwrap()
            .unwrap();
    }
}
