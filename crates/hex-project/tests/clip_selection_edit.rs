//! Atomic selection-wide values retain independent sources and one history edge.
use hex_project::{
    BeatTime, BeatsPreserve, BeatsTail, ClipCommandScope, ClipEdit, ClipFlavor, ClipId, Command,
    Created, Editor, Invalid, MAX_CLIPS, Project, Tempo, WarpAdjustment, WarpMarker, WarpMode,
    WarpSettings, default_audio_warp,
};
use std::cell::Cell;

fn fixture() -> (Editor, [ClipId; 4]) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "hybrid".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 240_000,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let mut audio = Vec::new();
    for (start, source_offset) in [(0, 0), (36_000, 24_000), (144_000, 48_000)] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start,
                duration: 24_000,
                source_offset,
                source_length: 24_000,
                looping: false,
            })
            .unwrap()
        else {
            panic!("audio")
        };
        audio.push(clip);
    }
    let Created::Clip(midi) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(4).unwrap(),
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("midi")
    };
    (
        Editor::new(editor.project().clone(), 16).unwrap(),
        [audio[0], audio[1], midi, audio[2]],
    )
}
fn warp(editor: &mut Editor, clip: ClipId, mode: WarpMode, middle: bool) -> WarpSettings {
    let mut settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        mode,
    )
    .unwrap();
    if middle {
        settings.markers.insert(
            1,
            WarpMarker {
                source_frame: settings.markers[0].source_frame + 12_000,
                beat: BeatTime::from_raw(BeatTime::ONE.raw() / 3),
            },
        );
    }
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings.clone()),
        })
        .unwrap();
    settings
}
fn settings(project: &Project, clip: ClipId) -> &WarpSettings {
    project
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain
        .warp
        .as_ref()
        .unwrap()
}
fn edit(clips: &[ClipId], edit: ClipEdit) -> Command {
    Command::EditClips {
        clips: clips.to_vec(),
        edit,
    }
}

#[test]
fn heterogeneous_warp_members_converge_without_copying_markers_or_other_settings() {
    let (mut editor, [a, b, midi, untouched]) = fixture();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().clip(a).unwrap().asset();
    let Created::Clip(raw) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 192_000,
            duration: 12_000,
            source_offset: 72_000,
            source_length: 12_000,
            looping: false,
        })
        .unwrap()
    else {
        panic!("raw default member")
    };
    let first = warp(
        &mut editor,
        a,
        WarpMode::Texture {
            grain_ms: 41,
            fluctuation: 29,
            seed: 73,
        },
        true,
    );
    let second = warp(&mut editor, b, WarpMode::Tones { grain_ms: 91 }, false);
    editor
        .execute(Command::SetAudioClipWarp {
            clip: b,
            settings: None,
        })
        .unwrap();
    let before = editor.project().clone();
    let mut editor = Editor::new(before.clone(), 8).unwrap();
    editor
        .execute(edit(
            &[a, midi, b, raw],
            ClipEdit::WarpEnabled { enabled: true },
        ))
        .unwrap();
    assert_eq!(settings(editor.project(), a), &first);
    assert_eq!(settings(editor.project(), b), &second);
    assert_eq!(
        settings(editor.project(), raw),
        &WarpSettings {
            mode: WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail: BeatsTail::Forward,
                decay: 100
            },
            markers: vec![
                WarpMarker {
                    source_frame: 72_000,
                    beat: BeatTime::ZERO
                },
                WarpMarker {
                    source_frame: 84_000,
                    beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2)
                }
            ],
        },
        "default settings derive from this member's own source and duration"
    );
    assert_eq!(editor.project().clip(raw).unwrap().start(), 192_000);
    assert_eq!(editor.project().clip(raw).unwrap().duration(), 12_000);

    for id in [a, b] {
        assert!(
            editor
                .project()
                .clip(id)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain
                .warp_enabled
        );
    }
    assert_eq!(editor.project().midi_clip(midi), before.midi_clip(midi));
    assert_eq!(editor.project().clip(untouched), before.clip(untouched));
    assert_eq!(
        editor.project().snapshot().next_id,
        before.snapshot().next_id
    );
    let accepted = editor.project().clone();
    assert!(editor.undo());
    assert!(!editor.can_undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(editor.project(), &accepted);
    assert_eq!(
        Project::from_snapshot(accepted.snapshot()).unwrap(),
        accepted
    );
    editor
        .execute(edit(
            &[a, b],
            ClipEdit::WarpMode {
                mode: WarpMode::Tones { grain_ms: 40 },
            },
        ))
        .unwrap();
    assert_eq!(
        settings(editor.project(), a).mode,
        WarpMode::Tones { grain_ms: 40 }
    );
    assert_eq!(
        settings(editor.project(), b).mode,
        WarpMode::Tones { grain_ms: 91 }
    );
    assert_eq!(settings(editor.project(), a).markers, first.markers);
    assert_eq!(settings(editor.project(), b).markers, second.markers);
    editor
        .execute(edit(&[a, b], ClipEdit::WarpEnabled { enabled: false }))
        .unwrap();
    for id in [a, b] {
        assert!(
            !editor
                .project()
                .clip(id)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain
                .warp_enabled
        );
    }
}

