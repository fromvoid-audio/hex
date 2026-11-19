use super::*;
use crate::{Audio, audio_transform::AudioTransformPreparation};
use hex_project::{
    AudioClipFadeClock, AudioEventEdit, AudioEventFade, AudioViewResizeOperation, ClipFade,
    ClipGain, Command, Created, Editor, FadeCurveLaw, Project,
};
use std::{collections::BTreeMap, sync::Arc};

fn fixture(frames: u64, looping: bool) -> (Editor, hex_project::ClipId, BTreeMap<AssetId, Audio>) {
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 32).unwrap();
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 44_100,
            frames,
        })
        .unwrap()
    else {
        panic!("import")
    };
    if looping {
        editor
            .execute(Command::SetClipLooping { clip, looping })
            .unwrap();
        editor
            .execute(Command::SetClipSourcePhase { clip, phase: 137 })
            .unwrap();
    }
    let audio = Audio::new(
        44_100,
        (0..frames)
            .map(|i| {
                [
                    ((i * 17 % 101) as f32 - 50.0) / 23.0,
                    ((i * 29 % 97) as f32 - 48.0) / 19.0,
                ]
            })
            .collect(),
    )
    .unwrap();
    (editor, clip, BTreeMap::from([(asset, audio)]))
}
fn prepare(
    editor: &Editor,
    media: &BTreeMap<AssetId, Audio>,
    context: &AudioTransformPreparation,
) -> crate::PreparedRevision {
    crate::PreparedRevision::prepare_with_audio_transforms(
        editor.project(),
        media,
        Arc::new(crate::SineTable::new()),
        None,
        context,
    )
    .unwrap()
}

#[test]
fn processed_frame_event_reads_apply_after_pitch_and_before_repeat_placement() {
    processed_frame_repeat_reads(false);
}

#[test]
fn reversed_processed_frame_event_reads_match_repeat_playback() {
    processed_frame_repeat_reads(true);
}

fn processed_frame_repeat_reads(reversed: bool) {
    let (mut editor, clip, media) = fixture(1_024, true);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let neutral = prepare(&editor, &media, &context);
    let source = &neutral.arrangement().audio_clip_views()[0].audio;
    assert_eq!(source.frames().len(), 1_024);

    for operation in [
        AudioEventEdit::Trim {
            event: 1,
            start: 128,
            end: 900,
        },
        AudioEventEdit::Move {
            event: 1,
            start: 64,
        },
        AudioEventEdit::SetFade {
            event: 1,
            fade: Some(AudioEventFade::new(128, ClipFade::new(772, 0, 79, 157).unwrap()).unwrap()),
        },
        AudioEventEdit::SetGain {
            event: 1,
            gain: ClipGain::new(3_200).unwrap(),
        },
    ] {
        editor
            .execute(Command::EditAudioEvents { clip, operation })
            .unwrap();
    }
    editor
        .execute(Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 2_417,
            },
        })
        .unwrap();
    editor
        .execute(Command::SetClipFadeEnvelope {
            clip,
            fade: ClipFade::new(2_417, 0, 123, 203).unwrap(),
        })
        .unwrap();
    if reversed {
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    }
    let project = editor.project();
    let row = project.clip(clip).unwrap();
    let asset = project.asset(row.asset()).unwrap();
    let request = |size| {
        FrameFadePreview::with_prepared_source(
            row,
            project.tempo_map(),
            asset,
            FrameFadeOverrides::default(),
            size,
        )
        .unwrap()
    };
    assert!(request(1_023).is_none());
    let query = request(source.frames().len() as u64).unwrap();
    let event = query.event(1).unwrap();
    assert_eq!(event.sample(139).unwrap().unwrap().source_frame(), 139);
    let mut remaining = 1;
    let coverage = event
        .visit_ranges(139..155, &mut remaining, |part| {
            assert_eq!(part.root, 75..91);
            assert_eq!(part.source, [Some(139..155), None]);
        })
        .unwrap()
        .unwrap();
    assert_eq!(remaining, 0);
    assert!(!coverage.has_silence);
    let actual = crate::audio_transform::tests::render(
        &prepare(&editor, &media, &context),
        row.start(),
        row.duration() as usize,
        79,
    );
    for (index, actual) in actual.into_iter().enumerate() {
        let sample = query.timeline_sample(row.start() + index as u64).unwrap();
        let expected = sample.map_or([0.0; 2], |sample| {
            sample.apply_to(source.frames()[sample.source_frame() as usize])
        });
        assert_eq!(
            actual, expected,
            "processed Repeat destination sample {index}"
        );
    }
}

