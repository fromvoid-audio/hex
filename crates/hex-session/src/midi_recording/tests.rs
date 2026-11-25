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
            loop_pass: 0,
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
            loop_pass: 0,
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
            loop_pass: 0,
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
        loop_pass: 0,
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
fn seven_frame_corrected_note_commits_at_the_heard_arrangement_beat() {
    use hex_engine::midi_capture::*;
    let (project, track, clip) = fixture();
    let binding = MidiTakeBinding::capture(&project, track, clip).unwrap();
    let mut take = MidiTake::new_with_latency(
        8_000,
        16_000,
        MidiCaptureClock {
            content_frame: 8_507,
            performance_frame: 100,
            loop_pass: 0,
        },
        7,
    )
    .unwrap();
    let mut event = AcceptedMidiNote {
        source: capture_source(),
        occurrence_id: 1,
        performance_frame: 100,
        event: MidiCaptureEvent::On {
            pitch: 60,
            velocity: 93,
        },
    };
    take.push(event, 8_500).unwrap();
    event.performance_frame = 104;
    event.event = MidiCaptureEvent::Off;
    take.push(event, 8_504).unwrap();
    let end = take
        .finish(MidiCaptureClock {
            content_frame: 8_512,
            performance_frame: 105,
            loop_pass: 0,
        })
        .unwrap();
    let Command::AddMidiNotes { notes, .. } = finish_take(&project, &binding, take.notes(), end)
        .unwrap()
        .unwrap()
    else {
        panic!("midi take command")
    };
    // At 120 bpm and 8 kHz the audible frame is 1/8 beat into the clip,
    // whose source starts at beat 1.
    assert_eq!(notes[0].start, BeatTime::from_ratio(9, 8).unwrap());
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
            loop_pass: 0,
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
            performance_frame: 112,
            loop_pass: 0,
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

#[test]
fn loop_enabled_midi_recording_wraps_across_passes_with_undo_redo_and_save_reopen() {
    use hex_engine::midi_capture::*;
    let (project, track, clip) = fixture();
    let mut editor = Editor::new(project, 8).unwrap();
    // Clip spans beats 2..4 = frames 8_000..16_000 at 120 bpm, 8 kHz.
    editor
        .execute(Command::SetLoopRegion {
            start: 8_000,
            end: 16_000,
            enabled: true,
        })
        .unwrap();
    let before = editor.project().snapshot();
    let binding = MidiTakeBinding::capture(editor.project(), track, clip).unwrap();
    assert_eq!(binding.loop_region(), Some((8_000, 16_000)));
    let origin = MidiCaptureClock {
        content_frame: 8_000,
        performance_frame: 1_000,
        loop_pass: 0,
    };
    let mut take = MidiTake::new_with_latency_and_loop(
        binding.first_frame(),
        binding.end_frame(),
        origin,
        0,
        binding.loop_region(),
    )
    .unwrap();
    let source = capture_source();
    // Pass 0: note A at 14_000..15_000, note B starts at 15_000 and is held across loop wrap.
    take.push(
        AcceptedMidiNote {
            source,
            occurrence_id: 1,
            performance_frame: 7_000,
            event: MidiCaptureEvent::On {
                pitch: 60,
                velocity: 100,
            },
        },
        14_000,
    )
    .unwrap();
    take.push(
        AcceptedMidiNote {
            source,
            occurrence_id: 1,
            performance_frame: 8_000,
            event: MidiCaptureEvent::Off,
        },
        15_000,
    )
    .unwrap();
    take.push(
        AcceptedMidiNote {
            source,
            occurrence_id: 2,
            performance_frame: 8_000,
            event: MidiCaptureEvent::On {
                pitch: 64,
                velocity: 90,
            },
        },
        15_000,
    )
    .unwrap();
    // Pass 1 (after wrapping at 16_000 -> 8_000 at perf 9_000): release note B at 8_500 (perf 9_500),
    // and overdub note C at 9_000..10_000 (perf 10_000..11_000).
    take.push(
        AcceptedMidiNote {
            source,
            occurrence_id: 2,
            performance_frame: 9_500,
            event: MidiCaptureEvent::Off,
        },
        8_500,
    )
    .unwrap();
    take.push(
        AcceptedMidiNote {
            source,
            occurrence_id: 3,
            performance_frame: 10_000,
            event: MidiCaptureEvent::On {
                pitch: 67,
                velocity: 88,
            },
        },
        9_000,
    )
    .unwrap();
    take.push(
        AcceptedMidiNote {
            source,
            occurrence_id: 3,
            performance_frame: 11_000,
            event: MidiCaptureEvent::Off,
        },
        10_000,
    )
    .unwrap();
    // Finish at pass 1, content_frame 11_000 (elapsed 11_000 -> 8000+11000=19000 -> wrap 16000 -> 11000, pass 1).
    let stop_frame = take
        .finish(MidiCaptureClock {
            content_frame: 11_000,
            performance_frame: 12_000,
            loop_pass: 1,
        })
        .unwrap();
    assert_eq!(stop_frame, 16_000);
    assert_eq!(
        take.notes(),
        &[
            RecordedMidiNote {
                start_frame: 9_000,
                end_frame: 10_000,
                pitch: 67,
                channel: 0,
                velocity: 88,
            },
            RecordedMidiNote {
                start_frame: 14_000,
                end_frame: 15_000,
                pitch: 60,
                channel: 0,
                velocity: 100,
            },
            RecordedMidiNote {
                start_frame: 15_000,
                end_frame: 16_000,
                pitch: 64,
                channel: 0,
                velocity: 90,
            },
        ]
    );
    // Changing the Arrangement loop region mid-take refuses without mutating the project.
    let mut mutated = Editor::new(editor.project().clone(), 8).unwrap();
    mutated
        .execute(Command::SetLoopRegion {
            start: 8_000,
            end: 16_000,
            enabled: false,
        })
        .unwrap();
    assert!(finish_take(mutated.project(), &binding, take.notes(), stop_frame).is_err());

    let command = finish_take(editor.project(), &binding, take.notes(), stop_frame)
        .unwrap()
        .unwrap();
    editor.execute(command).unwrap();
    let notes = editor.project().midi_clip(clip).unwrap().notes();
    assert_eq!(notes.len(), 3);
    assert_eq!(
        notes.iter().map(|n| n.pitch()).collect::<Vec<_>>(),
        vec![67, 60, 64]
    );
    let after = editor.project().snapshot();
    let encoded = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&encoded).unwrap().snapshot(), after);
    assert!(editor.undo());
    let mut restored = before;
    restored.next_id = after.next_id;
    assert_eq!(editor.project().snapshot(), restored);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn capture_track_auto_creates_clip_when_empty_and_supports_undo_redo_and_save_reopen() {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "auto".into(),
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
    let before = editor.project().snapshot();
    let binding = MidiTakeBinding::capture_track(
        editor.project(),
        track,
        8_000,
        hex_project::ResolvedInputGrid::off(),
    )
    .unwrap();
    assert!(binding.clip().is_none());
    assert!(binding.is_open_ended());
    assert_eq!(binding.first_frame(), 8_000);
    // Empty take creates no clip and no history edit.
    assert!(
        finish_take(editor.project(), &binding, &[], 12_000)
            .unwrap()
            .is_none()
    );
    let captured = [note(9_000, 10_000, 60), note(11_000, 12_000, 64)];
    let command = finish_take(editor.project(), &binding, &captured, 12_000)
        .unwrap()
        .unwrap();
    assert!(matches!(command, Command::PlaceMidiClipWithNotes { .. }));
    let Created::Clip(new_clip) = editor.execute(command).unwrap() else {
        panic!("auto-created clip")
    };
    let clip = editor.project().midi_clip(new_clip).unwrap();
    assert_eq!(clip.start(), beat(2));
    assert_eq!(clip.length(), beat(1));
    assert_eq!(clip.notes().len(), 2);
    assert_eq!(clip.notes()[0].start(), BeatTime::from_ratio(1, 4).unwrap());
    assert_eq!(clip.notes()[1].start(), BeatTime::from_ratio(3, 4).unwrap());

    let after = editor.project().snapshot();
    let encoded = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&encoded).unwrap().snapshot(), after);
    assert!(editor.undo());
    let mut restored = before;
    restored.next_id = after.next_id;
    assert_eq!(editor.project().snapshot(), restored);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn capture_track_extends_existing_clip_past_end_as_one_atomic_persisted_edit() {
    let (project, track, clip) = fixture();
    let mut editor = Editor::new(project, 8).unwrap();
    let before = editor.project().snapshot();
    // Clip spans beats 2..4 (frames 8_000..16_000). Start recording inside it at frame 12_000
    // and keep playing past frame 16_000 to frame 20_000 (beat 5 -> new clip length 3 beats).
    let binding = MidiTakeBinding::capture_track(
        editor.project(),
        track,
        12_000,
        hex_project::ResolvedInputGrid::off(),
    )
    .unwrap();
    assert_eq!(binding.clip(), Some(clip));
    assert!(binding.is_open_ended());
    let captured = [note(12_000, 14_000, 60), note(16_000, 19_000, 67)];
    let command = finish_take(editor.project(), &binding, &captured, 20_000)
        .unwrap()
        .unwrap();
    assert!(matches!(command, Command::ExtendMidiClipWithNotes { .. }));
    editor.execute(command).unwrap();
    let extended = editor.project().midi_clip(clip).unwrap();
    assert_eq!(extended.start(), beat(2));
    assert_eq!(extended.length(), beat(3));
    assert_eq!(extended.notes().len(), 2);
    assert_eq!(extended.notes()[1].pitch(), 67);
    assert_eq!(extended.notes()[1].start(), beat(3));

    let after = editor.project().snapshot();
    let encoded = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&encoded).unwrap().snapshot(), after);
    assert!(editor.undo());
    let mut restored = before;
    restored.next_id = after.next_id;
    assert_eq!(editor.project().snapshot(), restored);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn console_midi_take_maps_linear_and_looping_passes_quantizes_overwrites_and_replays_pcm() {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "console".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Scene(scene) = editor
        .execute(Command::AddScene { name: "s1".into() })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "lead".into(),
            source_length: beat(4),
        })
        .unwrap()
    else {
        panic!("session clip")
    };
    // Pre-existing notes: pitch 60 at beat 0..2 (will be overlapped on same pitch at beat 1..2)
    // and pitch 67 at beat 2..3 (must remain untouched).
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: beat(2),
            pitch: 60,
            channel: 0,
            velocity: 90,
        })
        .unwrap();
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(2),
            duration: BeatTime::ONE,
            pitch: 67,
            channel: 0,
            velocity: 95,
        })
        .unwrap();

    // Clip launched at performance beat 2 (frame 8_000 at 120 BPM, 8 kHz -> 4_000 frames/beat).
    let origin = hex_engine::SessionMidiOrigin::launch(beat(2));
    let anchor = None;
    // Arrangement loop enabled must refuse Console capture.
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 8_000,
            enabled: true,
        })
        .unwrap();
    assert!(
        MidiTakeBinding::capture_session_clip(
            editor.project(),
            track,
            clip,
            origin,
            anchor,
            8_000,
        )
        .is_err()
    );
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 8_000,
            enabled: false,
        })
        .unwrap();

    // Exhausted one-shot (performance frame 24_000 = beat 6 = elapsed beat 4) must refuse.
    assert!(
        MidiTakeBinding::capture_session_clip(
            editor.project(),
            track,
            clip,
            origin,
            anchor,
            24_000,
        )
        .is_err()
    );

    let before = editor.project().snapshot();
    let grid =
        hex_project::ResolvedInputGrid::fixed(editor.project().meter_map(), BeatTime::ONE).unwrap();
    let binding = MidiTakeBinding::capture_session_clip_with_grid(
        editor.project(),
        track,
        clip,
        origin,
        anchor,
        8_000,
        grid,
    )
    .unwrap();
    assert_eq!(binding.clip(), Some(clip));
    assert_eq!(binding.first_frame(), 8_000);
    assert_eq!(binding.end_frame(), 24_000);
    assert!(matches!(
        binding.capture_route(),
        hex_engine::midi_capture::MidiCaptureRoute::SessionMidi {
            clip: c,
            origin: o,
            clock_anchor: a,
        } if c == clip && o == origin && a == anchor
    ));

    // Record pitch 60 slightly late of clip beat 1 (perf frame 12_100 -> snaps to clip beat 1.0,
    // length 0.5 beat) and pitch 72 at clip beat 3.0..3.5 (perf frames 20_000..22_000).
    let captured = [
        RecordedMidiNote {
            start_frame: 12_100,
            end_frame: 14_100,
            pitch: 60,
            channel: 0,
            velocity: 110,
        },
        RecordedMidiNote {
            start_frame: 20_000,
            end_frame: 22_000,
            pitch: 72,
            channel: 1,
            velocity: 105,
        },
    ];
    let command = finish_take(editor.project(), &binding, &captured, 24_000)
        .unwrap()
        .unwrap();
    assert!(matches!(command, Command::AddMidiNotes { clip: c, .. } if c == clip));
    editor.execute(command).unwrap();

    let updated = editor.project().session_midi_clip(clip).unwrap();
    let notes = updated.source().notes();
    // Pre-existing pitch 60 trimmed to 0..1, recorded pitch 60 at 1..1.5, pre-existing pitch 67 at 2..3, recorded pitch 72 at 3..3.5.
    assert_eq!(notes.len(), 4);
    assert!(
        notes.iter().any(|n| n.pitch() == 60
            && n.start() == BeatTime::ZERO
            && n.duration() == BeatTime::ONE)
    );
    assert!(
        notes
            .iter()
            .any(|n| n.pitch() == 60 && n.start() == BeatTime::ONE && n.velocity() == 110)
    );
    assert!(
        notes
            .iter()
            .any(|n| n.pitch() == 67 && n.start() == beat(2) && n.duration() == BeatTime::ONE)
    );
    assert!(
        notes
            .iter()
            .any(|n| n.pitch() == 72 && n.start() == beat(3) && n.channel() == 1)
    );

    // Verify actual Console clip PCM replay plays the newly recorded pitch 72 note at beat 3!
    let rev = std::sync::Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &std::collections::BTreeMap::new(),
            std::sync::Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = hex_engine::SessionPlayback::new(rev).unwrap();
    playback
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    playback
        .apply(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    let mut pcm = vec![[0.0; 2]; 16_000];
    playback.render(&mut pcm).unwrap();
    let beat3_peak = pcm[12_200..13_500]
        .iter()
        .map(|f| f[0].abs())
        .fold(0.0_f32, f32::max);
    assert!(
        beat3_peak > 0.05,
        "recorded console note must render audible PCM"
    );

    // Save/reopen round trip + single Undo/Redo.
    let after = editor.project().snapshot();
    let encoded = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&encoded).unwrap().snapshot(), after);
    assert!(editor.undo());
    let mut restored = before;
    restored.next_id = after.next_id;
    assert_eq!(editor.project().snapshot(), restored);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);

    // Looping Console clip: record during wrapped loop pass 2 (loop 0..2 beats, launched at beat 0 -> pass 2 is beats 4..6 = frames 16_000..24_000).
    editor
        .execute(Command::SetSessionMidiClipSource {
            clip,
            source_length: beat(4),
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: beat(2),
            looping: true,
        })
        .unwrap();
    let loop_origin = hex_engine::SessionMidiOrigin::launch(BeatTime::ZERO);
    let loop_binding = MidiTakeBinding::capture_session_clip(
        editor.project(),
        track,
        clip,
        loop_origin,
        anchor,
        18_000, // beat 4.5 (pass 2, 0.5 beats into 0..2 loop)
    )
    .unwrap();
    assert_eq!(loop_binding.first_frame(), 16_000);
    assert_eq!(loop_binding.end_frame(), 24_000);
    // Note recorded at frames 18_000..24_000 (beat 4.5..6.0) maps to source beat 0.5..2.0 (closed cleanly at loop_end).
    let loop_cmd = finish_take(
        editor.project(),
        &loop_binding,
        &[RecordedMidiNote {
            start_frame: 18_000,
            end_frame: 24_000,
            pitch: 64,
            channel: 0,
            velocity: 100,
        }],
        24_000,
    )
    .unwrap()
    .unwrap();
    // Stale source refusal: mutating clip source invalidates binding without losing original notes.
    let mut stale_editor = Editor::new(editor.project().clone(), 8).unwrap();
    stale_editor
        .execute(Command::SetClipActive {
            clip,
            active: false,
        })
        .unwrap();
    assert!(
        finish_take(
            stale_editor.project(),
            &loop_binding,
            &[RecordedMidiNote {
                start_frame: 18_000,
                end_frame: 24_000,
                pitch: 64,
                channel: 0,
                velocity: 100,
            }],
            24_000,
        )
        .is_err()
    );
    editor.execute(loop_cmd).unwrap();
    let looped_notes = editor
        .project()
        .session_midi_clip(clip)
        .unwrap()
        .source()
        .notes();
    assert!(looped_notes.iter().any(|n| {
        n.pitch() == 64
            && n.start() == BeatTime::from_ratio(1, 2).unwrap()
            && n.duration() == BeatTime::from_ratio(3, 2).unwrap()
    }));
}