#[test]
fn shared_properties_and_delete_are_single_history_edges_with_explicit_midi_applicability() {
    for property in [
        ClipEdit::Gain { millidb: -6_000 },
        ClipEdit::Pitch { cents: 700 },
        ClipEdit::Musical { enabled: true },
        ClipEdit::Looping { looping: true },
        ClipEdit::Rename {
            name: "new label".into(),
        },
        ClipEdit::Flavor {
            flavor: ClipFlavor::Accent,
        },
        ClipEdit::Delete,
    ] {
        let (mut editor, [a, b, midi, untouched]) = fixture();
        let before = editor.project().clone();
        editor
            .execute(edit(&[a, midi, b], property.clone()))
            .unwrap();
        let after = editor.project().clone();
        assert_ne!(after, before);
        assert_eq!(after.clip(untouched), before.clip(untouched));
        assert_eq!(after.snapshot().next_id, before.snapshot().next_id);
        match property {
            ClipEdit::Gain { millidb } => {
                for id in [a, b] {
                    assert_eq!(after.clip(id).unwrap().gain().millidb(), millidb);
                }
                assert_eq!(after.midi_clip(midi), before.midi_clip(midi));
            }
            ClipEdit::Pitch { cents } => {
                for id in [a, b] {
                    assert_eq!(
                        after
                            .clip(id)
                            .unwrap()
                            .audio_edit()
                            .unwrap()
                            .domain
                            .pitch_cents,
                        cents
                    );
                }
                assert_eq!(after.midi_clip(midi), before.midi_clip(midi));
            }
            ClipEdit::Musical { enabled } => {
                assert!(enabled);
                for id in [a, b] {
                    assert_eq!(
                        after.clip(id).unwrap().audio_edit().unwrap().timing,
                        hex_project::AudioClipTiming::Musical(
                            before
                                .tempo_map()
                                .frame_to_beat(before.clip(id).unwrap().start())
                                .unwrap()
                        ),
                    );
                }
                assert_eq!(after.midi_clip(midi), before.midi_clip(midi));
            }
            ClipEdit::Looping { looping } => {
                for id in [a, b] {
                    assert_eq!(after.clip(id).unwrap().looping(), looping);
                }
                assert_eq!(after.midi_clip(midi), before.midi_clip(midi));
            }
            ClipEdit::Rename { name } => {
                for id in [a, b, midi] {
                    assert_eq!(after.clip_name(id), Some(name.as_str()));
                }
            }
            ClipEdit::Flavor { flavor } => {
                for id in [a, b, midi] {
                    assert_eq!(after.clip_flavor(id), Some(flavor));
                }
            }
            ClipEdit::Delete => {
                assert!(
                    after.clip(a).is_none()
                        && after.clip(b).is_none()
                        && after.midi_clip(midi).is_none()
                );
            }
            _ => panic!("fixture property"),
        }
        assert!(editor.undo());
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.project(), &after);
    }
}

