//! Literal render preservation across a global edit, including a tempo-seam event.
use super::*;
use hex_project::{AudioEventEdit, ClipFade};

#[test]
fn global_tempo_preserves_frozen_event_and_curved_fade_pcm_through_move_split_copy_and_history() {
    let (mut editor, clip, media) = fixture(true);
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
    let half = BeatTime::from_raw(BeatTime::ONE.raw() / 2);
    editor
        .execute(Command::SetTempoPoint {
            beat: half,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: None,
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: half.raw() },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Move {
                event: 1,
                start: half.raw(),
            },
        })
        .unwrap();
    editor
        .execute(Command::SetClipFadeEnvelope {
            clip,
            fade: ClipFade::new(12_000, 0, 500, 1000)
                .unwrap()
                .with_shape(100, 200, 600, -500)
                .unwrap(),
        })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let before = render(&prepare(&editor, &media, &context), 0, 12_000, 63);
    // The moved event spans the slower side of the seam. Even destination
    // frames advance only one source frame; endpoint-only conversion fails this.
    let source = media.values().next().unwrap().frames();
    for i in 0..16 {
        assert_eq!(before[4000 + 2 * i], source[137 + i]);
    }
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().duration(), 12_000);
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 12_000, 257),
        before
    );
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 32_000,
        })
        .unwrap();
    let previous = editor.project().clone();
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let start = editor.project().clip(clip).unwrap().start();
    assert_eq!(start, 21_333);
    assert_eq!(
        render(&prepare(&editor, &media, &context), start, 12_000, 1),
        before
    );
    assert_eq!(
        render(
            &prepare(&editor, &media, &AudioTransformPreparation::default()),
            start,
            12_000,
            127
        ),
        before
    );
    let changed = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(editor.project(), &previous);
    assert!(editor.redo());
    assert_eq!(editor.project(), &changed);
    let Created::Clip(right) = editor
        .execute(Command::SplitClip {
            clip,
            frame: start + 2001,
        })
        .unwrap()
    else {
        panic!("split")
    };
    assert_eq!(
        render(&prepare(&editor, &media, &context), start, 12_000, 64),
        before
    );
    let Created::Clip(copied) = editor
        .execute(Command::CopyClipTo {
            clip: right,
            track,
            start: 50_000,
        })
        .unwrap()
    else {
        panic!("copy")
    };
    let length = editor.project().clip(copied).unwrap().duration() as usize;
    assert_eq!(length, 9999);
    assert_eq!(
        render(&prepare(&editor, &media, &context), 50_000, length, 63),
        before[2001..]
    );
}