#[test]
fn reversed_once_cropped_columns_keep_genuine_leading_silence() {
    reversed_once_columns(false);
}

#[test]
fn directed_legacy_once_envelope_intersects_both_silence_gates() {
    reversed_once_columns(true);
}

fn reversed_once_columns(legacy_fade: bool) {
    let (mut editor, clip, media) = fixture(1_024, false);
    if legacy_fade {
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 491,
                fade_out: 93,
            })
            .unwrap();
    }
    for command in [
        Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 1_424,
            },
        },
        Command::SetClipLooping {
            clip,
            looping: false,
        },
        Command::ReverseClips(vec![clip]),
        Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            frame: 200,
        },
    ] {
        editor.execute(command).unwrap();
    }
    if legacy_fade {
        let edit = editor.project().clip(clip).unwrap().audio_edit().unwrap();
        assert_eq!(edit.domain.fade_clock, AudioClipFadeClock::LegacyRoot);
        assert!(!edit.domain.fade.is_neutral());
    }
    assert_matches(&editor, clip, &media, &AudioTransformPreparation::default());
    let project = editor.project();
    let row = project.clip(clip).unwrap();
    let query = FrameFadePreview::new(
        row,
        project.tempo_map(),
        project.asset(row.asset()).unwrap(),
    )
    .unwrap()
    .unwrap();
    let common_columns = [
        (360..400, None, true),
        (360..440, Some(984..1_024), true),
        (420..460, Some(964..1_004), false),
    ];
    let trailing_columns = if legacy_fade {
        [
            (984..1_064, Some(400..440), true),
            (1_100..1_200, None, true),
        ]
    } else {
        [
            (984..1_064, Some(360..440), false),
            (1_100..1_200, Some(224..324), false),
        ]
    };
    for (range, expected, silence) in common_columns.into_iter().chain(trailing_columns) {
        let mut source = None;
        let coverage = query
            .visit_timeline_ranges(range, &mut 2, |part| {
                assert!(source.is_none());
                source = part.source[0].clone();
                assert!(part.source[1].is_none());
            })
            .unwrap()
            .unwrap();
        assert_eq!(source, expected);
        assert_eq!(coverage.has_silence, silence);
    }
}
fn assert_matches(
    editor: &Editor,
    clip: hex_project::ClipId,
    media: &BTreeMap<AssetId, Audio>,
    context: &AudioTransformPreparation,
) {
    let project = editor.project();
    let clip = project.clip(clip).unwrap();
    let source = &media[&clip.asset()];
    let mapping = FrameFadePreview::new(
        clip,
        project.tempo_map(),
        project.asset(clip.asset()).unwrap(),
    )
    .unwrap()
    .unwrap();
    let prepared = prepare(editor, media, context);
    let mut renderer = crate::Renderer::new(prepared.arrangement());
    renderer.seek(clip.start());
    renderer.play();
    let mut actual = vec![[0.0; 2]; clip.duration() as usize];
    for block in actual.chunks_mut(257) {
        renderer.render(block).unwrap();
    }
    for (elapsed, actual) in actual.into_iter().enumerate() {
        let query = mapping
            .timeline_sample(clip.start() + elapsed as u64)
            .unwrap();
        let expected = query.map_or([0.0; 2], |sample| {
            sample.apply_to(source.frames()[sample.source_frame() as usize])
        });
        assert_eq!(actual, expected, "frame {elapsed}, mapping {query:?}");
    }
    assert!(
        mapping
            .timeline_sample(clip.start() + clip.duration())
            .unwrap()
            .is_none()
    );
}

