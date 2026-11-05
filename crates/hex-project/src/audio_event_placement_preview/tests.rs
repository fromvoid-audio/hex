use super::*;
use crate::{AudioEventEdit, ClipGain, Command, Created, Editor};
fn fixture() -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 100,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 40 },
        })
        .unwrap();
    (editor, clip)
}
#[test]
fn signed_preview_matches_canonical_transaction_without_changing_history_or_source() {
    for start in [-25, 25, 130] {
        let (mut editor, clip) = fixture();
        let before = editor.project().clone();
        let preview = before
            .preview_audio_event_placement(clip, 1, start)
            .unwrap();
        assert_eq!(editor.project(), &before);
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Place { event: 1, start },
            })
            .unwrap();
        assert_eq!(editor.project().clip(clip), Some(preview.clip()));
        assert_eq!(
            (preview.clip().start(), preview.clip().duration()),
            (0, 100)
        );
        let events = preview.clip().audio_events();
        let moved = events.events.iter().find(|e| e.id == 1).unwrap();
        assert_eq!(preview.original_waveform_frame(1, moved.start), Some(0));
        assert_eq!(preview.original_waveform_frame(1, moved.end), Some(40));
        for e in &events.events {
            assert_eq!(
                preview.original_waveform_frame(e.id, e.start),
                Some(e.source_start)
            );
            assert_eq!(
                preview.original_waveform_frame(e.id, e.end),
                Some(e.source_start + e.length())
            );
        }
        assert!(editor.undo());
        assert_eq!(editor.project(), &before);
    }
}
#[test]
fn gain_fade_identity_and_unsupported_roots_do_not_borrow_unproved_peaks() {
    let (mut editor, clip) = fixture();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::SetGain {
                event: 1,
                gain: ClipGain::new(-6000).unwrap(),
            },
        })
        .unwrap();
    let preview = editor
        .project()
        .preview_audio_event_placement(clip, 1, 130)
        .unwrap();
    assert_eq!(preview.original_waveform_frame(1, 130), Some(0));
    assert_eq!(preview.original_waveform_frame(1, 129), None);
    assert_eq!(preview.original_waveform_frame(99, 130), None);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 10,
            fade_out: 0,
        })
        .unwrap();
    let unproved = editor
        .project()
        .preview_audio_event_placement(clip, 1, 130)
        .unwrap();
    assert!(
        unproved.waveform.is_empty(),
        "a moved event cannot borrow its old position-dependent container fade"
    );
    for start in [i64::MIN, i64::MAX] {
        assert!(
            editor
                .project()
                .preview_audio_event_placement(clip, 1, start)
                .is_err()
        );
    }
    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: true,
        })
        .unwrap();
    assert!(
        editor
            .project()
            .preview_audio_event_placement(clip, 1, 130)
            .is_err()
    );
}

#[test]
fn musical_root_retains_ordinary_editing_without_admitting_free_editor_placement() {
    let (mut editor, clip) = fixture();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    assert_eq!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_event_placement_limit(),
        None
    );
    assert!(
        editor
            .project()
            .preview_audio_event_placement(clip, 1, 130)
            .is_err()
    );
}

