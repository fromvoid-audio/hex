//! Actual PCM acceptance of the canonical finite placement law.
use super::*;
use hex_project::{AudioClipTiming, AudioViewMode, AudioViewPlacement, ClipFade};

fn placed(
    mode: AudioViewMode,
    phase: u64,
) -> (
    Editor,
    hex_project::ClipId,
    BTreeMap<hex_project::AssetId, Audio>,
) {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 1 })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 0 })
        .unwrap();
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let row = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    let edit = row.edit.as_mut().unwrap();
    edit.view_start = 1_000;
    edit.view_end = 3_000;
    edit.placement = Some(AudioViewPlacement {
        mode,
        outer_length: AudioDomainClock::Frames(5_200),
        cycle_phase: phase,
    });
    row.duration = 5_200;
    row.fade = ClipFade::neutral(row.duration).unwrap();
    (
        Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap(),
        clip,
        media,
    )
}

fn musical_placement(
    mode: WarpMode,
) -> (
    Editor,
    hex_project::ClipId,
    BTreeMap<hex_project::AssetId, Audio>,
) {
    let (mut editor, clip, media) = fixture(false);
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        mode,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![
                SourceGainPoint {
                    frame: 0,
                    gain_q16: 65_536,
                },
                SourceGainPoint {
                    frame: 4_000,
                    gain_q16: 16_384,
                },
                SourceGainPoint {
                    frame: 8_000,
                    gain_q16: 65_536,
                },
            ],
        })
        .unwrap();
    for (raw, bpm) in [
        (BeatTime::ONE.raw() / 2, 90_000_000),
        (BeatTime::ONE.raw(), 180_000_000),
    ] {
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_raw(raw),
                tempo: Tempo::new(bpm).unwrap(),
            })
            .unwrap();
    }
    let mut snapshot = editor.project().snapshot();
    let row = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    let edit = row.edit.as_mut().unwrap();
    let quarter = BeatTime::ONE.raw();
    edit.view_start = quarter / 8;
    edit.view_end = quarter * 5 / 8;
    edit.timing = AudioClipTiming::Musical(BeatTime::from_raw(quarter / 4));
    edit.placement = Some(AudioViewPlacement {
        mode: AudioViewMode::Repeat,
        outer_length: AudioDomainClock::Beats(BeatTime::from_raw(quarter * 5 / 4)),
        cycle_phase: quarter / 8,
    });
    row.start = editor
        .project()
        .tempo_map()
        .beat_to_frame(BeatTime::from_raw(quarter / 4))
        .unwrap();
    row.duration = editor
        .project()
        .tempo_map()
        .beat_to_frame(BeatTime::from_raw(quarter * 3 / 2))
        .unwrap()
        - row.start;
    row.fade = ClipFade::neutral(row.duration).unwrap();
    (
        Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap(),
        clip,
        media,
    )
}

#[test]
fn musical_audio_placement_uses_each_tempo_root_without_restarting_warp_gain_or_fades() {
    for mode in [WarpMode::RePitch, WarpMode::Tones { grain_ms: 40 }] {
        let (editor, clip, media) = musical_placement(mode);
        let canonical = editor.project().clip(clip).unwrap();
        let projection =
            hex_project::AudioViewProjection::new(canonical, editor.project().tempo_map()).unwrap();
        assert_eq!(projection.occurrence_count(), 3);
        let mut expected = vec![[0.0; 2]; canonical.duration() as usize];
        // Independent oracle: prepare and render each full root through the
        // unchanged placement=None path, then crop its literal audible PCM.
        for index in 0..projection.occurrence_count() {
            let occurrence = projection.occurrence(index).unwrap();
            let origin = u64::try_from(occurrence.root_origin).unwrap();
            let mut snapshot = editor.project().snapshot();
            let row = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
            let edit = row.edit.as_mut().unwrap();
            edit.placement = None;
            edit.processing_projection = None;
            edit.view_start = 0;
            edit.view_end = edit.domain.clock.length();
            edit.timing = AudioClipTiming::Musical(BeatTime::from_raw(origin));
            row.start = editor
                .project()
                .tempo_map()
                .beat_to_frame(BeatTime::from_raw(origin))
                .unwrap();
            row.duration = occurrence.root_frame_length;
            row.fade = ClipFade::neutral(row.duration).unwrap();
            let root = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
            let root_pcm = render(
                &prepare(&root, &media, &AudioTransformPreparation::default()),
                row_start(&root, clip),
                occurrence.root_frame_length as usize,
                31,
            );
            let first = (occurrence.destination_start - canonical.start()) as usize;
            let last = (occurrence.destination_end - canonical.start()) as usize;
            expected[first..last].copy_from_slice(
                &root_pcm[occurrence.root_start as usize..occurrence.root_end as usize],
            );
        }
        let context = AudioTransformPreparation::default();
        let actual = prepare(&editor, &media, &context);
        for block in [1, 127, 257] {
            assert_eq!(
                render(&actual, canonical.start(), expected.len(), block),
                expected,
                "{mode:?} block={block}"
            );
            assert_eq!(
                render(&actual, canonical.start() + 2_503, 997, block),
                expected[2_503..3_500]
            );
        }
        let sidecar = &actual.arrangement().audio_clip_views()[0];
        assert_eq!((sidecar.offset, sidecar.length), (0, canonical.duration()));
        assert_eq!(sidecar.audio.frames(), expected);
        let again = prepare(&editor, &media, &context);
        assert!(
            sidecar
                .audio
                .shares_pcm_with(&again.arrangement().audio_clip_views()[0].audio)
        );
    }
}