#[test]
fn faster_global_tempo_crops_adjacent_audio_without_changing_retained_prefix_pcm() {
    for repeated_frozen_root in [false, true] {
        let (mut editor, first, media) = fixture(repeated_frozen_root);
        let track = editor.project().tracks()[0].id();
        let asset = editor.project().clip(first).unwrap().asset();
        if repeated_frozen_root {
            editor
                .execute(Command::SetAudioClipPitch {
                    clip: first,
                    cents: 123,
                })
                .unwrap();
            editor
                .execute(Command::SetAudioClipSourceGain {
                    clip: first,
                    points: vec![
                        SourceGainPoint {
                            frame: 0,
                            gain_q16: 65_536,
                        },
                        SourceGainPoint {
                            frame: 4_000,
                            gain_q16: 32_768,
                        },
                        SourceGainPoint {
                            frame: 8_000,
                            gain_q16: 65_536,
                        },
                    ],
                })
                .unwrap();
            let settings = default_audio_warp(
                editor.project().clip(first).unwrap(),
                editor.project().tempo_map(),
                WarpMode::RePitch,
            )
            .unwrap();
            editor
                .execute(Command::SetAudioClipWarp {
                    clip: first,
                    settings: Some(settings),
                })
                .unwrap();
            editor
                .execute(Command::SetAudioClipWarp {
                    clip: first,
                    settings: None,
                })
                .unwrap();
            editor
                .execute(Command::TrimClipEdge {
                    clip: first,
                    edge: hex_project::TrimEdge::Right,
                    frame: 1_000,
                })
                .unwrap();
            editor
                .execute(Command::ResizeAudioClipView {
                    clip: first,
                    operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd {
                        project_frame: 8_000,
                    },
                })
                .unwrap();
        }
        let mut ids = vec![first];
        for start in [8_000, 16_000] {
            let Created::Clip(id) = editor
                .execute(Command::PlaceClip {
                    track,
                    asset,
                    start,
                    duration: 8_000,
                    source_offset: 0,
                    // The middle one-shot has an intentional silent tail. A
                    // prefix crop beyond that source end must remain legal.
                    source_length: if start == 8_000 { 1_000 } else { 8_000 },
                    looping: false,
                })
                .unwrap()
            else {
                panic!("audio")
            };
            ids.push(id);
        }
        let before = editor.project().clone();
        let context = AudioTransformPreparation::default();
        let original = prepare(&editor, &media, &context);
        editor
            .execute(Command::SetProjectTempo {
                tempo: Tempo::new(333_333_333).unwrap(),
            })
            .unwrap();
        let changed = editor.project().clone();
        let warm = prepare(&editor, &media, &context);
        let cold = prepare(&editor, &media, &AudioTransformPreparation::default());
        for (index, id) in ids.iter().copied().enumerate() {
            let old = before.clip(id).unwrap();
            let current = changed.clip(id).unwrap();
            let expected_start = changed
                .tempo_map()
                .beat_to_frame(before.tempo_map().frame_to_beat(old.start()).unwrap())
                .unwrap();
            assert_eq!(current.start(), expected_start);
            let expected_length = if let Some(next) = ids.get(index + 1) {
                changed.clip(*next).unwrap().start() - current.start()
            } else {
                8_000
            };
            assert_eq!(current.duration(), expected_length);
            let expected = render(&original, old.start(), expected_length as usize, 31);
            if index == 1 {
                assert!(expected[..1_000].iter().any(|frame| *frame != [0.0; 2]));
                assert!(expected[1_000..].iter().all(|frame| *frame == [0.0; 2]));
            }
            assert_eq!(
                render(&warm, current.start(), expected_length as usize, 257),
                expected,
                "warm prefix {index}, retained repeat={repeated_frozen_root}"
            );
            assert_eq!(
                render(&cold, current.start(), expected_length as usize, 63),
                expected,
                "cold prefix {index}, retained repeat={repeated_frozen_root}"
            );
        }
        // Preparation must consume the candidate without rewriting authoring data.
        assert_eq!(editor.project(), &changed);
        assert!(editor.undo());
        assert_eq!(editor.project(), &before);
        assert!(editor.redo());
        assert_eq!(editor.project(), &changed);
    }
}

#[test]
fn global_tempo_warped_region_and_midi_note_edges_follow_new_clock() {
    let (mut editor, clip, media) = fixture(false);
    let fade = editor.project().clip(clip).unwrap().fade();
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
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 16_000,
        })
        .unwrap();
    let Created::Track(midi_track) = editor
        .execute(Command::AddMidiTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track: midi_track,
            start: BeatTime::from_quarters(2).unwrap(),
            length: BeatTime::from_quarters(2).unwrap(),
        })
        .unwrap()
    else {
        panic!("midi")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    assert_eq!(
        crate::compile::midi_events(editor.project())
            .unwrap()
            .iter()
            .map(|e| e.frame)
            .collect::<Vec<_>>(),
        [16_000, 24_000]
    );
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    assert_eq!(
        crate::compile::midi_events(editor.project())
            .unwrap()
            .iter()
            .map(|e| e.frame)
            .collect::<Vec<_>>(),
        [32_000, 48_000]
    );
    let current = editor.project().clip(clip).unwrap();
    assert_eq!((current.start(), current.duration()), (32_000, 16_000));
    let revision = prepare(&editor, &media, &AudioTransformPreparation::default());
    let actual = render(&revision, 32_000, 16_000, 63);
    let original = media.values().next().unwrap().frames();
    for (index, source) in original.iter().enumerate() {
        let gain = fade.gain_at(index as u64);
        assert_eq!(
            actual[index * 2],
            [source[0] * gain, source[1] * gain],
            "warped source frame {index}"
        );
    }
    assert_eq!(render(&revision, 31_999, 1, 1), [[0.0; 2]]);
}

fn section(editor: &mut Editor, start: u64, end: u64) -> hex_project::SectionId {
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
}