#[test]
#[ignore = "manual release-profile metadata cost; excludes GPUI, decoding, callback and physical deadlines"]
fn profile_free_placement_metadata_at_bounded_event_densities() {
    use std::{hint::black_box, time::Instant};
    for count in [2, 128, crate::MAX_AUDIO_EVENTS] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
        let length = 131_072_u64;
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "original.wav".into(),
                sample_rate: 48_000,
                frames: length,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        for index in 1..count {
            editor
                .execute(Command::EditAudioEvents {
                    clip,
                    operation: AudioEventEdit::Split {
                        at: index as u64 * length / count as u64,
                    },
                })
                .unwrap();
        }
        let events = editor
            .project()
            .clip(clip)
            .unwrap()
            .editable_audio_events()
            .unwrap();
        let id = events.events[0].id;
        let target = length / 3;
        for whole_clip in [false, true] {
            let mut samples = Vec::with_capacity(128);
            for index in 0..(16 + 128) {
                let start = Instant::now();
                if whole_clip {
                    black_box(
                        editor
                            .project()
                            .preview_audio_event_placement(
                                black_box(clip),
                                black_box(id),
                                black_box(target as i64),
                            )
                            .unwrap(),
                    );
                } else {
                    black_box(
                        events
                            .edited(
                                AudioEventEdit::Move {
                                    event: black_box(id),
                                    start: black_box(target),
                                },
                                length,
                            )
                            .unwrap(),
                    );
                }
                if index >= 16 {
                    samples.push(start.elapsed().as_nanos());
                }
            }
            samples.sort_unstable();
            println!(
                "free_placement_profile events={count} frames={length} whole_clip={whole_clip} warmup=16 samples=128 ns mean={} p50={} p99={} max={}",
                samples.iter().sum::<u128>() / samples.len() as u128,
                samples[samples.len() / 2],
                samples[samples.len() * 99 / 100],
                samples.last().unwrap()
            );
        }
    }
}

#[test]
fn grouped_source_refuses_pitch_before_any_editor_source_read_changes() {
    let (mut editor, clip) = fixture();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Place {
                event: 1,
                start: 130,
            },
        })
        .unwrap();
    let before = editor.project().clone();
    assert!(
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 100 })
            .is_err()
    );
    assert_eq!(editor.project(), &before);
    assert!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_event_placement_limit()
            .is_some()
    );
    let preview = editor
        .project()
        .preview_audio_event_placement(clip, 1, 150)
        .unwrap();
    assert_eq!(preview.original_waveform_frame(1, 150), Some(130));
}

#[test]
fn legacy_source_larger_than_the_derived_domain_keeps_its_existing_editor_extent() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let frames = crate::MAX_AUDIO_DOMAIN_FRAMES + 1;
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "large original".into(),
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let original = editor.project().clip(clip).unwrap();
    assert_eq!(original.duration(), frames);
    assert!(original.audio_edit().is_none());
    assert_eq!(
        original.audio_event_placement_limit(),
        None,
        "free placement cannot reduce the camera envelope of a larger legacy source"
    );
    assert!(
        editor
            .project()
            .preview_audio_event_placement(clip, 1, 1)
            .is_err()
    );
    assert_eq!(editor.project().clip(clip).unwrap(), original);
}

#[test]
fn signed_numeric_bounds_and_origin_translation_preserve_the_unchanged_clip_window() {
    let (mut editor, clip) = fixture();
    let before = editor.project().clip(clip).unwrap().clone();
    let (first, last) = before.audio_event_placement_bounds(1).unwrap();
    assert_eq!(first, -(crate::MAX_AUDIO_DOMAIN_FRAMES as i64 - 100));
    assert_eq!(last, crate::MAX_AUDIO_DOMAIN_FRAMES as i64 - 40);
    assert!(before.audio_event_placement_bounds(99).is_err());
    for value in [first, last] {
        let preview = editor
            .project()
            .preview_audio_event_placement(clip, 1, value)
            .unwrap();
        assert_eq!(
            preview.clip().audio_event_root().domain.clock.length(),
            crate::MAX_AUDIO_DOMAIN_FRAMES
        );
    }
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Place {
                event: 1,
                start: -25,
            },
        })
        .unwrap();
    let after = editor.project().clip(clip).unwrap().clone();
    assert_eq!(after.audio_event_coordinate_shift_from(&before), Some(25));
    assert_eq!(before.audio_event_coordinate_shift_from(&after), Some(-25));
    assert!(editor.undo());
    assert_eq!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_event_coordinate_shift_from(&after),
        Some(-25)
    );
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: crate::TrimEdge::Right,
            frame: 90,
        })
        .unwrap();
    assert_eq!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_event_coordinate_shift_from(&before),
        None
    );
}