#[test]
fn per_note_expression_take_pairs_distinct_overlapping_notes_commits_replays_and_refuses_invalid() {
    use hex_engine::computer_keyboard::ComputerKeyboardInput;
    use hex_engine::midi_capture::{
        AcceptedMidiNote, MidiCaptureClock, MidiCaptureEvent, MidiCaptureSource,
    };

    let (project, track, arr_clip) = fixture();
    let mut editor = Editor::new(project, 16).unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene { name: "mpe".into() })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(sess_clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "console".into(),
            source_length: beat(4),
        })
        .unwrap()
    else {
        panic!("session clip")
    };

    let hw_input = ComputerKeyboardInput::default()
        .with_live_notes()
        .unwrap()
        .with_recording_support()
        .unwrap();
    let keys_act = hw_input.begin_ordered_keys_on_control().unwrap();
    let hw = hw_input
        .live_notes()
        .unwrap()
        .current_activation_on_control()
        .unwrap();
    let src_ch2 = MidiCaptureSource::Hardware {
        activation: hw,
        channel: 2,
    };
    let src_ch3 = MidiCaptureSource::Hardware {
        activation: hw,
        channel: 3,
    };

    // Assemble one take on Arrangement clip (frames 8_000..16_000 = beats 2..4) with two overlapping
    // notes on different MPE member channels carrying completely distinct PitchBend/Timbre/Pressure!
    let origin = MidiCaptureClock {
        content_frame: 8_000,
        performance_frame: 8_000,
        loop_pass: 0,
    };
    let mut take = MidiTake::new(8_000, 16_000, origin).unwrap();
    let push = |take: &mut MidiTake, source, occ, frame, event| {
        take.push(
            AcceptedMidiNote {
                source,
                occurrence_id: occ,
                performance_frame: frame,
                event,
            },
            frame,
        )
        .unwrap();
    };
    // Note 1 (occ 10, ch 2, pitch 60): 9_000..14_000
    // Note 2 (occ 11, ch 3, pitch 64): 9_500..12_500 (finishes BEFORE Note 1!)
    push(
        &mut take,
        src_ch2,
        10,
        9_000,
        MidiCaptureEvent::On {
            pitch: 60,
            velocity: 100,
        },
    );
    push(
        &mut take,
        src_ch2,
        10,
        9_000,
        MidiCaptureEvent::PitchBend { cents: 200 },
    );
    push(
        &mut take,
        src_ch2,
        10,
        9_000,
        MidiCaptureEvent::Timbre { value: 90 },
    );
    push(
        &mut take,
        src_ch2,
        10,
        9_000,
        MidiCaptureEvent::Pressure { value: 80 },
    );
    push(
        &mut take,
        src_ch3,
        11,
        9_500,
        MidiCaptureEvent::On {
            pitch: 64,
            velocity: 105,
        },
    );
    push(
        &mut take,
        src_ch3,
        11,
        9_500,
        MidiCaptureEvent::PitchBend { cents: -300 },
    );
    push(
        &mut take,
        src_ch3,
        11,
        9_500,
        MidiCaptureEvent::Timbre { value: 30 },
    );
    push(
        &mut take,
        src_ch3,
        11,
        9_500,
        MidiCaptureEvent::Pressure { value: 110 },
    );
    // Time-varying update on Note 1 at frame 11_000
    push(
        &mut take,
        src_ch2,
        10,
        11_000,
        MidiCaptureEvent::PitchBend { cents: 450 },
    );
    // Release Note 2 first, then Note 1
    push(&mut take, src_ch3, 11, 12_500, MidiCaptureEvent::Off);
    push(&mut take, src_ch2, 10, 14_000, MidiCaptureEvent::Off);
    let stop_frame = take
        .finish(MidiCaptureClock {
            content_frame: 15_000,
            performance_frame: 15_000,
            loop_pass: 0,
        })
        .unwrap();
    assert_eq!(take.notes().len(), 2);
    assert_eq!(take.note_expressions().len(), 2);
    // Notes sorted by start_frame -> index 0 is pitch 60 (occ 10), index 1 is pitch 64 (occ 11)!
    assert_eq!(take.notes()[0].pitch, 60);
    assert_eq!(take.note_expressions()[0].initial.pitch_offset_cents, 200);
    assert_eq!(take.note_expressions()[0].points.len(), 2);
    assert_eq!(take.note_expressions()[0].points[1].pitch_offset_cents, 450);
    assert_eq!(take.notes()[1].pitch, 64);
    assert_eq!(take.note_expressions()[1].initial.pitch_offset_cents, -300);
    assert_eq!(take.note_expressions()[1].initial.timbre, 30);
    assert_eq!(take.note_expressions()[1].initial.pressure, 110);

    // Commit to Arrangement clip in one history step + verify save/reopen + single Undo/Redo!
    let before = editor.project().snapshot();
    let arr_binding = MidiTakeBinding::capture(editor.project(), track, arr_clip).unwrap();
    let cmd = finish_take_with_expression(
        editor.project(),
        &arr_binding,
        take.notes(),
        take.note_expressions(),
        stop_frame,
    )
    .unwrap()
    .unwrap();
    assert!(matches!(cmd, Command::AddMidiNotesWithExpression { .. }));
    editor.execute(cmd).unwrap();
    let arr_notes = editor.project().midi_clip(arr_clip).unwrap().notes();
    assert_eq!(arr_notes.len(), 2);
    let n60 = arr_notes.iter().find(|n| n.pitch() == 60).unwrap();
    let n64 = arr_notes.iter().find(|n| n.pitch() == 64).unwrap();
    assert_eq!(
        n60.expression(),
        hex_project::NoteExpression {
            pitch_offset_cents: 200,
            timbre: 90,
            pressure: 80,
        }
    );
    assert_eq!(n60.expression_points().len(), 2);
    assert_eq!(n60.expression_points()[1].pitch_offset_cents, 450);
    assert_eq!(
        n64.expression(),
        hex_project::NoteExpression {
            pitch_offset_cents: -300,
            timbre: 30,
            pressure: 110,
        }
    );
    // Deterministic playback render from prepared revision succeeds cleanly.
    let rev = std::sync::Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &std::collections::BTreeMap::new(),
            std::sync::Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = hex_engine::SessionPlayback::new(rev).unwrap();
    playback
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let mut pcm = vec![[0.0; 2]; 16_000];
    playback.render(&mut pcm).unwrap();
    let peak = pcm[9_200..12_000]
        .iter()
        .map(|f| f[0].abs())
        .fold(0.0_f32, f32::max);
    assert!(peak > 0.05);

    // Save/reopen round trip preserves both expression and expression_points!
    let after = editor.project().snapshot();
    let encoded = hex_storage::encode(editor.project()).unwrap();
    assert_eq!(hex_storage::decode(&encoded).unwrap().snapshot(), after);
    assert!(editor.undo());
    let mut restored = before;
    restored.next_id = after.next_id;
    assert_eq!(editor.project().snapshot(), restored);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);

    // Also commit expressive take to existing Console MIDI clip!
    let sess_binding = MidiTakeBinding::capture_session_clip(
        editor.project(),
        track,
        sess_clip,
        hex_engine::SessionMidiOrigin::launch(beat(2)),
        None,
        8_000,
    )
    .unwrap();
    let sess_cmd = finish_take_with_expression(
        editor.project(),
        &sess_binding,
        take.notes(),
        take.note_expressions(),
        stop_frame,
    )
    .unwrap()
    .unwrap();
    editor.execute(sess_cmd).unwrap();
    let sess_notes = editor
        .project()
        .session_midi_clip(sess_clip)
        .unwrap()
        .source()
        .notes();
    assert_eq!(sess_notes.len(), 2);
    assert_eq!(
        sess_notes
            .iter()
            .find(|n| n.pitch() == 60)
            .unwrap()
            .expression()
            .pitch_offset_cents,
        200
    );
    assert_eq!(
        sess_notes
            .iter()
            .find(|n| n.pitch() == 64)
            .unwrap()
            .expression()
            .pitch_offset_cents,
        -300
    );

    // Refusal cases: invalid pitch bend, computer-keyboard expression, and >64 points must refuse and fault take!
    let mut bad_bend = MidiTake::new(8_000, 16_000, origin).unwrap();
    assert!(
        bad_bend
            .push(
                AcceptedMidiNote {
                    source: src_ch2,
                    occurrence_id: 1,
                    performance_frame: 8_100,
                    event: MidiCaptureEvent::PitchBend { cents: 5_000 },
                },
                8_100,
            )
            .is_err()
    );
    let mut bad_source = MidiTake::new(8_000, 16_000, origin).unwrap();
    assert!(
        bad_source
            .push(
                AcceptedMidiNote {
                    source: MidiCaptureSource::Computer {
                        activation: keys_act,
                        key: 0,
                        press_token: 1,
                    },
                    occurrence_id: 1,
                    performance_frame: 8_100,
                    event: MidiCaptureEvent::Timbre { value: 64 },
                },
                8_100,
            )
            .is_err()
    );
    let mut overflow_pts = MidiTake::new(8_000, 16_000, origin).unwrap();
    push(
        &mut overflow_pts,
        src_ch2,
        1,
        8_000,
        MidiCaptureEvent::On {
            pitch: 60,
            velocity: 100,
        },
    );
    for i in 0..hex_project::MAX_NOTE_EXPRESSION_POINTS {
        push(
            &mut overflow_pts,
            src_ch2,
            1,
            8_001 + i as u64,
            MidiCaptureEvent::Timbre {
                value: (i % 128) as u8,
            },
        );
    }
    assert!(
        overflow_pts
            .push(
                AcceptedMidiNote {
                    source: src_ch2,
                    occurrence_id: 1,
                    performance_frame: 8_001 + hex_project::MAX_NOTE_EXPRESSION_POINTS as u64,
                    event: MidiCaptureEvent::Timbre { value: 1 },
                },
                8_001 + hex_project::MAX_NOTE_EXPRESSION_POINTS as u64,
            )
            .is_err()
    );
}
