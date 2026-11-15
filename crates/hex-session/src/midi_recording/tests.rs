use super::*;
use hex_project::{Created, Editor, Tempo};

fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn fixture() -> (Project, TrackId, ClipId) {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "record".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(2),
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: beat(1),
            loop_start: BeatTime::ZERO,
            loop_end: beat(4),
            looping: false,
            length: beat(2),
        })
        .unwrap();
    (editor.project().clone(), track, clip)
}
fn note(start_frame: u64, end_frame: u64, pitch: u8) -> RecordedMidiNote {
    RecordedMidiNote {
        start_frame,
        end_frame,
        pitch,
        channel: 3,
        velocity: 93,
    }
}

#[test]
fn frame_take_maps_trimmed_source_closes_held_notes_and_is_one_persisted_history_edit() {
    let (project, track, clip) = fixture();
    let binding = MidiTakeBinding::capture(&project, track, clip).unwrap();
    assert_eq!(
        (binding.first_frame(), binding.end_frame()),
        (8_000, 16_000)
    );
    let before = project.snapshot();
    let captured = [note(9_000, 10_000, 60), note(15_000, 18_000, 64)];
    let command = finish_take(&project, &binding, &captured, 18_000)
        .unwrap()
        .unwrap();
    assert_eq!(project.snapshot(), before);
    let mut editor = Editor::new(project, 8).unwrap();
    assert!(!editor.can_undo());
    editor.execute(command).unwrap();
    let notes = editor.project().midi_clip(clip).unwrap().notes();
    assert_eq!(notes.len(), 2);
    for (n, numerator) in notes.iter().zip([5, 11]) {
        assert_eq!(n.start(), BeatTime::from_ratio(numerator, 4).unwrap());
        assert_eq!(n.duration(), BeatTime::from_ratio(1, 4).unwrap());
        assert_eq!((n.channel(), n.velocity()), (3, 93));
    }
    let after = editor.project().snapshot();
    let encoded = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&encoded).unwrap().snapshot(), after);
    assert!(editor.undo());
    let mut restored = before;
    // Undo preserves the canonical global ID high-water mark.
    restored.next_id = after.next_id;
    assert_eq!(editor.project().snapshot(), restored);
    assert!(!editor.can_undo());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn source_substitution_tempo_and_geometry_changes_refuse_without_consuming_the_take() {
    let (project, track, clip) = fixture();
    let binding = MidiTakeBinding::capture(&project, track, clip).unwrap();
    let captured = [note(9_000, 10_000, 60)];
    let replacement = Project::from_snapshot(project.snapshot()).unwrap();
    assert!(finish_take(&replacement, &binding, &captured, 12_000).is_err());
    for command in [
        Command::SetProjectTempo {
            tempo: Tempo::new(130_000_000).unwrap(),
        },
        Command::MoveMidiClip {
            clip,
            track,
            start: beat(3),
        },
        Command::ReverseClips(vec![clip]),
    ] {
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        editor.execute(command).unwrap();
        let before = editor.project().snapshot();
        assert!(finish_take(editor.project(), &binding, &captured, 12_000).is_err());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.undo());
        assert!(!editor.can_undo(), "refusal must not add a history edge");
        assert!(
            finish_take(editor.project(), &binding, &captured, 12_000)
                .unwrap()
                .is_some()
        );
    }
    assert!(
        finish_take(&project, &binding, &captured, 12_000)
            .unwrap()
            .is_some()
    );
    assert_eq!(captured, [note(9_000, 10_000, 60)]);
}