#[test]
fn plain_first_fade_and_event_split_move_trim_match_prepared_pcm() {
    for looping in [false, true] {
        let (mut editor, clip, media) = fixture(8_000, looping);
        let context = AudioTransformPreparation::default();
        assert!(editor.project().clip(clip).unwrap().audio_edit().is_none());
        assert_matches(&editor, clip, &media, &context);
        let mut trimmed = Editor::new(editor.project().clone(), 4).unwrap();
        trimmed
            .execute(Command::TrimClipEdge {
                clip,
                edge: hex_project::TrimEdge::Left,
                frame: 1_000,
            })
            .unwrap();
        assert_matches(&trimmed, clip, &media, &context);
        editor
            .execute(Command::SetClipFadeEnvelope {
                clip,
                fade: ClipFade::new(8_000, 0, 2_901, 3_701)
                    .unwrap()
                    .with_shape(31, 73, -327, 741)
                    .unwrap()
                    .with_curve_positions(217, 803)
                    .unwrap()
                    .with_curve_laws([FadeCurveLaw::SmoothStep; 2]),
            })
            .unwrap();
        assert_matches(&editor, clip, &media, &context);
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::SetFade {
                    event: 1,
                    fade: Some(
                        AudioEventFade::new(
                            0,
                            ClipFade::new(8_000, 0, 1_901, 2_907)
                                .unwrap()
                                .with_curve_laws([FadeCurveLaw::SmoothStep; 2]),
                        )
                        .unwrap(),
                    ),
                },
            })
            .unwrap();
        for operation in [
            AudioEventEdit::Split { at: 2_000 },
            AudioEventEdit::Move {
                event: 1,
                start: 4_000,
            },
            AudioEventEdit::Trim {
                event: 1,
                start: 4_250,
                end: 5_750,
            },
            AudioEventEdit::SetGain {
                event: 1,
                gain: ClipGain::new(6_001).unwrap(),
            },
        ] {
            editor
                .execute(Command::EditAudioEvents { clip, operation })
                .unwrap();
        }
        editor
            .execute(Command::SetAudioClipGain {
                clip,
                millidb: 1_113,
            })
            .unwrap();
        assert_matches(&editor, clip, &media, &context);
        let project = editor.project();
        let row = project.clip(clip).unwrap();
        let query = FrameFadePreview::new(
            row,
            project.tempo_map(),
            project.asset(row.asset()).unwrap(),
        )
        .unwrap()
        .unwrap();
        let event = row
            .audio_edit()
            .unwrap()
            .domain
            .events
            .as_ref()
            .unwrap()
            .events
            .iter()
            .find(|event| event.id == 1)
            .unwrap();
        assert_eq!(
            query
                .event(event.id)
                .unwrap()
                .sample(event.source_start)
                .unwrap(),
            query.root_sample(event.start).unwrap()
        );
        assert_eq!(
            query
                .event(event.id)
                .unwrap()
                .sample(event.source_start + event.length() - 1)
                .unwrap(),
            query.root_sample(event.end - 1).unwrap()
        );
        assert!(
            query
                .event(event.id)
                .unwrap()
                .sample(event.source_start + event.length())
                .unwrap()
                .is_none()
        );
        assert!(query.event(u64::MAX).is_none());
    }
}

