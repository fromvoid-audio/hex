//! Global tempo changes preserve beat onsets and complete retained audio timing.
use hex_project::{
    AudioClipTiming, AudioViewResizeOperation, BeatTime, ClipFade, ClipId, Command, Created,
    Editor, Project, SourceGainPoint, Tempo, TrimEdge, WarpMode, default_audio_warp,
};
use std::sync::Arc;

fn bpm(n: u32) -> Tempo {
    Tempo::new(n * 1_000_000).unwrap()
}
fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn audio(editor: &mut Editor, start: u64, warp: Option<bool>) -> ClipId {
    let Created::Imported { track, clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::MoveClip { clip, track, start })
        .unwrap();
    if let Some(enabled) = warp {
        let settings = default_audio_warp(
            editor.project().clip(clip).unwrap(),
            editor.project().tempo_map(),
            WarpMode::RePitch,
        )
        .unwrap();
        editor
            .execute(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings),
            })
            .unwrap();
        if !enabled {
            editor
                .execute(Command::SetAudioClipWarp {
                    clip,
                    settings: None,
                })
                .unwrap();
        }
    }
    clip
}

#[test]
fn global_map_onsets_durations_and_complete_history_are_one_edit() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let ordinary = audio(&mut editor, 48_000, None);
    let warped = audio(&mut editor, 48_000, Some(true));
    let off = audio(&mut editor, 48_000, Some(false));
    editor
        .execute(Command::SetTempoPoint {
            beat: beat(8),
            tempo: bpm(90),
        })
        .unwrap();
    let before = editor.project().clone();
    editor
        .execute(Command::SetProjectTempo { tempo: bpm(60) })
        .unwrap();
    let changed = editor.project().clone();
    assert_eq!(changed.tempo_map().points().len(), 1);
    for n in [0, 2, 8, 100] {
        assert_eq!(changed.tempo_map().tempo_at(beat(n)), bpm(60));
    }
    for (id, duration) in [(ordinary, 48_000), (warped, 96_000), (off, 48_000)] {
        let old = before.clip(id).unwrap();
        let new = changed.clip(id).unwrap();
        assert_eq!((new.start(), new.duration()), (96_000, duration));
        assert_eq!(
            (
                new.asset(),
                new.source_offset(),
                new.source_length(),
                new.source_phase()
            ),
            (
                old.asset(),
                old.source_offset(),
                old.source_length(),
                old.source_phase()
            )
        );
    }
    assert!(matches!(
        changed.clip(off).unwrap().audio_edit().unwrap().timing,
        AudioClipTiming::Frozen { .. }
    ));
    assert_eq!(Project::from_snapshot(changed.snapshot()).unwrap(), changed);
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(editor.project(), &changed);
}

#[test]
fn colliding_global_tempo_crops_audio_tail_before_prepare_and_preserves_complete_history() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let clip = audio(&mut editor, 0, None);
    let track = editor.project().tracks()[0].id();
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(3),
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip")
    };
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let mut prepared = false;
    let result = editor.execute_prepared::<_, hex_project::Invalid>(
        Command::SetProjectTempo { tempo: bpm(240) },
        |_| {
            prepared = true;
            Ok(())
        },
    );
    result.unwrap();
    assert!(prepared);
    let changed = editor.project().clone();
    let cropped = changed.clip(clip).unwrap();
    assert_eq!((cropped.start(), cropped.duration()), (0, 36_000));
    assert_eq!(cropped.asset(), before.clip(clip).unwrap().asset());
    assert_eq!(changed.midi_clip(midi).unwrap().start(), beat(3));
    assert_eq!(changed.tempo_map().beat_to_frame(beat(3)).unwrap(), 36_000);
    assert_eq!(changed.tracks()[0].clips().len(), 1);
    assert_eq!(changed.tracks()[0].midi_clips().len(), 1);
    assert_eq!(Project::from_snapshot(changed.snapshot()).unwrap(), changed);
    assert!(!editor.can_redo());
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(editor.project(), &changed);
}

