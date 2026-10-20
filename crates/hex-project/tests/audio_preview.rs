//! Owner-free preview facts are compared with real canonical history edits and
//! the existing fade/warp laws; no UI, PCM or physical audio claims.
use hex_project::{
    AudioClipPreviewOperation as Op, AudioDomainClock, AudioEvent, AudioEvents,
    AudioPreviewWaveformPosition as Position, AudioViewMode, AudioViewPlacement,
    AudioViewProjection, BeatTime, Clip, ClipFade, ClipId, Command, Created, Editor, Project,
    Tempo, TrimEdge, WarpMarker, WarpMode, WarpSettings, preview_audio_clip_stretch,
};
use std::sync::Arc;
const Q: u64 = 1 << 32;
fn base() -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "preview".into(),
            path: "not-opened.wav".into(),
            sample_rate: 48_000,
            frames: 480_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    (editor, clip)
}
fn materialized(
    clip: &Clip,
    op: Op,
    frame: u64,
    tempo: &hex_project::TempoMap,
) -> Result<Clip, hex_project::Invalid> {
    match op {
        Op::Trim(edge) => clip.preview_edge_trim_with_tempo(edge, frame, 480_000, tempo),
        Op::FadeTrim(edge) => clip.preview_fade_edge_trim(edge, frame, 480_000, tempo),
        Op::Stretch(edge) => preview_audio_clip_stretch(clip, edge, frame, tempo, 480_000),
    }
}
fn compare(clip: &Clip, op: Op, frame: u64, tempo: &hex_project::TempoMap) -> Clip {
    let expected = materialized(clip, op, frame, tempo).unwrap();
    clip.with_audio_preview(op, frame, 480_000, tempo, |read| {
        let facts = read.facts();
        assert_eq!(
            (
                facts.start,
                facts.duration,
                facts.source_offset,
                facts.source_length,
                facts.source_phase,
                facts.looping,
                facts.gain,
                facts.fade,
                facts.retained_fade
            ),
            (
                expected.start(),
                expected.duration(),
                expected.source_offset(),
                expected.source_length(),
                expected.source_phase(),
                expected.looping(),
                expected.gain(),
                expected.fade(),
                expected.retained_fade()
            )
        );
        assert_eq!(facts.unchanged, clip == &expected);
        if let Some(edit) = expected.audio_edit() {
            assert_eq!(
                read.projected_domain().unwrap(),
                edit.projected_domain(tempo).unwrap()
            );
            let projection = edit.projected_fade(tempo).unwrap();
            let (_, length, _, _) = read.projected_domain().unwrap();
            for index in 0..17 {
                let frame = (length - 1) * index / 16;
                assert_eq!(
                    read.fade_gain_at(frame).unwrap(),
                    projection.gain_at(frame).unwrap()
                );
            }
            for position in edit.domain.fade.breakpoints().into_iter().flatten() {
                assert_eq!(
                    read.fade_frame_at(position).unwrap(),
                    projection.frame_at_domain_position(position).unwrap()
                );
            }
        }
        let placement = AudioViewProjection::new(&expected, tempo).unwrap();
        assert_eq!(
            read.occurrence_count().unwrap(),
            placement.occurrence_count()
        );
        for index in 0..placement.occurrence_count().min(8) {
            assert_eq!(
                read.occurrence(index).unwrap(),
                placement.occurrence(index).unwrap()
            );
        }
    })
    .unwrap();
    expected
}
#[test]
fn raw_trim_and_lower_handles_share_source_phase_clamps_fades_and_history() {
    for looping in [false, true] {
        let (mut editor, id) = base();
        editor
            .execute(Command::TrimClipEdge {
                clip: id,
                edge: TrimEdge::Left,
                frame: 24_000,
            })
            .unwrap();
        editor
            .execute(Command::TrimClipEdge {
                clip: id,
                edge: TrimEdge::Right,
                frame: 96_000,
            })
            .unwrap();
        editor
            .execute(Command::SetClipLooping { clip: id, looping })
            .unwrap();
        if looping {
            editor
                .execute(Command::SetClipSourcePhase {
                    clip: id,
                    phase: 17,
                })
                .unwrap();
        }
        editor
            .execute(Command::SetClipFadeEnvelope {
                clip: id,
                fade: ClipFade::new(72_000, 0, 12_000, 10_000)
                    .unwrap()
                    .with_shape(200, 100, 700, -600)
                    .unwrap(),
            })
            .unwrap();
        for (edge, frame) in [
            (TrimEdge::Left, 0),
            (TrimEdge::Left, 30_000),
            (TrimEdge::Right, 84_000),
            (TrimEdge::Right, 480_000),
        ] {
            for op in [Op::Trim(edge), Op::FadeTrim(edge)] {
                let before = editor.project().snapshot();
                let preview = compare(
                    editor.project().clip(id).unwrap(),
                    op,
                    frame,
                    editor.project().tempo_map(),
                );
                let command = match op {
                    Op::Trim(edge) => Command::TrimClipEdge {
                        clip: id,
                        edge,
                        frame,
                    },
                    Op::FadeTrim(edge) => Command::TrimClipFadeEdge {
                        clip: id,
                        edge,
                        frame,
                    },
                    _ => unreachable!(),
                };
                editor.execute(command).unwrap();
                assert_eq!(editor.project().clip(id), Some(&preview));
                assert!(editor.undo());
                assert_eq!(editor.project().snapshot(), before);
            }
        }
    }
}
#[test]
fn placement_trim_retains_repeat_phase_and_exhausted_once_silence() {
    for mode in [AudioViewMode::Once, AudioViewMode::Repeat] {
        let (editor, id) = base();
        let mut snapshot = editor.project().snapshot();
        // Pitch captures an edit without needing a musical clock.
        let mut setup = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
        setup
            .execute(Command::SetAudioClipPitch {
                clip: id,
                cents: 123,
            })
            .unwrap();
        snapshot = setup.project().snapshot();
        let clip = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
        let edit = clip.edit.as_mut().unwrap();
        edit.view_start = 12_000;
        edit.view_end = 36_000;
        edit.placement = Some(AudioViewPlacement {
            mode,
            outer_length: AudioDomainClock::Frames(48_000),
            cycle_phase: 6_000,
        });
        clip.duration = 48_000;
        clip.fade = ClipFade::neutral(48_000).unwrap();
        let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
        let original = editor.project().clip(id).unwrap();
        let root = Arc::clone(&original.audio_edit().unwrap().domain);
        let preview = compare(
            original,
            Op::Trim(TrimEdge::Left),
            30_000,
            editor.project().tempo_map(),
        );
        assert_eq!(preview.start(), 30_000);
        assert_eq!(preview.duration(), 18_000);
        let projection = AudioViewProjection::new(&preview, editor.project().tempo_map()).unwrap();
        assert_eq!(
            preview.audio_edit().unwrap().placement.unwrap().cycle_phase,
            if mode == AudioViewMode::Once {
                36_000
            } else {
                12_000
            }
        );
        if mode == AudioViewMode::Once {
            assert_eq!(projection.occurrence_count(), 0);
            assert_eq!(projection.root_origin(), -18_000);
        }
        assert!(Arc::ptr_eq(&root, &preview.audio_edit().unwrap().domain));
        editor
            .execute(Command::TrimClipEdge {
                clip: id,
                edge: TrimEdge::Left,
                frame: 30_000,
            })
            .unwrap();
        assert_eq!(editor.project().clip(id), Some(&preview));
    }
}
#[test]
fn musical_fade_projection_keeps_authored_tempo_across_current_tempo_change() {
    let (mut editor, id) = base();
    editor
        .execute(Command::SetAudioClipMusical {
            clip: id,
            enabled: true,
        })
        .unwrap();
    editor
        .execute(Command::SetClipFadeEnvelope {
            clip: id,
            fade: ClipFade::new(480_000, 0, 96_000, 48_000)
                .unwrap()
                .with_shape(0, 0, 750, -1000)
                .unwrap(),
        })
        .unwrap();
    let authored = editor.project().tempo_map().clone();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(2 * Q),
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let clip = editor.project().clip(id).unwrap();
    assert_eq!(
        clip.audio_edit()
            .unwrap()
            .domain
            .fade_projection
            .as_ref()
            .unwrap()
            .tempo,
        authored
    );
    for (edge, frame) in [(TrimEdge::Left, 24_000), (TrimEdge::Right, 480_000)] {
        compare(clip, Op::Trim(edge), frame, editor.project().tempo_map());
        compare(
            clip,
            Op::FadeTrim(edge),
            frame,
            editor.project().tempo_map(),
        );
    }
}
fn warped_events() -> (Editor, ClipId) {
    let (mut editor, id) = base();
    editor
        .execute(Command::SetAudioClipWarp {
            clip: id,
            settings: Some(WarpSettings {
                mode: WarpMode::RePitch,
                markers: vec![
                    WarpMarker {
                        source_frame: 0,
                        beat: BeatTime::ZERO,
                    },
                    WarpMarker {
                        source_frame: 24_000,
                        beat: BeatTime::from_raw(Q / 2),
                    },
                    WarpMarker {
                        source_frame: 480_000,
                        beat: BeatTime::from_raw(20 * Q),
                    },
                ],
            }),
        })
        .unwrap();
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
    editor
        .execute(Command::EditAudioEvents {
            clip: id,
            operation: hex_project::AudioEventEdit::Split { at: 2 * Q },
        })
        .unwrap();
    (editor, id)
}
#[test]
fn stretch_both_edges_keeps_processed_warp_event_mapping_and_canonical_undo() {
    let (mut editor, id) = warped_events();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(3 * Q),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip: id,
            track,
            start: 48_000,
        })
        .unwrap();
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        let before = editor.project().snapshot();
        let original = editor.project().clip(id).unwrap();
        let frame = if edge == TrimEdge::Left {
            24_000
        } else {
            original.start() + original.duration() * 5 / 4
        };
        let expected = compare(
            original,
            Op::Stretch(edge),
            frame,
            editor.project().tempo_map(),
        );
        let tempo = editor.project().tempo_map();
        original
            .with_audio_preview(Op::Stretch(edge), frame, 480_000, tempo, |read| {
                let next = expected.audio_edit().unwrap();
                let old = original.audio_edit().unwrap();
                let (origin, _, _, _) = next.projected_domain(tempo).unwrap();
                for index in 0..17 {
                    let timeline = expected.start() + expected.duration() * index / 16;
                    let root = u64::try_from(i128::from(timeline) - origin).unwrap();
                    let beat = next.root_position_at(root, tempo).unwrap();
                    let old_position = (u128::from(beat) * u128::from(old.domain.clock.length())
                        / u128::from(next.domain.clock.length()))
                        as u64;
                    assert_eq!(
                        read.accepted_waveform_position(timeline).unwrap(),
                        Some(Position::ProcessedRoot(
                            old.root_frame_at(old_position, tempo).unwrap()
                        ))
                    );
                }
            })
            .unwrap();
        editor
            .execute(Command::StretchAudioClipEdge {
                clip: id,
                edge,
                frame,
            })
            .unwrap();
        assert_eq!(editor.project().clip(id), Some(&expected));
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
    }
}
#[test]
fn raw_stretch_and_noop_keep_exact_source_projection_and_no_extra_history() {
    let (mut editor, id) = base();
    for edge in [TrimEdge::Left, TrimEdge::Right] {
        let clip = editor.project().clip(id).unwrap();
        let tempo = editor.project().tempo_map();
        let frame = if edge == TrimEdge::Left {
            clip.start()
        } else {
            clip.start() + clip.duration()
        };
        let before = editor.project().snapshot();
        let undo = editor.can_undo();
        clip.with_audio_preview(Op::Stretch(edge), frame, 0, tempo, |read| {
            assert!(read.facts().unchanged)
        })
        .unwrap();
        editor
            .execute(Command::StretchAudioClipEdge {
                clip: id,
                edge,
                frame,
            })
            .unwrap();
        assert_eq!(editor.project().snapshot(), before);
        assert_eq!(editor.can_undo(), undo);
    }
    let clip = editor.project().clip(id).unwrap();
    let tempo = editor.project().tempo_map();
    let expected = compare(clip, Op::Stretch(TrimEdge::Right), 600_000, tempo);
    clip.with_audio_preview(
        Op::Stretch(TrimEdge::Right),
        600_000,
        480_000,
        tempo,
        |read| {
            let edit = expected.audio_edit().unwrap();
            let (origin, _, _, _) = edit.projected_domain(tempo).unwrap();
            for timeline in [0, 24_000, 300_000, 600_000] {
                let beat = edit
                    .root_position_at(u64::try_from(i128::from(timeline) - origin).unwrap(), tempo)
                    .unwrap();
                let source = edit
                    .domain
                    .warp_source_position_q32(
                        u128::from(beat) + u128::from(edit.domain.warp_phase_beat().unwrap()),
                    )
                    .unwrap();
                assert_eq!(
                    read.accepted_waveform_position(timeline).unwrap(),
                    Some(Position::Source((source >> 32) as u128))
                );
            }
        },
    )
    .unwrap();
}
#[test]
fn rounding_collapsed_events_and_invalid_assets_refuse_without_mutation() {
    let (editor, id) = warped_events();
    let mut snapshot = editor.project().snapshot();
    let edit = snapshot.tracks[0].audio_clips_mut().unwrap()[0]
        .edit
        .as_mut()
        .unwrap();
    Arc::make_mut(&mut edit.domain).events = Some(Arc::new(AudioEvents {
        next_id: 2,
        events: vec![AudioEvent {
            id: 1,
            start: 0,
            end: 1,
            source_start: 0,
        }],
    }));
    let editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    let before = editor.project().snapshot();
    let clip = editor.project().clip(id).unwrap();
    let tempo = editor.project().tempo_map();
    let read = clip
        .with_audio_preview(Op::Stretch(TrimEdge::Right), 1, 480_000, tempo, |_| ())
        .unwrap_err();
    assert_eq!(
        read,
        materialized(clip, Op::Stretch(TrimEdge::Right), 1, tempo).unwrap_err()
    );
    assert_eq!(
        read.0,
        "audio events must have unique identities and bounded nonoverlapping intervals"
    );
    assert!(
        clip.with_audio_preview(Op::Stretch(TrimEdge::Right), 600_000, 0, tempo, |_| ())
            .is_err()
    );
    assert!(
        clip.with_audio_preview(Op::Stretch(TrimEdge::Right), 0, 480_000, tempo, |_| ())
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn bounded_event_identity_validation_keeps_timeline_order_independent_of_id_order() {
    let mut events = AudioEvents {
        next_id: 4,
        events: vec![
            AudioEvent {
                id: 3,
                start: 0,
                end: 10,
                source_start: 0,
            },
            AudioEvent {
                id: 1,
                start: 10,
                end: 20,
                source_start: 10,
            },
            AudioEvent {
                id: 2,
                start: 20,
                end: 30,
                source_start: 20,
            },
        ],
    };
    events.validate(30).unwrap();
    events.events[2].id = 3;
    assert_eq!(
        events.validate(30),
        Err(hex_project::Invalid(
            "audio events must have unique identities and bounded nonoverlapping intervals"
        ))
    );
    events.events[0].end = 11;
    assert!(
        events.validate(30).is_err(),
        "overlap remains invalid alongside duplicate identity"
    );
}

#[test]
fn default_warp_copy_endpoints_follow_source_region_tempo_and_placement_phase() {
    use hex_project::{default_audio_warp, default_audio_warp_markers};
    let (mut editor, id) = base();
    let check = |project: &Project, first: u64, last: u64, span: u64| {
        let clip = project.clip(id).unwrap();
        let markers = default_audio_warp_markers(clip, project.tempo_map()).unwrap();
        assert_eq!(
            markers,
            [
                WarpMarker {
                    source_frame: first,
                    beat: BeatTime::ZERO
                },
                WarpMarker {
                    source_frame: last,
                    beat: BeatTime::from_raw(span)
                }
            ]
        );
        assert_eq!(
            default_audio_warp(clip, project.tempo_map(), WarpMode::RePitch)
                .unwrap()
                .markers,
            markers
        );
    };
    check(editor.project(), 0, 480_000, 20 * Q);
    editor
        .execute(Command::TrimClipEdge {
            clip: id,
            edge: TrimEdge::Left,
            frame: 24_000,
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip: id,
            edge: TrimEdge::Right,
            frame: 96_000,
        })
        .unwrap();
    check(editor.project(), 24_000, 96_000, 3 * Q);
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
    check(editor.project(), 24_000, 96_000, 3 * Q);
    editor
        .execute(Command::SetAudioClipPitch {
            clip: id,
            cents: 123,
        })
        .unwrap();
    check(editor.project(), 24_000, 96_000, 3 * Q);
    editor
        .execute(Command::SetAudioClipMusical {
            clip: id,
            enabled: true,
        })
        .unwrap();
    check(editor.project(), 24_000, 96_000, 3 * Q);
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(2 * Q),
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    check(editor.project(), 24_000, 96_000, 2 * Q);
    for (mode, phase, span) in [
        (AudioViewMode::Repeat, Q / 2, 9 * Q / 4),
        (AudioViewMode::Once, Q / 2, 9 * Q / 4),
        (AudioViewMode::Once, 4 * Q, 3 * Q),
    ] {
        let mut snapshot = editor.project().snapshot();
        snapshot.tracks[0].audio_clips_mut().unwrap()[0]
            .edit
            .as_mut()
            .unwrap()
            .placement = Some(AudioViewPlacement {
            mode,
            outer_length: AudioDomainClock::Beats(BeatTime::from_raw(3 * Q)),
            cycle_phase: phase,
        });
        let project = Project::from_snapshot(snapshot).unwrap();
        check(&project, 24_000, 96_000, span);
    }
}

#[test]
fn default_warp_copy_endpoints_preserve_frame_event_timing_refusal() {
    use hex_project::{default_audio_warp, default_audio_warp_markers};
    let (mut editor, id) = base();
    editor
        .execute(Command::EditAudioEvents {
            clip: id,
            operation: hex_project::AudioEventEdit::Split { at: 24_000 },
        })
        .unwrap();
    let before = editor.project().snapshot();
    let clip = editor.project().clip(id).unwrap();
    let tempo = editor.project().tempo_map();
    let error = default_audio_warp_markers(clip, tempo).unwrap_err();
    assert_eq!(
        error.0,
        "timing mode cannot change after audio event edits; undo those edits first"
    );
    assert_eq!(
        default_audio_warp(clip, tempo, WarpMode::RePitch).unwrap_err(),
        error
    );
    assert_eq!(editor.project().snapshot(), before);
}