fn row_start(editor: &Editor, clip: hex_project::ClipId) -> u64 {
    editor.project().clip(clip).unwrap().start()
}

#[test]
fn placement_cache_keys_phase_and_retains_audible_outer_pcm_after_root_eviction() {
    let (editor, _, media) = placed(AudioViewMode::Repeat, 500);
    let context = isolated(200_000, 64_000, 1);
    let budget = Arc::clone(&context.budget);
    let first = prepare(&editor, &media, &context);
    let original = render(&first, 0, 5_200, 127);
    let mut snapshot = editor.project().snapshot();
    snapshot.tracks[0].audio_clips_mut().unwrap()[0]
        .edit
        .as_mut()
        .unwrap()
        .placement
        .as_mut()
        .unwrap()
        .cycle_phase = 750;
    let changed = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    let second = prepare(&changed, &media, &context);
    assert!(
        !first.arrangement().audio_clip_views()[0]
            .audio
            .shares_pcm_with(&second.arrangement().audio_clip_views()[0].audio)
    );
    assert_ne!(render(&second, 0, 5_200, 257), original);
    assert_eq!(render(&first, 0, 5_200, 31), original);
    assert_eq!(context.cached_pcm_bytes().unwrap(), 5_200 * 8);
    assert_eq!(budget.used(), 2 * 5_200 * 8);
    drop(first);
    assert_eq!(budget.used(), 5_200 * 8);
    drop(second);
    drop(context);
    assert_eq!(budget.used(), 0);
}

