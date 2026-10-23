//! Global tempo changes preserve beat onsets and complete retained audio timing.
use hex_project::{
    AudioClipTiming, BeatTime, ClipId, Command, Created, Editor, Project, Tempo, TrimEdge,
    WarpMode, default_audio_warp,
};

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
fn colliding_global_tempo_refuses_before_prepare_and_preserves_redo() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let clip = audio(&mut editor, 0, None);
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(3),
            length: BeatTime::ONE,
        })
        .unwrap();
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
    assert!(result.unwrap_err().0.contains("overlap"));
    assert!(!prepared);
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
    assert_eq!(editor.project().clip(clip).unwrap().duration(), 48_000);
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
fn retained_warped_sections_refuse_global_rebase_before_prepare_without_losing_redo() {
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
                Ok(())
            },
        )
        .unwrap_err();
    assert!(error.0.contains("rebase"));
    assert!(!prepared);
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
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