#[test]
fn malformed_overfull_and_unsorted_takes_refuse_while_empty_take_has_no_edit() {
    let (project, track, clip) = fixture();
    let binding = MidiTakeBinding::capture(&project, track, clip).unwrap();
    assert!(
        finish_take(&project, &binding, &[], 12_000)
            .unwrap()
            .is_none()
    );
    let valid = note(9_000, 10_000, 60);
    for invalid in [
        RecordedMidiNote {
            start_frame: 7_999,
            ..valid
        },
        RecordedMidiNote {
            start_frame: 16_000,
            end_frame: 17_000,
            ..valid
        },
        RecordedMidiNote {
            end_frame: 9_000,
            ..valid
        },
        RecordedMidiNote {
            end_frame: 12_001,
            ..valid
        },
        RecordedMidiNote {
            pitch: 128,
            ..valid
        },
        RecordedMidiNote {
            channel: 16,
            ..valid
        },
        RecordedMidiNote {
            velocity: 0,
            ..valid
        },
        RecordedMidiNote {
            velocity: 128,
            ..valid
        },
    ] {
        assert!(finish_take(&project, &binding, &[invalid], 12_000).is_err());
    }
    assert!(
        finish_take(
            &project,
            &binding,
            &[valid; MAX_MIDI_TAKE_NOTES + 1],
            12_000
        )
        .is_err()
    );
    assert!(
        finish_take(
            &project,
            &binding,
            &[note(10_000, 11_000, 64), valid],
            12_000
        )
        .is_err()
    );
}

#[test]
fn unrelated_name_or_mixer_edits_do_not_substitute_an_unchanged_note_source() {
    let (project, track, clip) = fixture();
    let binding = MidiTakeBinding::capture(&project, track, clip).unwrap();
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::RenameTrack {
            track,
            name: "new name".into(),
        })
        .unwrap();
    editor.execute(Command::Gain { track, gain: 0.5 }).unwrap();
    assert!(
        finish_take(
            editor.project(),
            &binding,
            &[note(9_000, 10_000, 60)],
            12_000
        )
        .unwrap()
        .is_some()
    );
}

#[test]
fn subframe_clip_edge_is_projected_inside_the_authored_source() {
    let (project, track, clip) = fixture();
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::MoveMidiClip {
            clip,
            track,
            start: BeatTime::from_raw(beat(2).raw() + 1),
        })
        .unwrap();
    let binding = MidiTakeBinding::capture(editor.project(), track, clip).unwrap();
    let command = finish_take(
        editor.project(),
        &binding,
        &[note(binding.first_frame(), binding.first_frame() + 1, 60)],
        binding.first_frame() + 1,
    )
    .unwrap()
    .unwrap();
    editor.execute(command).unwrap();
    let n = &editor.project().midi_clip(clip).unwrap().notes()[0];
    assert_eq!(n.start(), beat(1));
    assert!(n.duration() > BeatTime::ZERO);
}

#[test]
fn unsupported_capture_targets_refuse_before_any_note_or_history_edit() {
    let (project, track, clip) = fixture();
    for command in [
        Command::RemoveInstrument { track },
        Command::SetMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: beat(1),
            loop_start: BeatTime::ZERO,
            loop_end: beat(4),
            looping: true,
            length: beat(2),
        },
        Command::SetMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: beat(1),
            loop_start: BeatTime::ZERO,
            loop_end: beat(4),
            looping: false,
            length: beat(4),
        },
    ] {
        let mut editor = Editor::new(project.clone(), 8).unwrap();
        editor.execute(command).unwrap();
        let before = editor.project().snapshot();
        assert!(MidiTakeBinding::capture(editor.project(), track, clip).is_err());
        assert_eq!(editor.project().snapshot(), before);
    }
}

#[test]
fn inactive_or_retrimmed_clip_refuses_and_new_take_maps_playback_offset() {
    let (project, track, clip) = fixture();
    let binding = MidiTakeBinding::capture(&project, track, clip).unwrap();
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::SetClipActive {
            clip,
            active: false,
        })
        .unwrap();
    assert!(MidiTakeBinding::capture(editor.project(), track, clip).is_err());
    assert!(
        finish_take(
            editor.project(),
            &binding,
            &[note(9_000, 10_000, 60)],
            12_000
        )
        .is_err()
    );
    assert!(editor.undo());
    editor
        .execute(Command::TrimMidiClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            beat: BeatTime::from_ratio(5, 2).unwrap(),
        })
        .unwrap();
    assert!(
        finish_take(
            editor.project(),
            &binding,
            &[note(11_000, 12_000, 60)],
            13_000
        )
        .is_err()
    );
    let binding = MidiTakeBinding::capture(editor.project(), track, clip).unwrap();
    let command = finish_take(
        editor.project(),
        &binding,
        &[note(11_000, 12_000, 60)],
        13_000,
    )
    .unwrap()
    .unwrap();
    editor.execute(command).unwrap();
    assert_eq!(
        editor.project().midi_clip(clip).unwrap().notes()[0].start(),
        BeatTime::from_ratio(7, 4).unwrap()
    );
}