#[test]
fn faster_global_tempo_crops_only_colliding_prefixes_without_replacing_roots_or_onsets() {
    for tempo in [bpm(240), Tempo::new(333_333_333).unwrap()] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
        let plain = audio(&mut editor, 0, None);
        let track = editor.project().tracks()[0].id();
        let off = audio(&mut editor, 48_000, Some(false));
        let repeated = audio(&mut editor, 96_000, None);
        editor
            .execute(Command::SetClipLooping {
                clip: repeated,
                looping: true,
            })
            .unwrap();
        editor
            .execute(Command::SetClipSourcePhase {
                clip: repeated,
                phase: 137,
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch {
                clip: repeated,
                cents: 123,
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipSourceGain {
                clip: repeated,
                points: vec![
                    SourceGainPoint {
                        frame: 0,
                        gain_q16: 65_536,
                    },
                    SourceGainPoint {
                        frame: 24_000,
                        gain_q16: 32_768,
                    },
                    SourceGainPoint {
                        frame: 48_000,
                        gain_q16: 65_536,
                    },
                ],
            })
            .unwrap();
        editor
            .execute(Command::TrimClipEdge {
                clip: repeated,
                edge: TrimEdge::Right,
                frame: 120_000,
            })
            .unwrap();
        editor
            .execute(Command::ResizeAudioClipView {
                clip: repeated,
                operation: AudioViewResizeOperation::RepeatOuterEnd {
                    project_frame: 144_000,
                },
            })
            .unwrap();
        let warped = audio(&mut editor, 144_000, Some(true));
        let last = audio(&mut editor, 240_000, None);
        for (clip, start) in [
            (off, 48_000),
            (repeated, 96_000),
            (warped, 144_000),
            (last, 240_000),
        ] {
            editor
                .execute(Command::MoveClip { clip, track, start })
                .unwrap();
        }
        let Created::Clip(midi) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(8),
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("midi")
        };
        for clip in [plain, off, repeated] {
            let fade = ClipFade::new(48_000, 0, 997, 1_601)
                .unwrap()
                .with_shape(31, 47, 300, -400)
                .unwrap();
            editor
                .execute(Command::SetClipFadeEnvelope { clip, fade })
                .unwrap();
        }
        let before = editor.project().clone();
        editor.execute(Command::SetProjectTempo { tempo }).unwrap();
        let changed = editor.project();
        let map = changed.tempo_map();
        let ids = [plain, off, repeated, warped, last];
        assert_eq!(
            changed.tracks()[0]
                .clips()
                .iter()
                .map(|c| c.id())
                .collect::<Vec<_>>(),
            ids
        );
        assert_eq!(changed.tracks()[0].midi_clips()[0].id(), midi);
        assert_eq!(
            changed.assets().collect::<Vec<_>>(),
            before.assets().collect::<Vec<_>>()
        );
        assert_eq!(changed.snapshot().next_id, before.snapshot().next_id);
        assert_eq!(changed.midi_clip(midi), before.midi_clip(midi));
        for (id, onset, next) in [(plain, 0, 2), (off, 2, 4), (repeated, 4, 6)] {
            let old = before.clip(id).unwrap();
            let clip = changed.clip(id).unwrap();
            let start = map.beat_to_frame(beat(onset)).unwrap();
            let end = map.beat_to_frame(beat(next)).unwrap();
            assert_eq!(
                (clip.start(), clip.duration()),
                (start, end - start),
                "{id:?} at {tempo:?}"
            );
            assert_eq!(
                (
                    clip.asset(),
                    clip.source_offset(),
                    clip.source_phase(),
                    clip.gain()
                ),
                (
                    old.asset(),
                    old.source_offset(),
                    old.source_phase(),
                    old.gain()
                )
            );
            assert_eq!(clip.retained_fade(), old.retained_fade());
            if let Some(root) = old.audio_edit() {
                let edit = clip.audio_edit().unwrap();
                assert!(
                    Arc::ptr_eq(&edit.domain, &root.domain),
                    "cropping must retain the full processing root"
                );
                assert_eq!(edit.view_start, root.view_start);
                if let AudioClipTiming::Musical(start) = &root.timing {
                    assert!(matches!(&edit.timing,
                        AudioClipTiming::Frozen { reference_start_raw, tempo }
                        if *reference_start_raw == i128::from(start.raw())
                            && tempo.as_ref() == before.tempo_map()));
                } else {
                    assert_eq!(edit.timing, root.timing);
                }
                if let Some(placement) = root.placement {
                    assert_eq!(edit.view_end, root.view_end);
                    let shortened = edit.placement.unwrap();
                    assert_eq!(
                        (shortened.mode, shortened.cycle_phase),
                        (placement.mode, placement.cycle_phase)
                    );
                }
            }
        }
        let clip = changed.clip(warped).unwrap();
        let start = map.beat_to_frame(beat(6)).unwrap();
        assert_eq!(
            (clip.start(), clip.duration()),
            (start, map.beat_to_frame(beat(8)).unwrap() - start)
        );
        assert_eq!(clip.audio_edit(), before.clip(warped).unwrap().audio_edit());
        assert_eq!(
            (
                changed.clip(last).unwrap().start(),
                changed.clip(last).unwrap().duration()
            ),
            (map.beat_to_frame(beat(10)).unwrap(), 48_000)
        );
        assert_eq!(
            Project::from_snapshot(changed.snapshot()).unwrap(),
            *changed
        );
    }
}