#[test]
fn placement_bounds_and_cancellation_refuse_without_warming_or_leaking_pcm() {
    let (editor, clip, media) = placed(AudioViewMode::Repeat, 500);
    for (length, cycle) in [
        (hex_project::MAX_AUDIO_DOMAIN_FRAMES + 1, 2_000),
        (8_193, 1),
    ] {
        let mut snapshot = editor.project().snapshot();
        let row = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
        let edit = row.edit.as_mut().unwrap();
        edit.view_end = edit.view_start + cycle;
        let placement = edit.placement.as_mut().unwrap();
        placement.cycle_phase = 0;
        placement.outer_length = AudioDomainClock::Frames(length);
        row.duration = length;
        row.fade = ClipFade::neutral(length).unwrap();
        let project = Project::from_snapshot(snapshot).unwrap();
        let context = isolated(200_000, 64_000, 1);
        assert!(
            crate::PreparedRevision::prepare_with_audio_transforms(
                &project,
                &media,
                Arc::new(crate::SineTable::new()),
                None,
                &context,
            )
            .is_err()
        );
        assert_eq!(
            (
                context.retained_pcm_bytes(),
                context.cached_pcm_bytes().unwrap()
            ),
            (0, 0)
        );
    }
    let (musical, _, _) = musical_placement(WarpMode::RePitch);
    let mut snapshot = musical.project().snapshot();
    let row = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    let edit = row.edit.as_mut().unwrap();
    edit.view_start = 0;
    edit.view_end = BeatTime::ONE.raw() / 256;
    edit.timing = AudioClipTiming::Musical(BeatTime::ZERO);
    let placement = edit.placement.as_mut().unwrap();
    placement.cycle_phase = 0;
    placement.outer_length = AudioDomainClock::Beats(BeatTime::from_quarters(16).unwrap());
    row.start = 0;
    row.duration = musical
        .project()
        .tempo_map()
        .beat_to_frame(BeatTime::from_quarters(16).unwrap())
        .unwrap();
    row.fade = ClipFade::neutral(row.duration).unwrap();
    let project = Project::from_snapshot(snapshot).unwrap();
    let context = isolated(200_000, 64_000, 1);
    let error = crate::PreparedRevision::prepare_with_audio_transforms(
        &project,
        &media,
        Arc::new(crate::SineTable::new()),
        None,
        &context,
    )
    .unwrap_err();
    assert_eq!(
        error.0,
        "audio placement exceeds bounded full-root preparation work"
    );
    assert_eq!(
        (
            context.retained_pcm_bytes(),
            context.cached_pcm_bytes().unwrap()
        ),
        (0, 0)
    );
    let context = isolated(200_000, 64_000, 1);
    context.cancel_after_polls.set(Some(6));
    assert!(
        crate::PreparedRevision::prepare_with_audio_transforms(
            editor.project(),
            &media,
            Arc::new(crate::SineTable::new()),
            None,
            &context,
        )
        .is_err()
    );
    assert_eq!(
        (
            context.retained_pcm_bytes(),
            context.cached_pcm_bytes().unwrap()
        ),
        (0, 0)
    );
    assert!(editor.project().clip(clip).is_some());
}

#[test]
fn frozen_audio_placement_preserves_pcm_through_tempo_move_and_trim() {
    let (mut editor, clip, media) = musical_placement(WarpMode::RePitch);
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: None,
        })
        .unwrap();
    let start = row_start(&editor, clip);
    let length = editor.project().clip(clip).unwrap().duration() as usize;
    let context = AudioTransformPreparation::default();
    let expected = render(&prepare(&editor, &media, &context), start, length, 127);
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    assert!(matches!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .timing,
        AudioClipTiming::Frozen { .. }
    ));
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 32_000,
        })
        .unwrap();
    assert_eq!(
        render(&prepare(&editor, &media, &context), 32_000, length, 257),
        expected
    );
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            frame: 32_731,
        })
        .unwrap();
    assert_eq!(
        render(
            &prepare(&editor, &media, &context),
            32_731,
            length - 731,
            31
        ),
        expected[731..]
    );
}

#[test]
fn finite_audio_placement_renders_repeat_phase_partial_cycle_and_once_silence() {
    for (mode, phase) in [
        (AudioViewMode::Repeat, 500),
        (AudioViewMode::Once, 500),
        (AudioViewMode::Once, 3_000),
    ] {
        let (editor, clip, media) = placed(mode, phase);
        let before = editor.project().snapshot();
        let source = media.values().next().unwrap().frames();
        let expected = (0..5_200)
            .map(|elapsed| {
                let clock = phase + elapsed;
                if mode == AudioViewMode::Repeat || clock < 2_000 {
                    source[(1_000 + clock % 2_000) as usize]
                } else {
                    [0.0; 2]
                }
            })
            .collect::<Vec<_>>();
        let context = AudioTransformPreparation::default();
        let prepared = prepare(&editor, &media, &context);
        for block in [1, 127, 257] {
            let actual = render(&prepared, 0, expected.len(), block);
            let first_mismatch = actual.iter().zip(&expected).position(|(a, b)| a != b);
            assert_eq!(
                first_mismatch, None,
                "mode={mode:?} phase={phase} block={block}"
            );
            assert_eq!(render(&prepared, 1_497, 701, block), expected[1_497..2_198]);
        }
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.project().clip(clip).is_some());
    }
}