#[test]
fn section_reorder_preserves_frozen_negative_reference_event_fade_pcm_and_both_caches() {
    use hex_project::{AudioClipTiming, TrimEdge};
    let (mut editor, clip, media) = fixture(true);
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
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split {
                at: BeatTime::ONE.raw() / 2,
            },
        })
        .unwrap();
    editor
        .execute(Command::SetClipFadeEnvelope {
            clip,
            fade: ClipFade::new(8000, 0, 500, 1000)
                .unwrap()
                .with_shape(100, 200, 600, -500)
                .unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 2000,
        })
        .unwrap();
    let track = editor.project().tracks()[0].id();
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
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 16000,
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 14000,
        })
        .unwrap();
    assert!(
        matches!(editor.project().clip(clip).unwrap().audio_edit().unwrap().timing, AudioClipTiming::Frozen { reference_start_raw, .. } if reference_start_raw < 0)
    );
    let target = section(&mut editor, 0, 1);
    let source = section(&mut editor, 4, 5);
    let context = AudioTransformPreparation::default();
    let before = render(&prepare(&editor, &media, &context), 0, 48000, 63);
    assert!(before[14000..22000].iter().any(|f| *f != [0.0; 2]));
    let expected: Vec<_> = before[16000..20000]
        .iter()
        .chain(&before[..16000])
        .chain(&before[20000..])
        .copied()
        .collect();
    editor
        .execute(Command::ReorderSectionBefore {
            section: source,
            before: target,
        })
        .unwrap();
    for row in editor.project().tracks()[0].clips() {
        let edit = row.audio_edit().unwrap();
        assert!(matches!(edit.timing, AudioClipTiming::Frozen { .. }));
        assert!(edit.processing_projection.is_none());
    }
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 48000, 257),
        expected
    );
    assert_eq!(
        render(
            &prepare(&editor, &media, &AudioTransformPreparation::default()),
            0,
            48000,
            127
        ),
        expected
    );
}

#[test]
fn global_tempo_absorbs_section_projection_without_changing_fragment_pcm_or_split_history() {
    for reversed in [false, true] {
        use hex_project::AudioClipTiming;
        let (mut editor, clip, media) = fixture(true);
        editor
            .execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })
            .unwrap();
        if reversed {
            editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        }
        let source = section(&mut editor, 1, 2);
        let target = section(&mut editor, 3, 4);
        editor
            .execute(Command::ReorderSectionBefore {
                section: source,
                before: target,
            })
            .unwrap();
        let context = AudioTransformPreparation::default();
        let before = editor.project().clone();
        let prepared = prepare(&editor, &media, &context);
        let pieces: Vec<_> = before.tracks()[0]
            .clips()
            .iter()
            .map(|c| {
                (
                    c.id(),
                    c.start(),
                    render(&prepared, c.start(), c.duration() as usize, 63),
                    c.audio_edit().unwrap().processing_origin().unwrap(),
                )
            })
            .collect();
        assert!(
            before.tracks()[0].clips().iter().all(|c| c
                .audio_edit()
                .unwrap()
                .processing_projection
                .is_some())
        );
        editor
            .execute(Command::SetProjectTempo {
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        let changed = editor.project().clone();
        for (id, start, pcm, origin) in &pieces {
            let c = changed.clip(*id).unwrap();
            let edit = c.audio_edit().unwrap();
            assert_eq!(c.start(), start * 2);
            assert_eq!(c.duration(), pcm.len() as u64);
            assert!(matches!(edit.timing, AudioClipTiming::Frozen { .. }));
            assert!(edit.processing_projection.is_none());
            assert_eq!(edit.processing_origin().unwrap(), *origin);
            assert_eq!(
                (edit.placement_start_offset, edit.placement_end_offset),
                (0, 0)
            );
            assert_eq!(
                render(
                    &prepare(&editor, &media, &context),
                    c.start(),
                    pcm.len(),
                    257
                ),
                *pcm
            );
        }
        let c = editor.project().clip(pieces[0].0).unwrap();
        let (id, start, length) = (c.id(), c.start(), c.duration());
        let all_before_split = render(&prepare(&editor, &media, &context), 0, 16000, 63);
        editor
            .execute(Command::SplitClip {
                clip: id,
                frame: start + length / 2,
            })
            .unwrap();
        assert_eq!(
            render(
                &prepare(&editor, &media, &AudioTransformPreparation::default()),
                0,
                16000,
                127
            ),
            all_before_split
        );
        // IDs allocated by split retain the canonical monotonic high-water mark
        // through Undo; all authored state and processing coordinates restore.
        let compare = |actual: &Project, expected: &Project| {
            let mut restored = actual.snapshot();
            restored.next_id = expected.snapshot().next_id;
            assert_eq!(restored, expected.snapshot());
        };
        assert!(editor.undo());
        compare(editor.project(), &changed);
        assert!(editor.undo());
        compare(editor.project(), &before);
        assert!(editor.redo());
        compare(editor.project(), &changed);
    }
}