#[test]
fn global_tempo_quantized_identical_positive_starts_refuse_without_prepare_or_history_loss() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let first = audio(&mut editor, 0, None);
    editor
        .execute(Command::TrimClipEdge {
            clip: first,
            edge: TrimEdge::Right,
            frame: 1,
        })
        .unwrap();
    let source = editor.project().clip(first).unwrap();
    let Created::Clip(second) = editor
        .execute(Command::PlaceClip {
            track: editor.project().tracks()[0].id(),
            asset: source.asset(),
            start: 1,
            duration: 1,
            source_offset: 1,
            source_length: 1,
            looping: false,
        })
        .unwrap()
    else {
        panic!("second")
    };
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let projected = hex_project::TempoMap::constant(48_000, bpm(333)).unwrap();
    assert_eq!(
        projected
            .beat_to_frame(before.tempo_map().frame_to_beat(1).unwrap())
            .unwrap(),
        0
    );
    let mut prepared = false;
    let error = editor
        .execute_prepared::<_, hex_project::Invalid>(
            Command::SetProjectTempo { tempo: bpm(333) },
            |_| {
                prepared = true;
                Ok(())
            },
        )
        .unwrap_err();
    assert!(
        error.0.contains("same frame")
            || error.0.contains("same start")
            || error.0.contains("quantiz"),
        "{error}"
    );
    assert!(!prepared);
    assert_eq!(editor.project(), &before);
    assert!(editor.project().clip(first).is_some());
    assert!(editor.project().clip(second).is_some());
    assert!(editor.can_redo());
}

#[test]
fn frozen_crop_can_move_and_extend_left_into_negative_reference_then_thaw() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let clip = audio(&mut editor, 0, Some(true));
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 12_000,
        })
        .unwrap();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 0,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: None,
        })
        .unwrap();
    editor
        .execute(Command::SetProjectTempo { tempo: bpm(60) })
        .unwrap();
    let origin = editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .projection_origin()
        .unwrap();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 48_000,
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 36_000,
        })
        .unwrap();
    let grown = editor.project().clip(clip).unwrap();
    let edit = grown.audio_edit().unwrap();
    assert_eq!(
        (grown.start(), grown.duration(), edit.view_start),
        (36_000, 48_000, 0)
    );
    assert_eq!(edit.projection_origin().unwrap(), origin);
    assert!(
        matches!(edit.timing, AudioClipTiming::Frozen { reference_start_raw, .. } if reference_start_raw < 0)
    );
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
    let settings =
        default_audio_warp(grown, editor.project().tempo_map(), WarpMode::RePitch).unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
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
        AudioClipTiming::Musical(_)
    ));
}