#[test]
fn accepted_preview_root_keeps_hidden_pcm_and_exhausted_once_source_under_the_transform_owner() {
    for (mode, phase) in [(AudioViewMode::Repeat, 500), (AudioViewMode::Once, 2500)] {
        let (editor, clip, media) = placed(mode, phase);
        let source = &media[&editor.project().clip(clip).unwrap().asset()];
        let transforms = AudioTransformPreparation::default();
        let first = transforms
            .preview_root(editor.project(), clip, source)
            .unwrap()
            .unwrap();
        let again = transforms
            .preview_root(editor.project(), clip, source)
            .unwrap()
            .unwrap();
        assert!(
            first.audio.shares_pcm_with(&again.audio),
            "root query uses the existing cache"
        );
        assert_eq!(first.audio.frames().len(), 8000);
        assert_eq!(
            first.audio.frames(),
            source.frames(),
            "root keeps samples outside the visible cycle and Once cursor"
        );
        let wrong = Audio::new(44100, vec![[0.0; 2]; 8000]).unwrap();
        assert!(
            transforms
                .preview_root(editor.project(), clip, &wrong)
                .is_err()
        );
    }
}

#[test]
fn section_outer_placements_preserve_pcm_through_splices_reorders_and_repeated_cuts() {
    for variant in 0..6 {
        let musical = variant != 0;
        for mode in [AudioViewMode::Once, AudioViewMode::Repeat] {
            let (mut seed, clip, media) = if musical {
                musical_placement(WarpMode::RePitch)
            } else {
                placed(mode, 500)
            };
            if musical {
                let mut snapshot = seed.project().snapshot();
                snapshot.tracks[0].audio_clips_mut().unwrap()[0]
                    .edit
                    .as_mut()
                    .unwrap()
                    .placement
                    .as_mut()
                    .unwrap()
                    .mode = mode;
                seed = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
            }
            if variant >= 2 {
                for (beat, bpm) in [
                    (0, 123_456_789),
                    (8, 97_333_331),
                    (16, 177_777_779),
                    (20, 83_123_457),
                ] {
                    seed.execute(Command::SetTempoPoint {
                        beat: BeatTime::from_ratio(beat, 16).unwrap(),
                        tempo: Tempo::new(bpm).unwrap(),
                    })
                    .unwrap();
                }
            }
            if variant == 3 || variant == 5 {
                seed.execute(Command::ReverseClips(vec![clip])).unwrap();
            }
            if variant == 4 {
                seed.execute(Command::SetAudioClipWarp {
                    clip,
                    settings: None,
                })
                .unwrap();
                let mut snapshot = seed.project().snapshot();
                let edit = snapshot.tracks[0].audio_clips_mut().unwrap()[0]
                    .edit
                    .as_mut()
                    .unwrap();
                edit.timing = AudioClipTiming::Frozen {
                    reference_start_raw: edit.reference_start_raw().unwrap(),
                    tempo: Arc::new(seed.project().tempo_map().clone()),
                };
                seed = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
            }
            if variant == 5 {
                let frame = seed
                    .project()
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_ratio(21, 16).unwrap())
                    .unwrap();
                seed.execute(Command::TrimClipEdge {
                    clip,
                    edge: hex_project::TrimEdge::Right,
                    frame,
                })
                .unwrap();
            }
            let mut add = |start, end| {
                let Created::ArrangementSection(id) = seed
                    .execute(Command::AddArrangementSection {
                        name: "part".into(),
                        color: hex_project::ElementColor::Cyan,
                        start: BeatTime::from_ratio(start, 16).unwrap(),
                        end: BeatTime::from_ratio(end, 16).unwrap(),
                    })
                    .unwrap()
                else {
                    panic!("section")
                };
                id
            };
            // Cuts cross both a partial cycle and a later repeat (or Once silence).
            let section = add(5, 15);
            let target = add(17, 23);
            let old = seed.project().tempo_map();
            let s = old
                .beat_to_frame(BeatTime::from_ratio(5, 16).unwrap())
                .unwrap() as usize;
            let e = old
                .beat_to_frame(BeatTime::from_ratio(15, 16).unwrap())
                .unwrap() as usize;
            let t = old
                .beat_to_frame(BeatTime::from_ratio(23, 16).unwrap())
                .unwrap() as usize;
            let n = t + 12_000;
            let context = AudioTransformPreparation::default();
            let source = render(&prepare(&seed, &media, &context), 0, n, 113);
            for operation in 0..6 {
                let mut editor = Editor::new(seed.project().clone(), 32).unwrap();
                let before = editor.project().snapshot();
                let mut expected = source.clone();
                let command = match operation {
                    0 => {
                        expected.splice(e..e, source[s..e].iter().copied());
                        Command::DuplicateArrangementSection(section)
                    }
                    1 => {
                        expected.drain(s..e);
                        Command::DeleteArrangementSection(section)
                    }
                    2 => {
                        expected.splice(e..e, vec![[0.0; 2]; e - s]);
                        Command::ResizeArrangementSection {
                            section,
                            edge: hex_project::TrimEdge::Right,
                            length: BeatTime::from_ratio(20, 16).unwrap(),
                        }
                    }
                    3 => {
                        let removed = old
                            .beat_to_frame(BeatTime::from_ratio(10, 16).unwrap())
                            .unwrap() as usize;
                        expected.drain(removed..e);
                        Command::ResizeArrangementSection {
                            section,
                            edge: hex_project::TrimEdge::Right,
                            length: BeatTime::from_ratio(5, 16).unwrap(),
                        }
                    }
                    4 => {
                        expected.drain(s..e);
                        expected.splice(t - (e - s)..t - (e - s), source[s..e].iter().copied());
                        Command::ReorderSectionAfter {
                            section,
                            after: target,
                        }
                    }
                    _ => {
                        let begin = old
                            .beat_to_frame(BeatTime::from_ratio(17, 16).unwrap())
                            .unwrap() as usize;
                        expected.drain(begin..t);
                        expected.splice(s..s, source[begin..t].iter().copied());
                        Command::ReorderSectionBefore {
                            section: target,
                            before: section,
                        }
                    }
                };
                editor.execute(command).unwrap_or_else(|error| {
                    panic!("variant={variant} mode={mode:?} operation={operation}: {error}")
                });
                if operation == 2 {
                    let marker = editor.project().arrangement_section(section).unwrap();
                    let end = editor
                        .project()
                        .tempo_map()
                        .beat_to_frame(marker.end())
                        .unwrap() as usize;
                    expected = source.clone();
                    expected.splice(e..e, vec![[0.0; 2]; end - e]);
                }
                for block in [31, 257] {
                    let actual = render(
                        &prepare(&editor, &media, &context),
                        0,
                        expected.len(),
                        block,
                    );
                    assert!(
                        actual == expected,
                        "variant={variant} mode={mode:?} operation={operation} first mismatch={:?}, max difference={}",
                        actual.iter().zip(&expected).position(|(a, b)| a != b),
                        actual
                            .iter()
                            .zip(&expected)
                            .flat_map(|(a, b)| [(a[0] - b[0]).abs(), (a[1] - b[1]).abs()])
                            .fold(0.0_f32, f32::max)
                    );
                }
                let after = editor.project().snapshot();
                assert_eq!(
                    Project::from_snapshot(after.clone()).unwrap().snapshot(),
                    after
                );
                assert!(editor.undo());
                let mut undone = editor.project().snapshot();
                undone.next_id = before.next_id;
                assert_eq!(undone, before);
                assert!(editor.redo());
                assert_eq!(editor.project().snapshot(), after);
                // A second cut must retain the first edit's saved processing origin.
                let marker = editor.project().arrangement_section(target).unwrap();
                let clock = editor.project().tempo_map();
                let a = clock.beat_to_frame(marker.start()).unwrap() as usize;
                let b = clock.beat_to_frame(marker.end()).unwrap() as usize;
                let mut twice = expected.clone();
                twice.splice(b..b, expected[a..b].iter().copied());
                editor
                    .execute(Command::DuplicateArrangementSection(target))
                    .unwrap();
                let actual = render(&prepare(&editor, &media, &context), 0, twice.len(), 127);
                assert!(
                    actual == twice,
                    "second edit variant={variant} mode={mode:?} operation={operation} mismatch={:?}",
                    actual.iter().zip(&twice).position(|(a, b)| a != b)
                );
                Project::from_snapshot(editor.project().snapshot()).unwrap();
                if musical {
                    editor
                        .execute(Command::SetProjectTempo {
                            tempo: Tempo::new(101_111_113).unwrap(),
                        })
                        .unwrap();
                    prepare(&editor, &media, &context);
                }
            }
        }
    }
}