#[test]
fn invalid_members_values_and_prepare_failure_preserve_project_ids_and_redo() {
    let (mut editor, [a, b, midi, removed]) = fixture();
    editor.execute(Command::RemoveClip(removed)).unwrap();
    let base = editor.project().clone();
    let mut editor = Editor::new(base.clone(), 8).unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    for command in [
        edit(&[], ClipEdit::Delete),
        edit(&[a, a], ClipEdit::Delete),
        edit(&[a, removed], ClipEdit::Delete),
        Command::EditClips {
            clips: vec![a; MAX_CLIPS + 1],
            edit: ClipEdit::Delete,
        },
        edit(&[midi], ClipEdit::Pitch { cents: 100 }),
        edit(&[a, b], ClipEdit::Pitch { cents: 4_801 }),
        edit(&[a, b], ClipEdit::Gain { millidb: 24_001 }),
        edit(
            &[a, b],
            ClipEdit::Rename {
                name: String::new(),
            },
        ),
        edit(
            &[a, b],
            ClipEdit::Rename {
                name: "x".repeat(hex_project::MAX_NAME_BYTES + 1),
            },
        ),
        edit(
            &[a, b],
            ClipEdit::WarpMode {
                mode: WarpMode::Tones { grain_ms: 0 },
            },
        ),
    ] {
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project(), &before);
        assert!(editor.can_redo());
        assert!(!editor.can_undo());
    }
    let prepares = Cell::new(0);
    let result = editor.execute_prepared(
        edit(&[a, b, midi], ClipEdit::Gain { millidb: -12_000 }),
        |candidate| -> Result<(), Invalid> {
            prepares.set(prepares.get() + 1);
            for id in [a, b] {
                assert_eq!(candidate.clip(id).unwrap().gain().millidb(), -12_000);
            }
            assert_eq!(candidate.midi_clip(midi), before.midi_clip(midi));
            Err(Invalid("injected preparation refusal"))
        },
    );
    assert!(result.is_err());
    assert_eq!(prepares.get(), 1);
    assert_eq!(editor.project(), &before);
    assert!(editor.can_redo());
    assert!(!editor.can_undo());
    editor
        .execute(edit(&[a, b], ClipEdit::Gain { millidb: 0 }))
        .unwrap();
    assert!(editor.can_redo());
    assert!(!editor.can_undo(), "canonical no-op creates no history");
}

#[test]
fn growing_members_validate_only_complete_rows_and_never_cut_selected_or_unselected_peers() {
    let (mut editor, [a, b, _, untouched]) = fixture();
    warp(&mut editor, a, WarpMode::RePitch, false);
    warp(&mut editor, b, WarpMode::RePitch, false);
    // Each growth is playable in isolation; together the first's 48k end
    // overlaps the second's unchanged36k onset. The scalar overwrite path
    // would otherwise trim the second before its own update.
    let before = editor.project().clone();
    let mut editor = Editor::new(before.clone(), 8).unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(editor.undo());
    let id = before.snapshot().next_id;
    let result = editor.execute(edit(
        &[a, b],
        ClipEdit::SampleBpm {
            tempo: Tempo::new(240_000_000).unwrap(),
        },
    ));
    assert!(result.is_err());
    assert_eq!(editor.project(), &before);
    assert_eq!(editor.project().clip(untouched), before.clip(untouched));
    assert_eq!(editor.project().snapshot().next_id, id);
    assert!(editor.can_redo());
    assert!(!editor.can_undo());
}

