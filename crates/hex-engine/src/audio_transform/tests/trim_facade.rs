use super::*;
use hex_project::{AudioClipPreviewOperation as Op, ClipContentToken, TrimEdge};

#[test]
fn prospective_warp_matches_materialized_canonical_trim_and_stretch_across_modes_and_limits() {
    let modes = [
        WarpMode::RePitch,
        WarpMode::Beats {
            preserve: hex_project::BeatsPreserve::Division(BeatTime::ONE),
            tail: BeatsTail::Forward,
            decay: 100,
        },
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 50,
            seed: 9,
        },
        WarpMode::Complex,
        WarpMode::ComplexPro {
            formants: 50,
            envelope: 128,
        },
    ];
    let mut workspace = WarpPreviewWorkspace::reserve_on_control().unwrap();
    let mut accepted = 0;
    let mut refused = 0;
    for looping in [false, true] {
        for mode in modes {
            for pitch in [-1200, 0, 1200] {
                let (mut editor, id, _) = fixture(looping);
                let settings = default_audio_warp(
                    editor.project().clip(id).unwrap(),
                    editor.project().tempo_map(),
                    mode,
                )
                .unwrap();
                editor
                    .execute(Command::SetAudioClipWarp {
                        clip: id,
                        settings: Some(settings),
                    })
                    .unwrap();
                editor
                    .execute(Command::SetAudioClipPitch {
                        clip: id,
                        cents: pitch,
                    })
                    .unwrap();
                editor
                    .execute(Command::SetTempoPoint {
                        beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
                        tempo: Tempo::new(90_000_000).unwrap(),
                    })
                    .unwrap();
                let project = editor.project();
                let clip = project.clip(id).unwrap();
                let frames = project.asset(clip.asset()).unwrap().playback_frames();
                for edge in [TrimEdge::Left, TrimEdge::Right] {
                    for frame in [0, 1, 499, 500, 1000, 8000, 64000, 64001] {
                        for operation in [Op::Trim(edge), Op::Stretch(edge), Op::FadeTrim(edge)] {
                            let expected = match operation {
                                Op::Trim(edge) => clip.preview_edge_trim_with_tempo(
                                    edge,
                                    frame,
                                    frames,
                                    project.tempo_map(),
                                ),
                                Op::Stretch(edge) => hex_project::preview_audio_clip_stretch(
                                    clip,
                                    edge,
                                    frame,
                                    project.tempo_map(),
                                    frames,
                                ),
                                Op::FadeTrim(edge) => {
                                    let mut model = Editor::new(project.clone(), 4).unwrap();
                                    model
                                        .execute(Command::TrimClipFadeEdge {
                                            clip: id,
                                            edge,
                                            frame,
                                        })
                                        .map(|_| model.project().clip(id).unwrap().clone())
                                }
                            }
                            .map_err(Invalid::from)
                            .and_then(|materialized| {
                                validate_clip_warp_preview(&materialized, project.tempo_map())
                            });
                            let actual = clip
                                .with_audio_preview(
                                    operation,
                                    frame,
                                    frames,
                                    project.tempo_map(),
                                    |read| validate_audio_clip_preview_warp(&read, &mut workspace),
                                )
                                .map_err(Invalid::from)
                                .and_then(|r| r);
                            assert_eq!(
                                actual, expected,
                                "{looping:?} {mode:?} {pitch} {operation:?} {frame}"
                            );
                            if actual.is_ok() {
                                accepted += 1;
                            } else {
                                refused += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(accepted > 0 && refused > 0);
}

#[test]
fn exact_content_and_reusable_prospective_validator_allocate_and_free_nothing_in_scoped_reads() {
    let (mut editor, id, _) = fixture(true);
    let settings = default_audio_warp(
        editor.project().clip(id).unwrap(),
        editor.project().tempo_map(),
        WarpMode::Complex,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip: id,
            settings: Some(settings),
        })
        .unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "empty metadata fixture".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(midi_id) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi_id,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let project = editor.project();
    let clip = project.clip(id).unwrap();
    let mut token =
        ClipContentToken::reserve_on_control(clip.content_token_capacity().unwrap()).unwrap();
    let mut workspace = WarpPreviewWorkspace::reserve_on_control().unwrap();
    let midi = project.midi_clip(midi_id).unwrap();
    let mut midi_token =
        ClipContentToken::reserve_on_control(midi.content_token_capacity().unwrap()).unwrap();
    let measured = allocation_counter::measure(|| {
        for frame in [8000, 1000, 64000, 64001, 8000] {
            clip.capture_content(&mut token).unwrap();
            assert!(clip.matches_content(&token));
            midi.capture_content(&mut midi_token).unwrap();
            assert!(midi.matches_content(&midi_token));
            midi.preview_edge_trim(TrimEdge::Left, BeatTime::ZERO)
                .unwrap();
            let _ = clip.with_audio_preview(
                Op::Stretch(TrimEdge::Right),
                frame,
                8000,
                project.tempo_map(),
                |read| validate_audio_clip_preview_warp(&read, &mut workspace),
            );
        }
    });
    assert_eq!(
        (
            measured.count_total,
            measured.count_current,
            measured.bytes_total,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