#[test]
fn reported_four_repeat_window_keeps_original_source_and_fade_clocks() {
    let (mut editor, clip, media) = fixture(19_174, false);
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::SetFade {
                event: 1,
                fade: Some(
                    AudioEventFade::new(
                        0,
                        ClipFade::new(19_174, 0, 301, 3_787)
                            .unwrap()
                            .with_curve_laws([FadeCurveLaw::SmoothStep; 2]),
                    )
                    .unwrap(),
                ),
            },
        })
        .unwrap();
    editor
        .execute(Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 76_696,
            },
        })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let raw = prepare(&editor, &media, &context);
    let fade = ClipFade::new(76_696, 0, 13_184, 23_254).unwrap();
    editor
        .execute(Command::SetClipFadeEnvelope { clip, fade })
        .unwrap();
    let row = editor.project().clip(clip).unwrap();
    assert!(
        row.projected_outer_fade(editor.project().tempo_map())
            .unwrap()
            .is_some()
    );
    let mut raw_renderer = crate::Renderer::new(raw.arrangement());
    raw_renderer.play();
    let mut expected = vec![[0.0; 2]; 76_696];
    for block in expected.chunks_mut(317) {
        raw_renderer.render(block).unwrap();
    }
    for (frame, sample) in expected.iter_mut().enumerate() {
        *sample = sample.map(|value| value * fade.gain_at(frame as u64));
    }
    let actual = prepare(&editor, &media, &context);
    for size in [31, 256] {
        let mut renderer = crate::Renderer::new(actual.arrangement());
        renderer.play();
        let mut pcm = vec![[0.0; 2]; expected.len()];
        for block in pcm.chunks_mut(size) {
            renderer.render(block).unwrap();
        }
        assert_eq!(pcm, expected);
    }
    assert_matches(&editor, clip, &media, &context);
    let project = editor.project();
    let mapping = FrameFadePreview::new(
        row,
        project.tempo_map(),
        project.asset(row.asset()).unwrap(),
    )
    .unwrap()
    .unwrap();
    let root = mapping.root_sample(1_049).unwrap().unwrap();
    let first = mapping.timeline_sample(1_049).unwrap().unwrap();
    let repeated = mapping.timeline_sample(19_174 + 1_049).unwrap().unwrap();
    assert_eq!(first.source_frame(), repeated.source_frame());
    assert_eq!(&root.gains()[..2], &repeated.gains()[..2]);
    assert_eq!(root.gains()[2], 1.0);
    assert_eq!(first.gains()[2], fade.gain_at(1_049));
    assert_eq!(repeated.gains()[2], 1.0);
    let mut ranges = Vec::new();
    mapping
        .visit_timeline_ranges(1_000..1_100, &mut 8, |part| ranges.push(part))
        .unwrap()
        .unwrap();
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].sample.gains()[2], first.gains()[2]);
}

#[test]
fn source_mapping_does_not_consume_derived_pcm_admission_or_replace_silence() {
    let (mut editor, clip, media) = fixture(8_000, false);
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Delete {
                start: 2_000,
                end: 3_000,
            },
        })
        .unwrap();
    let context = AudioTransformPreparation {
        budget: Arc::new(crate::audio_transform::budget::Budget::new(128_000)),
        cache_limit: 64_000,
        entry_limit: 1,
        ..AudioTransformPreparation::default()
    };
    let prepared = prepare(&editor, &media, &context);
    assert_eq!(context.retained_pcm_bytes(), 64_000);
    let row = editor.project().clip(clip).unwrap();
    let query = FrameFadePreview::new(
        row,
        editor.project().tempo_map(),
        editor.project().asset(row.asset()).unwrap(),
    )
    .unwrap()
    .unwrap();
    assert!(query.timeline_sample(2_500).unwrap().is_none());
    assert!(query.timeline_sample(3_000).unwrap().is_some());
    assert_eq!(context.retained_pcm_bytes(), 64_000);
    assert_eq!(query.asset(), row.asset());
    assert_eq!(query.source_frames(), 8_000);
    drop(prepared);
}

#[test]
fn unsupported_processing_is_separate_from_supported_silence() {
    let (editor, clip, _) = fixture(8_000, false);
    for command in [
        Command::SetAudioClipPitch { clip, cents: 137 },
        Command::SetAudioClipMusical {
            clip,
            enabled: true,
        },
        Command::SetAudioClipSourceGain {
            clip,
            points: vec![hex_project::SourceGainPoint {
                frame: 0,
                gain_q16: 65_536,
            }],
        },
        Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Place {
                event: 1,
                start: 10_000,
            },
        },
    ] {
        let mut changed = Editor::new(editor.project().clone(), 8).unwrap();
        changed.execute(command).unwrap();
        let project = changed.project();
        let row = project.clip(clip).unwrap();
        assert!(
            FrameFadePreview::new(
                row,
                project.tempo_map(),
                project.asset(row.asset()).unwrap()
            )
            .unwrap()
            .is_none()
        );
    }
}