#[test]
fn warp_adjustments_keep_each_members_values_markers_and_refuse_incompatible_members() {
    let cases = [
        (
            WarpMode::Tones { grain_ms: 40 },
            WarpMode::Tones { grain_ms: 80 },
            WarpAdjustment::Grain(10),
            WarpMode::Tones { grain_ms: 50 },
            WarpMode::Tones { grain_ms: 90 },
        ),
        (
            WarpMode::Texture {
                grain_ms: 20,
                fluctuation: 25,
                seed: u64::MAX,
            },
            WarpMode::Texture {
                grain_ms: 40,
                fluctuation: 70,
                seed: 7,
            },
            WarpAdjustment::Seed,
            WarpMode::Texture {
                grain_ms: 20,
                fluctuation: 25,
                seed: 0,
            },
            WarpMode::Texture {
                grain_ms: 40,
                fluctuation: 70,
                seed: 8,
            },
        ),
        (
            WarpMode::Texture {
                grain_ms: 20,
                fluctuation: 25,
                seed: 1,
            },
            WarpMode::Texture {
                grain_ms: 40,
                fluctuation: 70,
                seed: 2,
            },
            WarpAdjustment::Fluctuation(127),
            WarpMode::Texture {
                grain_ms: 20,
                fluctuation: 100,
                seed: 1,
            },
            WarpMode::Texture {
                grain_ms: 40,
                fluctuation: 100,
                seed: 2,
            },
        ),
        (
            WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail: BeatsTail::Forward,
                decay: 60,
            },
            WarpMode::Beats {
                preserve: BeatsPreserve::Division(BeatTime::ONE),
                tail: BeatsTail::Off,
                decay: 20,
            },
            WarpAdjustment::Preserve,
            WarpMode::Beats {
                preserve: BeatsPreserve::Division(BeatTime::from_raw(BeatTime::ONE.raw() / 4)),
                tail: BeatsTail::Forward,
                decay: 60,
            },
            WarpMode::Beats {
                preserve: BeatsPreserve::Division(BeatTime::from_quarters(2).unwrap()),
                tail: BeatsTail::Off,
                decay: 20,
            },
        ),
        (
            WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail: BeatsTail::Off,
                decay: 20,
            },
            WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail: BeatsTail::Forward,
                decay: 70,
            },
            WarpAdjustment::Tail(BeatsTail::Alternating),
            WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail: BeatsTail::Alternating,
                decay: 20,
            },
            WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail: BeatsTail::Alternating,
                decay: 70,
            },
        ),
        (
            WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail: BeatsTail::Off,
                decay: 20,
            },
            WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail: BeatsTail::Forward,
                decay: 70,
            },
            WarpAdjustment::Decay(-128),
            WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail: BeatsTail::Off,
                decay: 0,
            },
            WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail: BeatsTail::Forward,
                decay: 0,
            },
        ),
        (
            WarpMode::ComplexPro {
                formants: 25,
                envelope: 256,
            },
            WarpMode::ComplexPro {
                formants: 70,
                envelope: 128,
            },
            WarpAdjustment::Formants(127),
            WarpMode::ComplexPro {
                formants: 100,
                envelope: 256,
            },
            WarpMode::ComplexPro {
                formants: 100,
                envelope: 128,
            },
        ),
        (
            WarpMode::ComplexPro {
                formants: 25,
                envelope: 256,
            },
            WarpMode::ComplexPro {
                formants: 70,
                envelope: 128,
            },
            WarpAdjustment::Resolution(i16::MIN),
            WarpMode::ComplexPro {
                formants: 25,
                envelope: 32,
            },
            WarpMode::ComplexPro {
                formants: 70,
                envelope: 32,
            },
        ),
    ];
    for (left, right, adjustment, expected_left, expected_right) in cases {
        let (mut editor, [a, b, midi, _]) = fixture();
        let original_left = warp(&mut editor, a, left, true);
        let original_right = warp(&mut editor, b, right, false);
        editor
            .execute(edit(&[a, b, midi], ClipEdit::WarpAdjust { adjustment }))
            .unwrap();
        assert_eq!(settings(editor.project(), a).mode, expected_left);
        assert_eq!(settings(editor.project(), b).mode, expected_right);
        assert_eq!(settings(editor.project(), a).markers, original_left.markers);
        assert_eq!(
            settings(editor.project(), b).markers,
            original_right.markers
        );
    }
    let (mut editor, [a, b, _, _]) = fixture();
    warp(&mut editor, a, WarpMode::Tones { grain_ms: 40 }, false);
    warp(
        &mut editor,
        b,
        WarpMode::Texture {
            grain_ms: 40,
            fluctuation: 25,
            seed: 1,
        },
        false,
    );
    let before = editor.project().clone();
    assert!(
        editor
            .execute(edit(
                &[a],
                ClipEdit::WarpMode {
                    mode: WarpMode::Tones { grain_ms: 0 }
                }
            ))
            .is_err(),
        "invalid requested values refuse even if this algorithm is already selected"
    );
    assert_eq!(editor.project(), &before);
    assert!(
        editor
            .execute(edit(
                &[a, b],
                ClipEdit::WarpAdjust {
                    adjustment: WarpAdjustment::Fluctuation(1)
                }
            ))
            .is_err()
    );
    assert_eq!(editor.project(), &before);
}