fn capture_source() -> hex_engine::midi_capture::MidiCaptureSource {
    let input = hex_engine::computer_keyboard::ComputerKeyboardInput::with_output_activation(
        std::sync::Arc::new(Default::default()),
    )
    .unwrap()
    .with_recording_support()
    .unwrap();
    hex_engine::midi_capture::MidiCaptureSource::Computer {
        activation: input.begin_ordered_keys_on_control().unwrap(),
        key: 0,
        press_token: 1,
    }
}

#[test]
fn latency_corrected_take_clips_preroll_and_finishes_at_the_audible_end() {
    use hex_engine::midi_capture::*;
    let source = capture_source();
    let mut take = MidiTake::new_with_latency(
        100,
        110,
        MidiCaptureClock {
            content_frame: 100,
            performance_frame: 1_000,
        },
        3,
    )
    .unwrap();
    for (id, performance_frame, frame, event) in [
        (
            1,
            1_001,
            100,
            MidiCaptureEvent::On {
                pitch: 60,
                velocity: 90,
            },
        ),
        (1, 1_002, 100, MidiCaptureEvent::Off),
        (
            2,
            1_004,
            101,
            MidiCaptureEvent::On {
                pitch: 64,
                velocity: 80,
            },
        ),
        (2, 1_012, 109, MidiCaptureEvent::Off),
    ] {
        take.push(
            AcceptedMidiNote {
                source,
                occurrence_id: id,
                performance_frame,
                event,
            },
            frame,
        )
        .unwrap();
    }
    let end = take
        .finish(MidiCaptureClock {
            content_frame: 113,
            performance_frame: 1_013,
        })
        .unwrap();
    assert_eq!(end, 110);
    assert_eq!(
        take.notes(),
        &[RecordedMidiNote {
            start_frame: 101,
            end_frame: 109,
            pitch: 64,
            channel: 0,
            velocity: 80,
        }]
    );
}

#[test]
fn delivered_short_notes_chords_and_stop_pair_by_occurrence_and_keep_take_until_ack() {
    use hex_engine::midi_capture::*;
    let source = capture_source();
    let mut take = MidiTake::new(
        8_000,
        16_000,
        MidiCaptureClock {
            content_frame: 8_500,
            performance_frame: 100,
        },
    )
    .unwrap();
    let events = [
        (
            1,
            102,
            MidiCaptureEvent::On {
                pitch: 60,
                velocity: 93,
            },
        ),
        (
            2,
            103,
            MidiCaptureEvent::On {
                pitch: 64,
                velocity: 37,
            },
        ),
        (1, 104, MidiCaptureEvent::Off),
        (
            3,
            105,
            MidiCaptureEvent::On {
                pitch: 60,
                velocity: 70,
            },
        ),
        (3, 106, MidiCaptureEvent::Off),
    ];
    for (occurrence_id, performance_frame, event) in events {
        take.push(
            AcceptedMidiNote {
                source,
                occurrence_id,
                performance_frame,
                event,
            },
            8_400 + performance_frame,
        )
        .unwrap();
    }
    take.finish(MidiCaptureClock {
        content_frame: 8_512,
        performance_frame: 112,
    })
    .unwrap();
    assert!(take.complete());
    assert_eq!(
        take.notes(),
        &[
            RecordedMidiNote {
                start_frame: 8_502,
                end_frame: 8_504,
                pitch: 60,
                channel: 0,
                velocity: 93
            },
            RecordedMidiNote {
                start_frame: 8_503,
                end_frame: 8_512,
                pitch: 64,
                channel: 0,
                velocity: 37
            },
            RecordedMidiNote {
                start_frame: 8_505,
                end_frame: 8_506,
                pitch: 60,
                channel: 0,
                velocity: 70
            },
        ]
    );
    let (project, track, clip) = fixture();
    let binding = MidiTakeBinding::capture(&project, track, clip).unwrap();
    let replaced = Project::from_snapshot(project.snapshot()).unwrap();
    assert!(finish_take(&replaced, &binding, take.notes(), 8_512).is_err());
    assert_eq!(take.notes().len(), 3);
    assert!(
        finish_take(&project, &binding, take.notes(), 8_512)
            .unwrap()
            .is_some()
    );
    assert_eq!(take.notes().len(), 3);
}