#[test]
fn directed_legacy_repeat_keeps_forward_phase_and_reflects_the_event() {
    let (mut editor, clip, media) = fixture(1_024, false);
    for command in [
        Command::SetClipFades {
            clip,
            fade_in: 157,
            fade_out: 205,
        },
        Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::SetFade {
                event: 1,
                fade: Some(
                    AudioEventFade::new(0, ClipFade::new(1_024, 0, 387, 49).unwrap()).unwrap(),
                ),
            },
        },
        Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 2_317,
            },
        },
        Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::PlayStart { position: 139 },
        },
        Command::ReverseClips(vec![clip]),
    ] {
        editor.execute(command).unwrap();
    }
    let edit = editor.project().clip(clip).unwrap().audio_edit().unwrap();
    assert_eq!(edit.domain.fade_clock, AudioClipFadeClock::LegacyRoot);
    assert!(!edit.domain.fade.is_neutral());
    assert_eq!(edit.placement.unwrap().cycle_phase, 139);
    assert_matches(&editor, clip, &media, &AudioTransformPreparation::default());
}

#[test]
fn ordered_gains_preserve_the_full_scale_boundary() {
    let sample = FrameFadeSample {
        source_frame: 0,
        event: Some(1),
        gains: [
            0.196_145_68,
            ClipGain::new(9_089).unwrap().linear(),
            1.0,
            1.0,
        ],
    };
    let source = 1.790_487_4;
    assert!(sample.apply_to([source, -source])[0] > 1.0);
    assert!(source * (sample.gains()[0] * sample.gains()[1]) <= 1.0);
}

#[test]
fn borrowed_fade_events_and_gain_match_first_fade_and_repeated_view_ack() {
    for repeated in [false, true] {
        let (mut editor, clip, media) = fixture(8_000, false);
        if repeated {
            editor
                .execute(Command::ResizeAudioClipView {
                    clip,
                    operation: AudioViewResizeOperation::CycleRange {
                        start: 1_000,
                        end: 7_000,
                        looping: true,
                    },
                })
                .unwrap();
        }
        let project = editor.project();
        let row = project.clip(clip).unwrap();
        let root_length = row
            .audio_edit()
            .map_or(row.duration(), |edit| edit.domain.clock.length());
        let fade = ClipFade::new(row.duration(), 0, 1_337, 2_753)
            .unwrap()
            .with_curve_laws([FadeCurveLaw::SmoothStep; 2]);
        let event_fade =
            AudioEventFade::new(0, ClipFade::new(root_length, 0, 2_113, 3_171).unwrap()).unwrap();
        // The real editor/command first materialize virtual crop fragments.
        // Resolve the visible event from that same basis before applying a fade.
        let editable = row.editable_audio_events().unwrap();
        let view_start = row.audio_edit().map_or(0, |edit| edit.view_start);
        let event = editable
            .events
            .iter()
            .find(|event| event.start == view_start)
            .unwrap()
            .id;
        let operation = AudioEventEdit::SetFade {
            event,
            fade: Some(event_fade),
        };
        let events = editable.edited(operation.clone(), root_length).unwrap();
        let gain = ClipGain::new(7_019).unwrap();
        let preview = FrameFadePreview::with_preview(
            row,
            project.tempo_map(),
            project.asset(row.asset()).unwrap(),
            FrameFadeOverrides {
                fade: Some(fade),
                legacy_root_fade: false,
                events: Some(&events),
                gain: Some(gain),
            },
        )
        .unwrap()
        .unwrap();
        let mut accepted = Editor::new(project.clone(), 8).unwrap();
        accepted
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        accepted
            .execute(Command::EditAudioEvents { clip, operation })
            .unwrap();
        accepted
            .execute(Command::SetAudioClipGain {
                clip,
                millidb: gain.millidb(),
            })
            .unwrap();
        assert_matches(
            &accepted,
            clip,
            &media,
            &AudioTransformPreparation::default(),
        );
        let accepted_row = accepted.project().clip(clip).unwrap();
        let canonical = FrameFadePreview::new(
            accepted_row,
            accepted.project().tempo_map(),
            accepted.project().asset(accepted_row.asset()).unwrap(),
        )
        .unwrap()
        .unwrap();
        for frame in 0..root_length {
            assert_eq!(
                preview.root_sample(frame).unwrap(),
                canonical.root_sample(frame).unwrap(),
                "root frame {frame}"
            );
        }
        for frame in row.start()..row.start() + row.duration() {
            assert_eq!(
                preview.timeline_sample(frame).unwrap(),
                canonical.timeline_sample(frame).unwrap(),
                "frame {frame}"
            );
        }
        assert!(
            FrameFadePreview::with_preview(
                row,
                project.tempo_map(),
                project.asset(row.asset()).unwrap(),
                FrameFadeOverrides {
                    fade: Some(ClipFade::neutral(1).unwrap()),
                    ..FrameFadeOverrides::default()
                }
            )
            .is_err()
        );
    }
}