#[test]
fn command_scope_retains_property_source_note_geometry_and_explicit_boundaries() {
    let (mut editor, [a, _, midi, _]) = fixture();
    let Created::Note(note) = editor
        .execute(Command::AddMidiNote {
            clip: midi,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 93,
        })
        .unwrap()
    else {
        panic!("note")
    };
    assert_eq!(
        Command::SetAudioClipPitch {
            clip: a,
            cents: 100
        }
        .clip_scope(),
        ClipCommandScope::Property {
            clip: a,
            edit: ClipEdit::Pitch { cents: 100 }
        }
    );
    assert_eq!(
        Command::SetAudioClipWarp {
            clip: a,
            settings: None
        }
        .clip_scope(),
        ClipCommandScope::FocusedSnapshot { clip: a }
    );
    assert_eq!(
        Command::EditAudioEvents {
            clip: a,
            operation: hex_project::AudioEventEdit::Delete { start: 0, end: 1 }
        }
        .clip_scope(),
        ClipCommandScope::FocusedSource { clip: a }
    );
    assert_eq!(
        Command::RemoveMidiNotes {
            clip: midi,
            notes: vec![note]
        }
        .clip_scope(),
        ClipCommandScope::FocusedSource { clip: midi }
    );
    assert_eq!(
        Command::RemoveMidiNote(note).clip_scope(),
        ClipCommandScope::FocusedNote { note }
    );
    assert_eq!(
        Command::DuplicateClip(a).clip_scope(),
        ClipCommandScope::Geometry { clip: a }
    );
    assert_eq!(
        edit(&[a], ClipEdit::Delete).clip_scope(),
        ClipCommandScope::Explicit
    );
    assert_eq!(
        Command::MasterGain { gain: 0.5 }.clip_scope(),
        ClipCommandScope::Unrelated
    );
}

#[test]
fn relative_controls_preserve_member_offsets_and_scalar_limits_atomically() {
    let (mut editor, [a, b, midi, untouched]) = fixture();
    for (clip, gain, pitch) in [(a, -6_000, 0), (b, 3_000, 700)] {
        editor
            .execute(Command::SetAudioClipGain {
                clip,
                millidb: gain,
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: pitch })
            .unwrap();
    }
    let before = editor.project().clone();
    let targets = [a, midi, b];
    editor
        .execute(edit(&targets, ClipEdit::GainBy { millidb: 1_000 }))
        .unwrap();
    for (clip, gain) in [(a, -5_000), (b, 4_000)] {
        assert_eq!(editor.project().clip(clip).unwrap().gain().millidb(), gain);
    }
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    editor
        .execute(edit(&targets, ClipEdit::PitchBy { cents: 100 }))
        .unwrap();
    for (clip, pitch) in [(a, 100), (b, 800)] {
        assert_eq!(
            editor
                .project()
                .clip(clip)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain
                .pitch_cents,
            pitch
        );
    }
    assert_eq!(editor.project().clip(untouched), before.clip(untouched));
    editor
        .execute(Command::SetAudioClipPitch {
            clip: b,
            cents: 4_799,
        })
        .unwrap();
    editor
        .execute(edit(&targets, ClipEdit::PitchBy { cents: 100 }))
        .unwrap();
    assert_eq!(
        editor
            .project()
            .clip(b)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .pitch_cents,
        4_800
    );
    editor
        .execute(Command::SetAudioClipGain {
            clip: b,
            millidb: 24_000,
        })
        .unwrap();
    let bounded = editor.project().clone();
    for millidb in [100, i32::MAX] {
        assert!(
            editor
                .execute(edit(&targets, ClipEdit::GainBy { millidb }))
                .is_err()
        );
        assert_eq!(editor.project(), &bounded);
    }
}