#[test]
fn interrupted_or_noncontiguous_delivery_cannot_become_a_complete_take() {
    use hex_engine::midi_capture::*;
    let mut take = MidiTake::new(
        8_000,
        16_000,
        MidiCaptureClock {
            content_frame: 8_500,
            performance_frame: 100,
        },
    )
    .unwrap();
    let event = AcceptedMidiNote {
        source: capture_source(),
        occurrence_id: 1,
        performance_frame: 102,
        event: MidiCaptureEvent::On {
            pitch: 60,
            velocity: 93,
        },
    };
    assert!(take.push(event, 8_503).is_err());
    assert!(
        take.finish(MidiCaptureClock {
            content_frame: 8_512,
            performance_frame: 112
        })
        .is_err()
    );
    assert!(!take.complete());
}

#[test]
fn frozen_input_quantization_preserves_lengths_edge_notes_and_one_persisted_edit() {
    let (project, track, clip) = fixture();
    let grid = hex_project::ResolvedInputGrid::fixed(
        project.meter_map(),
        hex_project::InputQuantization::Eighth.fixed_step().unwrap(),
    )
    .unwrap();
    let binding = MidiTakeBinding::capture_with_grid(&project, track, clip, grid).unwrap();
    let captured = [
        note(8_900, 9_300, 60),
        note(11_100, 11_700, 64),
        note(15_999, 16_000, 67),
    ];
    let command = finish_take(&project, &binding, &captured, 16_000)
        .unwrap()
        .unwrap();
    let Command::AddMidiNotes { notes, .. } = &command else {
        panic!("midi take command")
    };
    assert_eq!(notes[0].start, beat(1));
    assert_eq!(notes[1].start, beat(2));
    for (quantized, original) in notes[..2].iter().zip(&captured[..2]) {
        let tempo = project.tempo_map();
        assert_eq!(
            quantized.duration.raw(),
            tempo.frame_to_beat(original.end_frame).unwrap().raw()
                - tempo.frame_to_beat(original.start_frame).unwrap().raw()
        );
        assert_eq!((quantized.channel, quantized.velocity), (3, 93));
    }
    assert!(notes[2].duration.raw() > 0);
    let edge_start = BeatTime::from_raw(notes[2].start.raw() - beat(1).raw() + beat(2).raw());
    assert!(project.tempo_map().beat_to_frame(edge_start).unwrap() < 16_000);
    let mut editor = Editor::new(project, 8).unwrap();
    editor.execute(command).unwrap();
    let after = editor.project().snapshot();
    assert_eq!(
        hex_storage::decode(&hex_storage::encode(editor.project()).unwrap())
            .unwrap()
            .snapshot(),
        after
    );
    assert!(editor.undo());
    assert!(!editor.can_undo());
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
    assert_eq!(captured[0], note(8_900, 9_300, 60));
}

#[test]
fn input_quantization_across_faster_tempo_keeps_one_rendered_frame() {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: beat(1),
            tempo: Tempo::new(240_000_000).unwrap(),
        })
        .unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "seam".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beat(2),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let grid =
        hex_project::ResolvedInputGrid::fixed(editor.project().meter_map(), BeatTime::ONE).unwrap();
    let binding = MidiTakeBinding::capture_with_grid(editor.project(), track, clip, grid).unwrap();
    let command = finish_take(editor.project(), &binding, &[note(7_999, 8_000, 60)], 8_000)
        .unwrap()
        .unwrap();
    editor.execute(command).unwrap();
    let inserted = &editor.project().midi_clip(clip).unwrap().notes()[0];
    assert_eq!(inserted.start(), beat(1));
    let prepared = hex_engine::PreparedRevision::prepare(
        editor.project(),
        &std::collections::BTreeMap::new(),
        std::sync::Arc::new(hex_engine::SineTable::new()),
        None,
    );
    assert!(
        prepared.is_ok(),
        "quantized take must remain renderable: {:?}",
        prepared.err()
    );
}