#[test]
fn range_unions_cover_moves_phase_silence_and_budget_without_partial_emission() {
    let (mut editor, clip, media) = fixture(8_000, true);
    for operation in [
        AudioEventEdit::Split { at: 2_000 },
        AudioEventEdit::Move {
            event: 1,
            start: 4_000,
        },
        AudioEventEdit::Delete {
            start: 6_000,
            end: 6_500,
        },
    ] {
        editor
            .execute(Command::EditAudioEvents { clip, operation })
            .unwrap();
    }
    editor
        .execute(Command::ResizeAudioClipView {
            clip,
            operation: AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 32_000,
            },
        })
        .unwrap();
    let project = editor.project();
    let row = project.clip(clip).unwrap();
    let query = FrameFadePreview::new(
        row,
        project.tempo_map(),
        project.asset(row.asset()).unwrap(),
    )
    .unwrap()
    .unwrap();
    for range in [
        0..32_000,
        1_900..2_100,
        3_999..6_501,
        7_800..8_200,
        31_999..32_100,
    ] {
        let mut expected = std::collections::BTreeSet::new();
        let mut expected_silence = false;
        for frame in range.clone() {
            if let Some(sample) = query.timeline_sample(frame).unwrap() {
                expected.insert(sample.source_frame());
            } else {
                expected_silence = true;
            }
        }
        let mut visited = std::collections::BTreeSet::new();
        let mut remaining = 64;
        let coverage = query
            .visit_timeline_ranges(range, &mut remaining, |part| {
                assert_eq!(
                    part.sample,
                    query
                        .root_sample(part.root.start + (part.root.end - part.root.start - 1) / 2)
                        .unwrap()
                        .unwrap()
                );
                for source in part.source.into_iter().flatten() {
                    visited.extend(source);
                }
            })
            .unwrap()
            .unwrap();
        assert_eq!(visited, expected);
        assert_eq!(coverage.has_silence, expected_silence);
        assert!(remaining < 64);
    }
    let mut emitted = 0;
    let mut remaining = 1;
    assert!(
        query
            .visit_timeline_ranges(0..32_000, &mut remaining, |_| emitted += 1)
            .unwrap()
            .is_none()
    );
    assert_eq!((emitted, remaining), (0, 0));
    assert!(
        query
            .visit_timeline_ranges(0..1, &mut remaining, |_| emitted += 1)
            .unwrap()
            .is_none()
    );
    assert_eq!(emitted, 0);
    let event = query.event(1).unwrap();
    let mut remaining = 16;
    let coverage = event
        .visit_ranges(0..2_000, &mut remaining, |_| {})
        .unwrap()
        .unwrap();
    assert!(!coverage.has_silence);
    assert_eq!(media.values().next().unwrap().frames().len(), 8_000);
}

#[test]
fn once_and_exhausted_original_source_ranges_report_authentic_zero() {
    for once in [false, true] {
        let (mut editor, clip, _) = fixture(8_000, false);
        editor
            .execute(Command::ResizeAudioClipView {
                clip,
                operation: AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: 16_000,
                },
            })
            .unwrap();
        if once {
            editor
                .execute(Command::SetClipLooping {
                    clip,
                    looping: false,
                })
                .unwrap();
        }
        let project = editor.project();
        let row = project.clip(clip).unwrap();
        let query = FrameFadePreview::new(
            row,
            project.tempo_map(),
            project.asset(row.asset()).unwrap(),
        )
        .unwrap()
        .unwrap();
        let mut remaining = 16;
        let result = query
            .visit_timeline_ranges(0..16_000, &mut remaining, |_| {})
            .unwrap()
            .unwrap();
        assert_eq!(
            result.has_silence,
            (0..16_000).any(|frame| query.timeline_sample(frame).unwrap().is_none())
        );
    }
}
