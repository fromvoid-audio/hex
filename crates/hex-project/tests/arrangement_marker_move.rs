use hex_project::{
    ArrangementSectionSnapshot, BeatTime, Command, Created, Editor, ElementColor, Meter, Project,
    SectionId, Tempo,
};

fn b(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).unwrap()
}

fn fixture(length: u64) -> (Project, SectionId) {
    let mut editor = Editor::new(Project::new(8_000).unwrap(), 32).unwrap();
    let Created::Imported { track, clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "Original.wav".into(),
            sample_rate: 8_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    editor
        .execute(Command::SetClipLooping {
            clip,
            looping: true,
        })
        .unwrap();
    editor
        .execute(Command::SetClipSourcePhase { clip, phase: 173 })
        .unwrap();
    editor
        .execute(Command::ReplaceTrackGainEnvelope {
            track,
            points: vec![
                hex_project::GainPoint {
                    beat: b(0),
                    gain: 0.3,
                    incoming_gain: None,
                    outgoing: hex_project::GainInterpolation::Linear,
                },
                hex_project::GainPoint {
                    beat: b(8),
                    gain: 0.9,
                    incoming_gain: None,
                    outgoing: hex_project::GainInterpolation::Linear,
                },
            ],
        })
        .unwrap();
    let Created::Track(midi) = editor
        .execute(Command::AddTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("midi")
    };
    let Created::Clip(midi_clip) = editor
        .execute(Command::PlaceMidiClip {
            track: midi,
            start: b(1),
            length: b(60),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi_clip,
            start: b(2),
            duration: b(3),
            pitch: 60,
            channel: 0,
            velocity: 90,
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: b(24),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetMeterPoint {
            beat: b(16),
            meter: Meter::new(3, 4).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 100,
            end: 8_000,
            enabled: true,
        })
        .unwrap();
    let mut moved = None;
    for (name, color, start, end) in [
        ("Moving", ElementColor::Blue, 0, length),
        ("Target", ElementColor::Cyan, 32, 40),
        ("Middle", ElementColor::Gray, 44, 46),
        ("Neighbor", ElementColor::Rose, 48, 56),
    ] {
        let Created::ArrangementSection(id) = editor
            .execute(Command::AddArrangementSection {
                name: name.into(),
                color,
                start: b(start),
                end: b(end),
            })
            .unwrap()
        else {
            panic!("section")
        };
        if name == "Moving" {
            moved = Some(id);
        }
    }
    (editor.project().clone(), moved.unwrap())
}

#[test]
fn marker_move_carves_edges_covers_and_splits_with_one_history_edge_and_no_music_edit() {
    // Literal outcomes independently describe half-open overlap. `true` marks
    // the sole new right-hand identity; every other fragment keeps its old ID.
    type ExpectedMarker = (&'static str, u64, u64, bool);
    type MoveCase = (u64, u64, &'static [ExpectedMarker]);
    let cases: &[MoveCase] = &[
        (
            2,
            20,
            &[
                ("Moving", 20, 22, false),
                ("Target", 32, 40, false),
                ("Middle", 44, 46, false),
                ("Neighbor", 48, 56, false),
            ],
        ),
        (
            2,
            30,
            &[
                ("Moving", 30, 32, false),
                ("Target", 32, 40, false),
                ("Middle", 44, 46, false),
                ("Neighbor", 48, 56, false),
            ],
        ),
        (
            2,
            40,
            &[
                ("Target", 32, 40, false),
                ("Moving", 40, 42, false),
                ("Middle", 44, 46, false),
                ("Neighbor", 48, 56, false),
            ],
        ),
        (
            2,
            31,
            &[
                ("Moving", 31, 33, false),
                ("Target", 33, 40, false),
                ("Middle", 44, 46, false),
                ("Neighbor", 48, 56, false),
            ],
        ),
        (
            2,
            39,
            &[
                ("Target", 32, 39, false),
                ("Moving", 39, 41, false),
                ("Middle", 44, 46, false),
                ("Neighbor", 48, 56, false),
            ],
        ),
        (
            2,
            34,
            &[
                ("Target", 32, 34, false),
                ("Moving", 34, 36, false),
                ("Target", 36, 40, true),
                ("Middle", 44, 46, false),
                ("Neighbor", 48, 56, false),
            ],
        ),
        (
            8,
            32,
            &[
                ("Moving", 32, 40, false),
                ("Middle", 44, 46, false),
                ("Neighbor", 48, 56, false),
            ],
        ),
        (
            12,
            30,
            &[
                ("Moving", 30, 42, false),
                ("Middle", 44, 46, false),
                ("Neighbor", 48, 56, false),
            ],
        ),
        (
            24,
            30,
            &[("Moving", 30, 54, false), ("Neighbor", 54, 56, false)],
        ),
        (
            16,
            36,
            &[
                ("Target", 32, 36, false),
                ("Moving", 36, 52, false),
                ("Neighbor", 52, 56, false),
            ],
        ),
    ];
    for &(length, start, expected) in cases {
        let (project, id) = fixture(length);
        let before = project.snapshot();
        let command = Command::MoveArrangementSectionMarker {
            section: id,
            start: b(start),
        };
        assert!(
            !command.requires_section_publication(),
            "metadata-only admission"
        );
        let preview = project.preview_arrangement_section_edit(&command).unwrap();
        assert_eq!(project.snapshot(), before, "preview does not consume IDs");
        assert_eq!(preview.tempo_map(), project.tempo_map());
        let mut editor = Editor::new(project, 8).unwrap();
        editor.execute(command).unwrap();
        assert!(
            !editor.next_section_history_requires_publication(false),
            "metadata undo must not acquire the structural audio gate"
        );
        let after = editor.project().snapshot();
        assert_eq!(after.arrangement_sections.len(), expected.len());
        for ((actual, shown), &(name, start, end, fresh)) in after
            .arrangement_sections
            .iter()
            .zip(preview.sections())
            .zip(expected)
        {
            let old = before
                .arrangement_sections
                .iter()
                .find(|s| s.name == name)
                .unwrap();
            assert_eq!(
                (actual.name.as_str(), actual.start, actual.end, actual.color),
                (name, b(start), b(end), old.color)
            );
            assert_eq!(actual.id, if fresh { before.next_id } else { old.id });
            assert_eq!(
                (shown.name(), shown.start(), shown.end(), shown.color()),
                (name, b(start), b(end), old.color)
            );
            assert_eq!(shown.id().value(), if fresh { 0 } else { old.id });
        }
        let births = expected.iter().filter(|row| row.3).count() as u64;
        assert_eq!(after.next_id, before.next_id + births);
        let mut unchanged = after.clone();
        unchanged.arrangement_sections = before.arrangement_sections.clone();
        unchanged.next_id = before.next_id;
        assert_eq!(
            unchanged, before,
            "clips, audio, MIDI, automation, tempo, meter, loop and routing stay fixed"
        );
        assert!(editor.undo());
        assert!(
            !editor.next_section_history_requires_publication(true),
            "metadata redo must not acquire the structural audio gate"
        );
        let mut restored = before.clone();
        restored.next_id = after.next_id; // Canonical identities never rewind.
        assert_eq!(editor.project().snapshot(), restored);
        assert!(!editor.can_undo(), "exactly one history edge");
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}

#[test]
fn marker_move_refusals_preserve_ids_history_and_strict_resize_contract() {
    let (project, id) = fixture(2);
    let before = project.snapshot();
    let mut editor = Editor::new(project, 8).unwrap();
    assert!(
        editor
            .execute(Command::SetArrangementSectionRange {
                section: id,
                start: b(34),
                end: b(36)
            })
            .is_err()
    );
    assert!(
        editor
            .execute(Command::MoveArrangementSectionMarker {
                section: id,
                start: BeatTime::from_raw(u64::MAX)
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());
    editor
        .execute(Command::MoveArrangementSectionMarker {
            section: id,
            start: b(0),
        })
        .unwrap();
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo(), "stationary move is a no-op");
    editor
        .execute(Command::RemoveArrangementSection(id))
        .unwrap();
    let removed = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::MoveArrangementSectionMarker {
                section: id,
                start: b(20)
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), removed);
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);

    let mut exhausted = before.clone();
    exhausted.next_id = u64::MAX;
    let mut editor = Editor::new(Project::from_snapshot(exhausted.clone()).unwrap(), 8).unwrap();
    assert!(
        editor
            .execute(Command::MoveArrangementSectionMarker {
                section: id,
                start: b(34)
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), exhausted);
    assert!(!editor.can_undo());
}

#[test]
fn contained_marker_move_respects_section_capacity_without_publishing_a_partial_split() {
    let (project, id) = fixture(2);
    let mut snapshot = project.snapshot();
    while snapshot.arrangement_sections.len() < hex_project::MAX_ARRANGEMENT_SECTIONS {
        let start = 64 + snapshot.arrangement_sections.len() as u64 * 2;
        snapshot
            .arrangement_sections
            .push(ArrangementSectionSnapshot {
                id: snapshot.next_id,
                name: "tail".into(),
                color: ElementColor::Gray,
                start: b(start),
                end: b(start + 1),
            });
        snapshot.next_id += 1;
    }
    let project = Project::from_snapshot(snapshot.clone()).unwrap();
    let command = Command::MoveArrangementSectionMarker {
        section: id,
        start: b(34),
    };
    assert!(project.preview_arrangement_section_edit(&command).is_err());
    let mut editor = Editor::new(project, 8).unwrap();
    assert!(editor.execute(command).is_err());
    assert_eq!(editor.project().snapshot(), snapshot);
    assert!(!editor.can_undo());
}

#[test]
fn marker_resize_carves_both_edges_and_cover_without_touching_music_or_history_flags() {
    use hex_project::TrimEdge::{Left, Right};
    for (name, edge, boundary, expected) in [
        (
            "Moving",
            Right,
            36,
            vec![
                ("Moving", 0, 36),
                ("Target", 36, 40),
                ("Middle", 44, 46),
                ("Neighbor", 48, 56),
            ],
        ),
        (
            "Moving",
            Right,
            44,
            vec![("Moving", 0, 44), ("Middle", 44, 46), ("Neighbor", 48, 56)],
        ),
        ("Moving", Right, 60, vec![("Moving", 0, 60)]),
        (
            "Neighbor",
            Left,
            38,
            vec![("Moving", 0, 4), ("Target", 32, 38), ("Neighbor", 38, 56)],
        ),
        (
            "Target",
            Left,
            2,
            vec![
                ("Moving", 0, 2),
                ("Target", 2, 40),
                ("Middle", 44, 46),
                ("Neighbor", 48, 56),
            ],
        ),
        (
            "Target",
            Left,
            34,
            vec![
                ("Moving", 0, 4),
                ("Target", 34, 40),
                ("Middle", 44, 46),
                ("Neighbor", 48, 56),
            ],
        ),
    ] {
        let (project, _) = fixture(4);
        let before = project.snapshot();
        let id = project
            .arrangement_sections()
            .iter()
            .find(|s| s.name() == name)
            .unwrap()
            .id();
        let command = Command::ResizeArrangementSectionMarker {
            section: id,
            edge,
            boundary: b(boundary),
        };
        assert!(!command.requires_section_publication());
        let preview = project.preview_arrangement_section_edit(&command).unwrap();
        assert_eq!(project.snapshot(), before);
        let mut editor = Editor::new(project, 8).unwrap();
        editor.execute(command).unwrap();
        let after = editor.project().snapshot();
        assert_eq!(after.arrangement_sections.len(), expected.len());
        for ((actual, shown), (name, start, end)) in after
            .arrangement_sections
            .iter()
            .zip(preview.sections())
            .zip(expected)
        {
            let original = before
                .arrangement_sections
                .iter()
                .find(|s| s.name == name)
                .unwrap();
            assert_eq!(
                (
                    actual.name.as_str(),
                    actual.start,
                    actual.end,
                    actual.id,
                    actual.color
                ),
                (name, b(start), b(end), original.id, original.color)
            );
            assert_eq!(
                (shown.id().value(), shown.start(), shown.end()),
                (actual.id, actual.start, actual.end)
            );
        }
        let mut music = after.clone();
        music.arrangement_sections = before.arrangement_sections.clone();
        assert_eq!(music, before);
        assert!(!editor.next_section_history_requires_publication(false));
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(!editor.next_section_history_requires_publication(true));
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
        assert!(editor.undo());
        let opposite = if edge == Left {
            before
                .arrangement_sections
                .iter()
                .find(|s| s.id == id.value())
                .unwrap()
                .end
        } else {
            before
                .arrangement_sections
                .iter()
                .find(|s| s.id == id.value())
                .unwrap()
                .start
        };
        assert!(
            editor
                .execute(Command::ResizeArrangementSectionMarker {
                    section: id,
                    edge,
                    boundary: opposite
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo(), "refusal retains redo");
    }
}

#[test]
fn only_marker_commands_leave_every_audio_input_unchanged() {
    let (project, section) = fixture(8);
    let changed = |command: Command| {
        let mut editor = Editor::new(project.clone(), 32).unwrap();
        editor.execute(command).unwrap();
        assert_ne!(editor.project(), &project);
        project.differs_only_in_arrangement_sections(editor.project())
    };
    for command in [
        Command::AddArrangementSection {
            name: "Added".into(),
            color: ElementColor::Green,
            start: b(60),
            end: b(64),
        },
        Command::MoveArrangementSectionMarker {
            section,
            start: b(2),
        },
        Command::RenameArrangementSection {
            section,
            name: "Renamed".into(),
        },
        Command::SetArrangementSectionColor {
            section,
            color: ElementColor::Green,
        },
        Command::RemoveArrangementSection(section),
    ] {
        assert!(changed(command));
    }
    for command in [
        Command::DuplicateArrangementSection(section),
        Command::DeleteArrangementSection(section),
        Command::SetTempoPoint {
            beat: b(4),
            tempo: Tempo::new(100_000_000).unwrap(),
        },
        Command::SetLoopRegion {
            start: 0,
            end: 4_000,
            enabled: true,
        },
    ] {
        assert!(!changed(command));
    }
}