#[test]
fn retained_warped_sections_follow_global_tempo_with_atomic_preparation_and_history() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    audio(&mut editor, 0, Some(true));
    let mut add = |start, end| {
        let Created::ArrangementSection(id) = editor
            .execute(Command::AddArrangementSection {
                name: "part".into(),
                color: hex_project::ElementColor::Rose,
                start: BeatTime::from_raw(BeatTime::ONE.raw() * start / 4),
                end: BeatTime::from_raw(BeatTime::ONE.raw() * end / 4),
            })
            .unwrap()
        else {
            panic!("section")
        };
        id
    };
    let source = add(1, 2);
    let target = add(3, 4);
    editor
        .execute(Command::ReorderSectionBefore {
            section: source,
            before: target,
        })
        .unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    let mut prepared = false;
    let error = editor
        .execute_prepared::<_, hex_project::Invalid>(
            Command::SetProjectTempo { tempo: bpm(60) },
            |_| {
                prepared = true;
                Err::<(), _>(hex_project::Invalid("injected preparation refusal"))
            },
        )
        .unwrap_err();
    assert_eq!(error.0, "injected preparation refusal");
    assert!(prepared);
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
    editor
        .execute(Command::SetProjectTempo { tempo: bpm(180) })
        .unwrap();
    let changed = editor.project().clone();
    for old in before.tracks()[0].clips() {
        let current = changed.clip(old.id()).unwrap();
        let old_edit = old.audio_edit().unwrap();
        let edit = current.audio_edit().unwrap();
        assert!(old_edit.processing_projection.is_some());
        assert!(edit.processing_projection.is_none());
        assert_eq!(
            (edit.placement_start_offset, edit.placement_end_offset),
            (0, 0)
        );
        assert!(Arc::ptr_eq(&edit.domain, &old_edit.domain));
        assert_eq!(
            (edit.view_start, edit.view_end, &edit.timing),
            (old_edit.view_start, old_edit.view_end, &old_edit.timing)
        );
        let start = edit.live_musical_start().unwrap();
        let end = BeatTime::from_raw(start.raw() + edit.view_end - edit.view_start);
        assert_eq!(
            current.start(),
            changed.tempo_map().beat_to_frame(start).unwrap()
        );
        assert_eq!(
            current.start() + current.duration(),
            changed.tempo_map().beat_to_frame(end).unwrap()
        );
    }
    assert!(!editor.can_redo());
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(editor.project(), &changed);
    // Redundant equal-tempo cut anchors do not prevent a legal current-format
    // retained state from having a single-point map. Its exact no-op is allowed.
    let mut single = before.snapshot();
    single.tempo_points = vec![hex_project::TempoPoint::new(BeatTime::ZERO, bpm(120))];
    let mut plain = Editor::new(Project::from_snapshot(single).unwrap(), 16).unwrap();
    plain.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(plain.undo());
    let before = plain.project().clone();
    plain
        .execute(Command::SetProjectTempo { tempo: bpm(120) })
        .unwrap();
    assert_eq!(plain.project(), &before);
    assert!(plain.can_redo());
}

#[test]
fn both_tempo_edits_keep_the_loop_brace_on_its_beats_and_undo_restores_it() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let frames = |editor: &Editor, n| editor.project().tempo_map().beat_to_frame(beat(n)).unwrap();
    // A four-bar brace from bar 2 at the default 120 bpm.
    let (start, end) = (frames(&editor, 4), frames(&editor, 20));
    editor
        .execute(Command::SetLoopRegion {
            start,
            end,
            enabled: true,
        })
        .unwrap();
    let original = editor.project().loop_region().unwrap();
    for command in [
        Command::SetProjectTempo { tempo: bpm(90) },
        Command::SetTempoPoint {
            beat: beat(2),
            tempo: bpm(60),
        },
    ] {
        let before = editor.project().loop_region().unwrap();
        editor.execute(command).unwrap();
        let region = editor.project().loop_region().unwrap();
        let map = editor.project().tempo_map();
        assert_eq!(map.frame_to_beat(region.start()).unwrap(), beat(4));
        assert_eq!(map.frame_to_beat(region.end()).unwrap(), beat(20));
        assert_eq!(region.start(), frames(&editor, 4));
        assert_eq!(region.end(), frames(&editor, 20));
        assert!(region.enabled());
        assert_ne!(region, before, "the tempo edit moved the brace's frames");
    }
    let retimed = editor.project().loop_region();
    assert!(editor.undo());
    assert!(editor.undo());
    assert_eq!(editor.project().loop_region(), Some(original));
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(editor.project().loop_region(), retimed);
}

#[test]
fn tempo_point_keeps_unwarped_audio_and_midi_at_the_same_beat_together() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let at_beat_eight = editor.project().tempo_map().beat_to_frame(beat(8)).unwrap();
    let clip = audio(&mut editor, at_beat_eight, None);
    audio(&mut editor, 0, None);
    let midi_track = editor.project().tracks()[1].id();
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track: midi_track,
            start: beat(8),
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi clip")
    };
    let before = editor.project().clone();
    editor
        .execute(Command::SetTempoPoint {
            beat: beat(2),
            tempo: bpm(60),
        })
        .unwrap();
    let changed = editor.project();
    let midi_start = changed
        .tempo_map()
        .beat_to_frame(changed.midi_clip(midi).unwrap().start())
        .unwrap();
    // Two beats at 120 bpm, then six beats at 60 bpm.
    assert_eq!(midi_start, 24_000 * 2 + 48_000 * 6);
    assert_eq!(changed.clip(clip).unwrap().start(), midi_start);
    assert_eq!(
        changed.clip(clip).unwrap().asset(),
        before.clip(clip).unwrap().asset()
    );
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
}
