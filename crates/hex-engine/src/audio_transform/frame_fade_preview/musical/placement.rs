//! Bounded forward reads of one accepted musical processing root.
use super::*;
use crate::PreparedAudioRepeat;

pub(super) enum ArrangementRead {
    EventOnly,
    Once {
        destination: Range<u64>,
        root_start: u64,
    },
    Repeat(PreparedAudioRepeat),
}

impl ArrangementRead {
    pub(super) fn new(
        clip: &Clip,
        edit: &hex_project::AudioClipEdit,
        tempo: &TempoMap,
        root_frames: u64,
        crop: Range<u64>,
        clip_end: u64,
        repeat: Option<PreparedAudioRepeat>,
    ) -> Result<Option<Self>, Invalid> {
        if edit.placement.is_none() {
            return Ok(
                (crop.end - crop.start == clip.duration()).then_some(Self::Once {
                    destination: clip.start()..clip_end,
                    root_start: crop.start,
                }),
            );
        }
        let projection = AudioViewProjection::new(clip, tempo)?;
        if projection.occurrence_count() == 0 {
            return Ok(Some(Self::Once {
                destination: clip.start()..clip.start(),
                root_start: 0,
            }));
        }
        let first = projection.occurrence(0)?;
        if first.root_origin != edit.processing_origin()? || first.root_frame_length != root_frames
        {
            return Ok(None);
        }
        super::super::super::view::validate_copy(clip, first)?;
        if projection.occurrence_count() == 1 {
            return Ok(Some(Self::Once {
                destination: first.destination_start..first.destination_end,
                root_start: first.root_start,
            }));
        }
        Ok(repeat
            .filter(|shape| {
                shape.phase < shape.period
                    && shape
                        .offset
                        .checked_add(shape.period)
                        .is_some_and(|end| end <= root_frames)
                    && shape.offset.checked_add(shape.phase) == Some(first.root_start)
            })
            .map(Self::Repeat))
    }
}

/// Visit at most two root windows even when a column spans thousands of wraps.
pub(super) fn repeat_ranges(
    shape: PreparedAudioRepeat,
    elapsed: Range<u64>,
) -> [Option<Range<u64>>; 2] {
    let length = elapsed.end - elapsed.start;
    if length >= shape.period {
        return [Some(shape.offset..shape.offset + shape.period), None];
    }
    let start =
        ((u128::from(shape.phase) + u128::from(elapsed.start)) % u128::from(shape.period)) as u64;
    let first = length.min(shape.period - start);
    [
        Some(shape.offset + start..shape.offset + start + first),
        (first < length).then_some(shape.offset..shape.offset + length - first),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_transform::{
        AudioTransformPreparation,
        tests::{fixture, prepare, render},
    };
    use hex_project::{
        AudioEventEdit, AudioViewResizeOperation, Command, WarpMode, default_audio_warp,
    };

    #[test]
    fn phased_warped_event_preview_matches_shared_loop_and_finite_once() {
        let (mut editor, clip, media) = fixture(false);
        let track = editor.project().tracks()[0].id();
        let run = |editor: &mut hex_project::Editor, command| {
            editor.execute(command).unwrap();
        };
        run(
            &mut editor,
            Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            },
        );
        run(
            &mut editor,
            Command::MoveClip {
                clip,
                track,
                start: 16_000,
            },
        );
        let project = editor.project();
        let warp = default_audio_warp(
            project.clip(clip).unwrap(),
            project.tempo_map(),
            WarpMode::RePitch,
        )
        .unwrap();
        run(
            &mut editor,
            Command::SetAudioClipWarp {
                clip,
                settings: Some(warp),
            },
        );
        let context = AudioTransformPreparation::default();
        let neutral = prepare(&editor, &media, &context);
        let pcm = neutral.arrangement().audio_clip_views()[0].audio.frames();
        let beat = BeatTime::ONE.raw();
        let resize = |operation| Command::ResizeAudioClipView { clip, operation };
        for command in [
            Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::SetFade {
                    event: 1,
                    fade: Some(
                        AudioEventFade::new(0, ClipFade::new(beat, 0, beat / 2, beat / 8).unwrap())
                            .unwrap(),
                    ),
                },
            },
            resize(AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 40_000,
            }),
            resize(AudioViewResizeOperation::CycleRange {
                start: beat / 8,
                end: 7 * beat / 8,
                looping: true,
            }),
            resize(AudioViewResizeOperation::PlayStart {
                position: 3 * beat / 8,
            }),
            Command::SetClipFadeEnvelope {
                clip,
                fade: ClipFade::new(24_000, 0, 9_700, 7_500).unwrap(),
            },
        ] {
            run(&mut editor, command);
        }
        for looping in [true, false] {
            if !looping {
                run(
                    &mut editor,
                    Command::SetClipLooping {
                        clip,
                        looping: false,
                    },
                );
            }
            let accepted = prepare(&editor, &media, &context);
            let view = &accepted.arrangement().audio_clip_views()[0];
            assert!(view.repeat.is_none(), "outer clip fade materializes output");
            let project = editor.project();
            let clip = project.clip(clip).unwrap();
            let asset = project.asset(clip.asset()).unwrap();
            let event_clock = if looping {
                let repeat = view.event_source_repeat.unwrap();
                assert_eq!(
                    (repeat.offset, repeat.period, repeat.phase),
                    (1_000, 6_000, 2_000)
                );
                view.event_source_peaks.clone().unwrap()
            } else {
                assert!(view.event_source_repeat.is_none());
                context
                    .preview_root(project, clip.id(), &media[&asset.id()])
                    .unwrap()
                    .unwrap()
                    .event_source_peaks
                    .unwrap()
            };
            let events = clip.audio_events();
            let query = MusicalEventFadePreview::new(
                clip,
                project.tempo_map(),
                event_clock.musical_clock().unwrap(),
                &events.events,
                clip.gain(),
                view.event_source_repeat,
            )
            .unwrap()
            .unwrap();
            let audio = render(&accepted, clip.start(), clip.duration() as usize, 79);
            let mut saw_silence = false;
            for (index, expected) in audio.into_iter().enumerate() {
                let frame = clip.start() + index as u64;
                let mut sample = None;
                let coverage = query
                    .visit_arrangement(frame..frame + 1, &mut 1, |part| {
                        assert!(sample.is_none());
                        sample = Some(part.sample);
                    })
                    .unwrap()
                    .unwrap();
                saw_silence |= coverage.has_silence;
                assert_eq!(
                    expected,
                    sample.map_or([0.0; 2], |sample| sample
                        .apply_to(pcm[sample.source_frame() as usize])),
                    "frame {frame}"
                );
            }
            assert_eq!(saw_silence, !looping);
            let mut budget = 1;
            let whole = query
                .visit_arrangement(
                    clip.start()..clip.start() + clip.duration(),
                    &mut budget,
                    |_| {},
                )
                .unwrap()
                .unwrap();
            assert_eq!(whole.has_silence, !looping);
            assert_eq!(budget, 0, "whole repeat window consumes one span");
        }
    }
}
