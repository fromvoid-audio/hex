use hex_project::{BeatTime, Command, Created, Editor, ElementColor, Project, SectionId, Tempo};
fn b(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn section(editor: &mut Editor, name: &str, start: u64, end: u64) -> SectionId {
    let Created::ArrangementSection(id) = editor
        .execute(Command::AddArrangementSection {
            name: name.into(),
            color: ElementColor::Cyan,
            start: b(start),
            end: b(end),
        })
        .unwrap()
    else {
        panic!("section")
    };
    id
}
fn fixture() -> (Editor, SectionId, SectionId) {
    let mut e = Editor::new(Project::new(8_000).unwrap(), 32).unwrap();
    let Created::Imported { track, clip, .. } = e
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "original.wav".into(),
            sample_rate: 8_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    e.execute(Command::SetClipLooping {
        clip,
        looping: true,
    })
    .unwrap();
    e.execute(Command::SetClipSourcePhase { clip, phase: 173 })
        .unwrap();
    let Created::Track(midi) = e
        .execute(Command::AddTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(midi_clip) = e
        .execute(Command::PlaceMidiClip {
            track: midi,
            start: b(0),
            length: b(12),
        })
        .unwrap()
    else {
        panic!("midi")
    };
    e.execute(Command::SetMidiClipSource {
        clip: midi_clip,
        source_length: b(3),
        source_start: b(0),
        loop_start: b(0),
        loop_end: b(3),
        looping: true,
        length: b(12),
    })
    .unwrap();
    e.execute(Command::AddMidiNote {
        clip: midi_clip,
        start: BeatTime::from_ratio(1, 2).unwrap(),
        duration: b(2),
        pitch: 60,
        channel: 0,
        velocity: 90,
    })
    .unwrap();
    e.execute(Command::ReplaceMidiClipVolume {
        clip: midi_clip,
        volume: hex_project::MidiClipVolume::new(
            vec![
                hex_project::MidiClipVolumePoint::linear(
                    b(0),
                    hex_project::MidiClipGain::new(0.2).unwrap(),
                ),
                hex_project::MidiClipVolumePoint::linear(
                    b(1),
                    hex_project::MidiClipGain::new(0.9).unwrap(),
                ),
                hex_project::MidiClipVolumePoint::linear(
                    b(3),
                    hex_project::MidiClipGain::new(0.4).unwrap(),
                ),
            ],
            b(3),
        )
        .unwrap(),
    })
    .unwrap();
    assert_eq!(e.project().track(track).unwrap().clips().len(), 1);
    let a = section(&mut e, "intro", 2, 4);
    let c = section(&mut e, "chorus", 7, 10);
    (Editor::new(e.project().clone(), 32).unwrap(), a, c)
}
fn events(project: &Project) -> Vec<(u64, u64, u8)> {
    let mut out = Vec::new();
    for clip in project.tracks().iter().flat_map(|t| t.midi_clips()) {
        for note in clip.notes() {
            for occurrence in clip
                .source_playback()
                .unwrap()
                .note_occurrences(note.start(), note.duration())
                .unwrap()
                .iter()
            {
                out.push((
                    clip.start().raw() + occurrence.timeline_start().raw(),
                    occurrence.gate_duration().raw(),
                    note.pitch(),
                ));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn section_drag_copies_use_stable_destinations_fresh_identity_and_one_history_step() {
    for (copy_chorus, target_chorus, after, intro, chorus, copy_range) in [
        (false, false, false, (4, 6), (9, 12), (2, 4)),
        (false, false, true, (2, 4), (9, 12), (4, 6)),
        (false, true, false, (2, 4), (9, 12), (7, 9)),
        (false, true, true, (2, 4), (7, 10), (10, 12)),
        (true, false, false, (5, 7), (10, 13), (2, 5)),
        (true, false, true, (2, 4), (10, 13), (4, 7)),
    ] {
        let (mut editor, a, c) = fixture();
        let source = if copy_chorus { c } else { a };
        let original = editor
            .project()
            .arrangement_section(source)
            .unwrap()
            .clone();
        let before = editor.project().clone();
        let command = Command::DuplicateArrangementSectionAt {
            section: source,
            target: if target_chorus { c } else { a },
            after,
        };
        assert!(command.requires_section_publication());
        let preview = editor
            .project()
            .preview_arrangement_section_edit(&command)
            .unwrap();
        let Created::ArrangementSection(copy) = editor.execute(command).unwrap() else {
            panic!("copy identity");
        };
        assert!(copy != a && copy != c);
        for (id, range) in [(a, intro), (c, chorus), (copy, copy_range)] {
            let marker = editor.project().arrangement_section(id).unwrap();
            assert_eq!((marker.start(), marker.end()), (b(range.0), b(range.1)));
        }
        let copied = editor.project().arrangement_section(copy).unwrap();
        assert_eq!(copied.name(), original.name());
        assert!(ElementColor::ALL.contains(&copied.color()));
        let shape = |markers: &[hex_project::ArrangementSection]| {
            markers
                .iter()
                .map(|s| (s.name().to_owned(), s.color(), s.start(), s.end()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            shape(preview.sections()),
            shape(editor.project().arrangement_sections())
        );
        assert_eq!(preview.tempo_map(), editor.project().tempo_map());
        let after = editor.project().clone();
        assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
        assert!(editor.undo());
        let mut restored = editor.project().snapshot();
        assert!(
            restored.next_id >= after.snapshot().next_id,
            "undo must not reuse allocated IDs"
        );
        restored.next_id = before.snapshot().next_id;
        assert_eq!(restored, before.snapshot());
        assert!(
            !editor.can_undo(),
            "copy and insertion share one history entry"
        );
        assert!(editor.redo());
        assert_eq!(editor.project(), &after);
    }
}

#[test]
fn section_clip_layout_preview_matches_the_committed_candidate_without_mutation() {
    use hex_project::TrimEdge;
    let (editor, a, c) = fixture();
    for command in [
        Command::ReorderSectionAfter {
            section: a,
            after: c,
        },
        Command::ReorderSectionBefore {
            section: c,
            before: a,
        },
        Command::ResizeArrangementSection {
            section: a,
            edge: TrimEdge::Right,
            length: b(3),
        },
        Command::ResizeArrangementSectionEdge {
            section: c,
            edge: TrimEdge::Left,
            boundary: b(8),
        },
        Command::DeleteArrangementSection(a),
        Command::DuplicateArrangementSection(c),
        Command::DuplicateArrangementSectionAt {
            section: c,
            target: a,
            after: false,
        },
    ] {
        let before = editor.project().clone();
        let preview = before.preview_section_layout(&command).unwrap();
        assert_eq!(editor.project(), &before, "preview must not mutate");
        let mut committed = Editor::new(before.clone(), 32).unwrap();
        committed.execute(command.clone()).unwrap();
        assert_ne!(&preview, &before, "{command:?} must change the layout");
        assert_eq!(&preview, committed.project(), "{command:?}");
    }
    let marker_only = Command::MoveArrangementSectionMarker {
        section: a,
        start: b(1),
    };
    assert!(
        editor
            .project()
            .preview_section_layout(&marker_only)
            .is_err()
    );
    let mut stale = Editor::new(editor.project().clone(), 32).unwrap();
    stale.execute(Command::RemoveArrangementSection(a)).unwrap();
    assert!(
        stale
            .project()
            .preview_section_layout(&Command::DeleteArrangementSection(a))
            .is_err()
    );
}

#[test]
fn stale_section_copy_destinations_preserve_the_project_and_redo() {
    let (mut editor, source, stale) = fixture();
    editor
        .execute(Command::RemoveArrangementSection(stale))
        .unwrap();
    editor
        .execute(Command::RenameArrangementSection {
            section: source,
            name: "temporary".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().snapshot();
    for (section, target) in [(source, stale), (stale, source)] {
        let command = Command::DuplicateArrangementSectionAt {
            section,
            target,
            after: true,
        };
        assert!(
            editor
                .project()
                .preview_arrangement_section_edit(&command)
                .is_err()
        );
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
    }
}
#[test]
fn metadata_is_marker_only_and_overlap_refusals_preserve_redo() {
    let (mut e, a, c) = fixture();
    let tracks = e.project().tracks().to_vec();
    e.execute(Command::RenameArrangementSection {
        section: a,
        name: "Introduction".into(),
    })
    .unwrap();
    e.execute(Command::SetArrangementSectionColor {
        section: a,
        color: ElementColor::Rose,
    })
    .unwrap();
    e.execute(Command::SetArrangementSectionRange {
        section: a,
        start: b(1),
        end: b(5),
    })
    .unwrap();
    assert_eq!(e.project().tracks(), tracks);
    assert_eq!(
        e.project().arrangement_section(a).unwrap().name(),
        "Introduction"
    );
    assert!(e.undo());
    let before = e.project().snapshot();
    assert!(e.can_redo());
    for cmd in [
        Command::SetArrangementSectionRange {
            section: a,
            start: b(3),
            end: b(8),
        },
        Command::AddArrangementSection {
            name: "overlap".into(),
            color: ElementColor::Red,
            start: b(8),
            end: b(9),
        },
        Command::SetArrangementSectionRange {
            section: c,
            start: b(7),
            end: b(7),
        },
    ] {
        assert!(e.execute(cmd).is_err());
        assert_eq!(e.project().snapshot(), before);
        assert!(e.can_redo());
    }
    let loaded = Project::from_snapshot(e.project().snapshot()).unwrap();
    assert_eq!(loaded, e.project().clone());
}
#[test]
fn unequal_gapped_both_direction_reorders_move_all_audio_midi_crossings_with_one_history_edge() {
    for later_first in [false, true] {
        let (mut e, a, c) = fixture();
        let before = e.project().snapshot();
        let original = e.project().tracks()[0].clips()[0].clone();
        let (source, target) = if later_first { (c, a) } else { (a, c) };
        let preview = e.project().preview_section_reorder(source, target).unwrap();
        let cuts = [
            preview.source_start().raw(),
            preview.source_end().raw(),
            preview.insertion_beat().raw(),
        ];
        let mut expected = Vec::new();
        for (start, length, pitch) in events(e.project()) {
            let end = start + length;
            let mut boundaries = vec![start, end];
            boundaries.extend(cuts.into_iter().filter(|x| start < *x && *x < end));
            boundaries.sort();
            boundaries.dedup();
            for pair in boundaries.windows(2) {
                expected.push((
                    preview.map_beat(BeatTime::from_raw(pair[0])).unwrap().raw(),
                    pair[1] - pair[0],
                    pitch,
                ));
            }
        }
        expected.sort();
        e.execute(Command::ReorderSectionBefore {
            section: source,
            before: target,
        })
        .unwrap();
        assert_eq!(events(e.project()), expected);
        let old = Project::from_snapshot(before.clone()).unwrap();
        let old_midi = &old.tracks()[1].midi_clips()[0];
        for numerator in 0..48 {
            let at = BeatTime::from_ratio(numerator, 4).unwrap();
            let destination = preview.map_beat(at).unwrap();
            let original_phase = old_midi
                .source_playback()
                .unwrap()
                .source_phase_at_elapsed(at)
                .unwrap();
            let expected_gain = old_midi.volume().gain_at(original_phase).linear();
            let moved = e.project().tracks()[1]
                .midi_clips()
                .iter()
                .find(|clip| {
                    clip.start() <= destination
                        && destination.raw() < clip.start().raw() + clip.length().raw()
                })
                .unwrap();
            let phase = moved
                .source_playback()
                .unwrap()
                .source_phase_at_elapsed(BeatTime::from_raw(
                    destination.raw() - moved.start().raw(),
                ))
                .unwrap();
            assert!((moved.volume().gain_at(phase).linear() - expected_gain).abs() < 1e-12);
        }
        let prefix = e.project().midi_clip(old_midi.id()).unwrap();
        assert_eq!(prefix.source(), old_midi.source());
        assert_eq!(prefix.volume(), old_midi.volume());

        let audio = e.project().tracks()[0].clips();
        assert_eq!(audio.len(), 4);
        assert_eq!(
            audio.iter().map(|c| c.duration()).sum::<u64>(),
            original.duration()
        );
        for clip in audio {
            assert_eq!(clip.asset(), original.asset());
            assert!(clip.looping());
            assert_eq!(clip.source_length(), original.source_length());
        }
        assert_eq!(e.project().arrangement_sections(), preview.sections());
        assert_eq!(e.project().tempo_map(), preview.tempo_map());
        let after = e.project().snapshot();
        assert_eq!(
            Project::from_snapshot(after.clone()).unwrap().snapshot(),
            after
        );
        assert!(e.undo());
        let mut restored = e.project().snapshot();
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        assert!(!e.can_undo());
        assert!(e.redo());
        assert_eq!(e.project().snapshot(), after);
    }
}

#[test]
fn first_section_can_move_after_last_across_unequal_gapped_sections_with_history() {
    let (mut e, first, last) = fixture();
    let before = e.project().snapshot();
    let original_events = events(e.project());
    let audio = e.project().tracks()[0].clips()[0].clone();
    let preview = e
        .project()
        .preview_section_reorder_after(first, last)
        .unwrap();
    assert_eq!(preview.insertion_beat(), b(10));
    assert_eq!(preview.destination_start(), b(8));

    let cuts = [
        preview.source_start().raw(),
        preview.source_end().raw(),
        preview.insertion_beat().raw(),
    ];
    let mut expected = Vec::new();
    for (start, length, pitch) in original_events {
        let end = start + length;
        let mut boundaries = vec![start, end];
        boundaries.extend(cuts.into_iter().filter(|cut| start < *cut && *cut < end));
        boundaries.sort();
        boundaries.dedup();
        for pair in boundaries.windows(2) {
            expected.push((
                preview.map_beat(BeatTime::from_raw(pair[0])).unwrap().raw(),
                pair[1] - pair[0],
                pitch,
            ));
        }
    }
    expected.sort();
    e.execute(Command::ReorderSectionAfter {
        section: first,
        after: last,
    })
    .unwrap();

    let sections = e.project().arrangement_sections();
    assert_eq!(
        sections.iter().map(|s| s.id()).collect::<Vec<_>>(),
        [last, first]
    );
    assert_eq!(
        e.project().arrangement_section(first).unwrap().start(),
        b(8)
    );
    assert_eq!(e.project().arrangement_section(first).unwrap().end(), b(10));
    assert_eq!(events(e.project()), expected);
    let moved_audio = e.project().tracks()[0]
        .clips()
        .iter()
        .find(|clip| clip.start() == e.project().tempo_map().beat_to_frame(b(8)).unwrap())
        .unwrap();
    assert_eq!(moved_audio.asset(), audio.asset());
    assert_eq!(
        moved_audio.audio_edit().map(|edit| &edit.domain),
        audio.audio_edit().map(|edit| &edit.domain)
    );

    let after = e.project().snapshot();
    assert_eq!(
        Project::from_snapshot(after.clone()).unwrap().snapshot(),
        after
    );
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}

#[test]
fn prepare_failure_and_noop_preserve_all_ids_history_and_redo() {
    let (mut e, a, c) = fixture();
    e.execute(Command::RenameArrangementSection {
        section: a,
        name: "changed".into(),
    })
    .unwrap();
    assert!(e.undo());
    let before = e.project().snapshot();
    let result = e.execute_prepared::<(), hex_project::Invalid>(
        Command::ReorderSectionBefore {
            section: c,
            before: a,
        },
        |_| Err(hex_project::Invalid("injected preparation refusal")),
    );
    assert!(result.is_err());
    assert_eq!(e.project().snapshot(), before);
    assert!(e.can_redo());
    assert!(!e.can_undo());
    e.execute(Command::ReorderSectionBefore {
        section: a,
        before: a,
    })
    .unwrap();
    assert_eq!(e.project().snapshot(), before);
    assert!(e.can_redo());
}
#[test]
fn tempo_meter_and_loop_follow_explicit_song_permutation_policy() {
    let (mut e, a, c) = fixture();
    for (beat, bpm) in [(0, 120), (3, 80), (6, 150), (9, 90)] {
        e.execute(Command::SetTempoPoint {
            beat: b(beat),
            tempo: Tempo::new(bpm * 1_000_000).unwrap(),
        })
        .unwrap();
    }
    e.execute(Command::SetMeterPoint {
        beat: b(8),
        meter: hex_project::Meter::new(3, 4).unwrap(),
    })
    .unwrap();
    let start = e.project().tempo_map().beat_to_frame(b(1)).unwrap();
    let end = e.project().tempo_map().beat_to_frame(b(11)).unwrap();
    e.execute(Command::SetLoopRegion {
        start,
        end,
        enabled: true,
    })
    .unwrap();
    let old = e.project().clone();
    let preview = old.preview_section_reorder(c, a).unwrap();
    e.execute(Command::ReorderSectionBefore {
        section: c,
        before: a,
    })
    .unwrap();
    for beat in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] {
        let mapped = preview.map_beat(b(beat)).unwrap();
        assert_eq!(
            old.tempo_map().tempo_at(b(beat)),
            e.project().tempo_map().tempo_at(mapped)
        );
        assert_eq!(
            old.meter_map().meter_at(b(beat)),
            e.project().meter_map().meter_at(mapped)
        );
    }
    let region = e.project().loop_region().unwrap();
    assert_eq!(
        region.start(),
        e.project().tempo_map().beat_to_frame(b(1)).unwrap()
    );
    assert_eq!(
        region.end(),
        e.project().tempo_map().beat_to_frame(b(11)).unwrap()
    );
    assert!(
        e.project()
            .meter_map()
            .points()
            .iter()
            .any(|p| p.beat() == b(3) && p.meter() == hex_project::Meter::new(3, 4).unwrap())
    );
}

#[test]
fn bounded_gain_splice_and_dense_midi_expansion_fail_without_consuming_source_or_ids() {
    let (mut seed, a, c) = fixture();
    let mut snapshot = seed.project().snapshot();
    snapshot.tracks[0].gain_envelope = (0..hex_project::MAX_GAIN_POINTS_PER_TRACK)
        .map(|i| hex_project::GainPoint {
            beat: BeatTime::from_raw(i as u64),
            gain: 1.0,
            incoming_gain: None,
            outgoing: hex_project::GainInterpolation::Linear,
        })
        .collect();
    seed = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    let before = seed.project().snapshot();
    for command in [
        Command::ReorderSectionBefore {
            section: c,
            before: a,
        },
        Command::ResizeArrangementSection {
            section: a,
            edge: hex_project::TrimEdge::Right,
            length: b(3),
        },
        Command::DeleteArrangementSection(a),
        Command::DuplicateArrangementSection(a),
    ] {
        assert!(seed.execute(command).is_err());
        assert_eq!(seed.project().snapshot(), before);
    }
    assert_eq!(seed.project().snapshot(), before);
    assert!(!seed.can_undo());
    let mut e = Editor::new(Project::new(8_000).unwrap(), 8).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "dense".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = e
        .execute(Command::PlaceMidiClip {
            track,
            start: b(0),
            length: b(12),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let raw = BeatTime::from_raw(1);
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: raw,
        source_start: b(0),
        loop_start: b(0),
        loop_end: raw,
        looping: true,
        length: b(12),
    })
    .unwrap();
    e.execute(Command::AddMidiNote {
        clip,
        start: b(0),
        duration: raw,
        pitch: 60,
        channel: 0,
        velocity: 100,
    })
    .unwrap();
    let a = section(&mut e, "intro", 2, 4);
    let c = section(&mut e, "chorus", 7, 10);
    let before = e.project().snapshot();
    let mut e = Editor::new(e.project().clone(), 8).unwrap();
    for command in [
        Command::ReorderSectionBefore {
            section: c,
            before: a,
        },
        Command::ResizeArrangementSection {
            section: a,
            edge: hex_project::TrimEdge::Right,
            length: b(3),
        },
        Command::DeleteArrangementSection(a),
        Command::DuplicateArrangementSection(a),
    ] {
        assert!(e.execute(command).is_err());
        assert_eq!(e.project().snapshot(), before);
    }
    assert_eq!(e.project().snapshot(), before);
    assert!(!e.can_undo());
}

#[test]
fn outer_audio_placements_section_edits_are_atomic_and_retain_history() {
    for mode in [
        hex_project::AudioViewMode::Once,
        hex_project::AudioViewMode::Repeat,
    ] {
        let (seed, a, c) = fixture();
        let clip = seed.project().tracks()[0].clips()[0].clone();
        let mut edit = clip.audio_event_root();
        edit.view_start = 271;
        edit.view_end = 4_271;
        edit.placement = Some(hex_project::AudioViewPlacement {
            mode,
            outer_length: hex_project::AudioDomainClock::Frames(clip.duration()),
            cycle_phase: 173,
        });
        let mut snapshot = seed.project().snapshot();
        snapshot.tracks[0].audio_clips_mut().unwrap()[0].edit = Some(edit);
        let seed = Project::from_snapshot(snapshot).unwrap();
        for command in [
            Command::ReorderSectionBefore {
                section: c,
                before: a,
            },
            Command::DuplicateArrangementSection(a),
            Command::DeleteArrangementSection(a),
            Command::ResizeArrangementSection {
                section: a,
                edge: hex_project::TrimEdge::Left,
                length: b(3),
            },
        ] {
            let mut e = Editor::new(seed.clone(), 8).unwrap();
            let before = e.project().snapshot();
            let mut prepared = false;
            e.execute_prepared::<(), hex_project::Invalid>(command.clone(), |_| {
                prepared = true;
                Err(hex_project::Invalid("injected preparation refusal"))
            })
            .unwrap_err();
            assert!(prepared);
            assert_eq!(e.project().snapshot(), before);
            assert!(!e.can_undo());
            e.execute(command).unwrap();
            let after = e.project().snapshot();
            assert_ne!(after, before);
            assert_eq!(
                Project::from_snapshot(after.clone()).unwrap().snapshot(),
                after
            );
            assert!(e.undo());
            let mut undone = e.project().snapshot();
            undone.next_id = before.next_id;
            assert_eq!(undone, before);
            assert!(e.redo());
            assert_eq!(e.project().snapshot(), after);
        }
    }
}

#[test]
fn same_signature_midbar_anchors_retain_bar_origins_after_section_reorder() {
    let (mut e, a, c) = fixture();
    e.execute(Command::SetMeterPoint {
        beat: b(3),
        meter: hex_project::Meter::COMMON_TIME,
    })
    .unwrap();
    e.execute(Command::SetTempoPoint {
        beat: b(3),
        tempo: Tempo::DEFAULT,
    })
    .unwrap();
    assert_eq!(
        e.project().meter_map().position(b(3)).unwrap().bar_start(),
        b(3)
    );
    let preview = e.project().preview_section_reorder(c, a).unwrap();
    let marker = preview.map_beat(b(3)).unwrap();
    assert_eq!(marker, b(6));
    e.execute(Command::ReorderSectionBefore {
        section: c,
        before: a,
    })
    .unwrap();
    assert!(
        e.project()
            .meter_map()
            .points()
            .iter()
            .any(|p| p.beat() == marker && p.meter() == hex_project::Meter::COMMON_TIME)
    );
    assert!(
        e.project()
            .tempo_map()
            .points()
            .iter()
            .any(|p| p.beat() == marker && p.tempo() == Tempo::DEFAULT)
    );
    let position = e.project().meter_map().position(marker).unwrap();
    assert_eq!(position.bar_start(), marker);
    assert_eq!(position.beat(), 1);
    assert_eq!(
        e.project().meter_map().points(),
        &[
            hex_project::MeterPoint::new(b(0), hex_project::Meter::COMMON_TIME),
            hex_project::MeterPoint::new(marker, hex_project::Meter::COMMON_TIME),
        ],
        "retain the authored bar reset, without synthetic same-meter seams"
    );
}

// Intended to append to crates/hex-project/tests/arrangement_sections.rs when
// the canonical section splice commands are integrated.

#[test]
fn end_growth_inserts_empty_time_without_stretching_audio_or_rephasing_midi() {
    let (mut e, section_id, _) = fixture();
    let before_events = events(e.project());
    let original_audio = e.project().tracks()[0].clips()[0].clone();
    e.execute(Command::SetTempoPoint {
        beat: b(5),
        tempo: Tempo::new(80_000_000).unwrap(),
    })
    .unwrap();
    let loop_start = e.project().tempo_map().beat_to_frame(b(1)).unwrap();
    let loop_end = e.project().tempo_map().beat_to_frame(b(11)).unwrap();
    e.execute(Command::SetLoopRegion {
        start: loop_start,
        end: loop_end,
        enabled: true,
    })
    .unwrap();
    let preview = e
        .project()
        .preview_arrangement_section_edit(&Command::ResizeArrangementSection {
            section: section_id,
            edge: hex_project::TrimEdge::Right,
            length: b(4),
        })
        .unwrap();
    e.execute(Command::ResizeArrangementSection {
        section: section_id,
        edge: hex_project::TrimEdge::Right,
        length: b(4),
    })
    .unwrap();

    let marker = e.project().arrangement_section(section_id).unwrap();
    assert_eq!((marker.start(), marker.end()), (b(2), b(6)));
    // The two-beat insertion is empty; it does not stretch either retained
    // audio fragment, and the original tempo change follows the suffix.
    assert_eq!(e.project().tempo_map().tempo_at(b(4)), Tempo::DEFAULT);
    assert_eq!(e.project().tempo_map().tempo_at(b(5)), Tempo::DEFAULT);
    assert_eq!(
        e.project().tempo_map().tempo_at(b(7)),
        Tempo::new(80_000_000).unwrap()
    );
    let loop_region = e.project().loop_region().unwrap();
    assert_eq!(
        loop_region.start(),
        e.project().tempo_map().beat_to_frame(b(1)).unwrap()
    );
    assert_eq!(
        loop_region.end(),
        e.project().tempo_map().beat_to_frame(b(11)).unwrap()
    );
    assert!(loop_region.enabled());
    let clips = e.project().tracks()[0].clips();
    assert_eq!(clips.len(), 2);
    assert_eq!((clips[0].start(), clips[0].duration()), (0, 16_000));
    assert_eq!((clips[1].start(), clips[1].duration()), (24_000, 32_000));
    assert_eq!(clips[0].asset(), original_audio.asset());
    assert_eq!(clips[1].asset(), original_audio.asset());
    assert_eq!(clips[0].source_phase(), original_audio.source_phase());
    assert_eq!(
        clips[1].source_phase(),
        (original_audio.source_phase() + 16_000) % 48_000
    );
    assert!(
        events(e.project())
            .iter()
            .any(|event| event.0 >= b(6).raw())
    );
    assert_ne!(events(e.project()), before_events);
    assert_eq!(e.project().arrangement_sections(), preview.sections());
    assert_eq!(e.project().tempo_map(), preview.tempo_map());
}

#[test]
fn start_growth_inserts_prefix_and_keeps_the_section_start_fixed() {
    let (mut e, section_id, _) = fixture();
    e.execute(Command::ResizeArrangementSection {
        section: section_id,
        edge: hex_project::TrimEdge::Left,
        length: b(4),
    })
    .unwrap();

    let marker = e.project().arrangement_section(section_id).unwrap();
    assert_eq!((marker.start(), marker.end()), (b(2), b(6)));
    let clips = e.project().tracks()[0].clips();
    assert_eq!(clips.len(), 2);
    assert_eq!((clips[0].start(), clips[0].duration()), (0, 8_000));
    assert_eq!((clips[1].start(), clips[1].duration()), (16_000, 40_000));
    assert_eq!(clips[0].source_phase(), 173);
    assert_eq!(clips[1].source_phase(), 8_173);
    // A MIDI gate crossing the inserted boundary is split at the seam; later
    // events retain their source phase and move by exactly two beats.
    let all_events = events(e.project());
    assert!(all_events.iter().any(|event| event.0 == b(4).raw()));
    assert!(all_events.iter().any(|event| event.0 >= b(6).raw()));
}

#[test]
fn both_edge_shrinks_remove_time_at_the_edge_and_close_the_gap() {
    for edge in [hex_project::TrimEdge::Left, hex_project::TrimEdge::Right] {
        let (mut e, section_id, _) = fixture();
        let target_length = if edge == hex_project::TrimEdge::Left {
            // Make the old section longer than the new one so this catches
            // deleting target_length instead of old_length - target_length.
            e.execute(Command::SetArrangementSectionRange {
                section: section_id,
                start: b(2),
                end: b(6),
            })
            .unwrap();
            b(3)
        } else {
            b(1)
        };
        e.execute(Command::ResizeArrangementSection {
            section: section_id,
            edge,
            length: target_length,
        })
        .unwrap();

        let marker = e.project().arrangement_section(section_id).unwrap();
        assert_eq!(
            (marker.start(), marker.end()),
            (
                b(2),
                BeatTime::from_raw(b(2).raw().checked_add(target_length.raw()).unwrap())
            )
        );
        let clips = e.project().tracks()[0].clips();
        assert_eq!(clips.len(), 2);
        let expected = if edge == hex_project::TrimEdge::Left {
            [(0, 8_000), (8_000, 36_000)]
        } else {
            [(0, 12_000), (12_000, 32_000)]
        };
        assert_eq!(
            [
                (clips[0].start(), clips[0].duration()),
                (clips[1].start(), clips[1].duration()),
            ],
            expected
        );
        let phase_delta = if edge == hex_project::TrimEdge::Left {
            12_000
        } else {
            16_000
        };
        assert_eq!(clips[0].source_phase(), 173);
        assert_eq!(clips[1].source_phase(), (173 + phase_delta) % 48_000);
    }
}

#[test]
fn deleting_a_section_removes_its_time_and_ripples_both_tracks_left() {
    let (mut e, section_id, _) = fixture();
    let audio_track = e.project().tracks()[0].id();
    e.execute(Command::ReplaceTrackGainEnvelope {
        track: audio_track,
        points: vec![
            hex_project::GainPoint {
                beat: b(0),
                gain: 1.0,
                incoming_gain: None,
                outgoing: hex_project::GainInterpolation::Linear,
            },
            hex_project::GainPoint {
                beat: b(2),
                gain: 0.8,
                incoming_gain: Some(0.6),
                outgoing: hex_project::GainInterpolation::Linear,
            },
            hex_project::GainPoint {
                beat: b(4),
                gain: 0.3,
                incoming_gain: Some(0.2),
                outgoing: hex_project::GainInterpolation::Step,
            },
        ],
    })
    .unwrap();
    let before = e.project().snapshot();
    let preview = e
        .project()
        .preview_arrangement_section_edit(&Command::DeleteArrangementSection(section_id))
        .unwrap();
    e.execute(Command::DeleteArrangementSection(section_id))
        .unwrap();

    assert!(e.project().arrangement_section(section_id).is_none());
    let clips = e.project().tracks()[0].clips();
    assert_eq!(clips.len(), 2);
    assert_eq!((clips[0].start(), clips[0].duration()), (0, 8_000));
    assert_eq!((clips[1].start(), clips[1].duration()), (8_000, 32_000));
    assert_eq!(clips[0].source_phase(), 173);
    assert_eq!(clips[1].source_phase(), 16_173);
    let seam = e.project().track(audio_track).unwrap().gain_lane().base()[1];
    assert_eq!(seam.beat, b(2));
    assert_eq!(seam.incoming_gain, Some(0.6));
    assert_eq!(seam.gain, 0.3);
    assert_eq!(e.project().arrangement_sections(), preview.sections());
    assert_eq!(e.project().tempo_map(), preview.tempo_map());
    assert!(
        events(e.project())
            .iter()
            .any(|event| { event.0 == BeatTime::from_ratio(9, 2).unwrap().raw() })
    );

    let after = e.project().snapshot();
    assert_eq!(
        Project::from_snapshot(after.clone()).unwrap().snapshot(),
        after
    );
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}

#[test]
fn duplication_copies_midi_gain_touch_tempo_and_meter_with_fresh_identities() {
    let (mut e, _, section_id) = fixture();
    e.execute(Command::SetTempoPoint {
        beat: b(8),
        tempo: Tempo::new(90_000_000).unwrap(),
    })
    .unwrap();
    e.execute(Command::SetMeterPoint {
        beat: b(8),
        meter: hex_project::Meter::new(3, 4).unwrap(),
    })
    .unwrap();
    let audio_track = e.project().tracks()[0].id();
    e.execute(Command::ReplaceTrackGainEnvelope {
        track: audio_track,
        points: vec![
            hex_project::GainPoint {
                beat: b(0),
                gain: 1.0,
                incoming_gain: None,
                outgoing: hex_project::GainInterpolation::Linear,
            },
            hex_project::GainPoint {
                beat: b(8),
                gain: 0.5,
                incoming_gain: None,
                outgoing: hex_project::GainInterpolation::Linear,
            },
        ],
    })
    .unwrap();
    e.execute(Command::SpliceGainTouch {
        track: audio_track,
        expected_epoch: 0,
        span: hex_project::GainTouchSpan {
            source_start: b(7),
            start: b(7),
            end: b(9),
            context: hex_project::GainTouchContext {
                recording_epoch: 1,
                project_generation: 0,
                canonical_revision: 1,
                audio_revision: 1,
                transport_epoch: 1,
                gesture_epoch: 1,
                gesture_serial: 1,
                sample_rate: 8_000,
                loop_pass: 0,
            },
            steps: vec![
                hex_project::GainTouchStep {
                    beat: b(7),
                    gain: 0.4,
                    origin_beat: b(7),
                    source_sequence: 1,
                },
                hex_project::GainTouchStep {
                    beat: b(8),
                    gain: 0.8,
                    origin_beat: b(8),
                    source_sequence: 2,
                },
            ],
        },
    })
    .unwrap();
    let old_midi = e.project().tracks()[1].midi_clips()[0].clone();
    let old_events = events(e.project());
    let expected_duplicated: Vec<_> = old_events
        .iter()
        .filter(|(start, _, _)| b(7).raw() <= *start && *start < b(10).raw())
        .map(|(start, duration, pitch)| {
            let kept_end = (*start + *duration).min(b(10).raw());
            (*start + b(3).raw(), kept_end - *start, *pitch)
        })
        .collect();
    let before = e.project().snapshot();
    let preview = e
        .project()
        .preview_arrangement_section_edit(&Command::DuplicateArrangementSection(section_id))
        .unwrap();

    let Created::ArrangementSection(copy_id) = e
        .execute(Command::DuplicateArrangementSection(section_id))
        .unwrap()
    else {
        panic!("duplicate section result")
    };
    let copy = e.project().arrangement_section(copy_id).unwrap();
    assert_eq!((copy.start(), copy.end()), (b(10), b(13)));
    assert_eq!(
        copy.name(),
        e.project().arrangement_section(section_id).unwrap().name()
    );
    // Valid copy draws may reuse a colour; the preview must predict the exact
    // candidate draw, and history/storage retain it rather than drawing again.
    assert_eq!(
        copy.color(),
        preview
            .sections()
            .iter()
            .find(|marker| marker.id().value() == 0)
            .unwrap()
            .color()
    );
    assert!(
        e.project()
            .tempo_map()
            .points()
            .iter()
            .any(|p| p.beat() == b(11))
    );
    assert!(
        e.project()
            .meter_map()
            .points()
            .iter()
            .any(|p| p.beat() == b(11))
    );
    assert_eq!(
        e.project().arrangement_sections().len(),
        preview.sections().len()
    );
    for (actual, expected) in e
        .project()
        .arrangement_sections()
        .iter()
        .zip(preview.sections())
    {
        assert_eq!(actual.name(), expected.name());
        assert_eq!(actual.color(), expected.color());
        assert_eq!(
            (actual.start(), actual.end()),
            (expected.start(), expected.end())
        );
        if expected.id().value() != 0 {
            assert_eq!(actual.id(), expected.id());
        } else {
            assert_eq!(actual.id(), copy_id);
        }
    }
    assert_eq!(e.project().tempo_map(), preview.tempo_map());
    let lane = e.project().track(audio_track).unwrap().gain_lane();
    assert!(
        lane.base()
            .iter()
            .any(|point| point.beat == b(11) && point.gain == 0.5)
    );
    assert!(
        lane.spans()
            .iter()
            .any(|span| span.start == b(10) && span.end == b(12))
    );
    let copied_midi = e.project().tracks()[1]
        .midi_clips()
        .iter()
        .find(|clip| clip.start() == b(10))
        .unwrap();
    assert_ne!(copied_midi.id(), old_midi.id());
    assert_ne!(copied_midi.source(), old_midi.source());
    let old_playback = old_midi.source_playback().unwrap();
    for (elapsed, copied_elapsed) in [(b(7), b(0)), (b(8), b(1)), (b(9), b(2))] {
        let old_phase = old_playback.source_phase_at_elapsed(elapsed).unwrap();
        assert_eq!(
            copied_midi.volume().gain_at(copied_elapsed),
            old_midi.volume().gain_at(old_phase)
        );
    }
    let new_events = events(e.project());
    // The note beginning at 6.5 crosses the section start at 7; its retained
    // gate fragment is present at the duplicate's leading edge.
    assert!(new_events.iter().any(|event| event.0 == b(10).raw()));
    for event in expected_duplicated {
        assert!(
            new_events.contains(&event),
            "missing copied event {event:?}"
        );
    }
    let after = e.project().snapshot();
    assert_eq!(
        Project::from_snapshot(after.clone()).unwrap().snapshot(),
        after
    );
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}

#[test]
fn prefix_growth_does_not_extend_an_adjacent_preceding_marker() {
    let mut e = Editor::new(Project::new(8000).unwrap(), 8).unwrap();
    let first = section(&mut e, "first", 0, 2);
    let second = section(&mut e, "second", 2, 4);
    e.execute(Command::ResizeArrangementSection {
        section: second,
        edge: hex_project::TrimEdge::Left,
        length: b(3),
    })
    .unwrap();
    assert_eq!(
        (
            e.project().arrangement_section(first).unwrap().start(),
            e.project().arrangement_section(first).unwrap().end()
        ),
        (b(0), b(2))
    );
    assert_eq!(
        (
            e.project().arrangement_section(second).unwrap().start(),
            e.project().arrangement_section(second).unwrap().end()
        ),
        (b(2), b(5))
    );
}

#[test]
fn nonadjacent_merge_keeps_earliest_identity_and_music_through_history() {
    let (mut e, first, last) = fixture();
    let middle = section(&mut e, "bridge", 4, 6);
    let before = e.project().snapshot();
    let music = e.project().tracks().to_vec();
    e.execute(Command::MergeArrangementSections(vec![last, first]))
        .unwrap();
    assert_eq!(e.project().arrangement_sections().len(), 1);
    let merged = e.project().arrangement_section(first).unwrap();
    assert_eq!(
        (merged.start(), merged.end(), merged.name(), merged.color()),
        (b(2), b(10), "intro", ElementColor::Cyan)
    );
    assert!(e.project().arrangement_section(middle).is_none());
    assert_eq!(e.project().tracks(), music);
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
    assert!(e.redo());
    assert_eq!(e.project().tracks(), music);
}
#[test]
fn deleting_a_section_preserves_gain_before_first_authored_point() {
    let (mut e, first, _) = fixture();
    let track = e.project().tracks()[0].id();
    e.execute(Command::ReplaceTrackGainEnvelope {
        track,
        points: vec![
            hex_project::GainPoint {
                beat: b(3),
                gain: 0.3,
                outgoing: hex_project::GainInterpolation::Linear,
                incoming_gain: None,
            },
            hex_project::GainPoint {
                beat: b(8),
                gain: 0.8,
                outgoing: hex_project::GainInterpolation::Step,
                incoming_gain: None,
            },
        ],
    })
    .unwrap();
    e.execute(Command::DeleteArrangementSection(first)).unwrap();
    let points = e.project().track(track).unwrap().gain_envelope();
    assert_eq!(points[0].beat, b(0));
    assert_eq!(points[0].gain, 0.3);
    assert_eq!(points[1].beat, b(2));
    assert_eq!(points[1].incoming_gain, Some(0.3));
}
#[test]
fn duplication_does_not_retrigger_original_gates_at_the_copy_capture_start() {
    let (mut e, _, last) = fixture();
    e.execute(Command::DuplicateArrangementSection(last))
        .unwrap();
    assert!(
        events(e.project()).contains(&(BeatTime::from_ratio(13, 2).unwrap().raw(), b(2).raw(), 60)),
        "original gate crossing beat7 must remain continuous until8.5"
    );
    assert!(
        events(e.project())
            .iter()
            .any(|(start, _, _)| *start == b(10).raw()),
        "copy retriggers carried gate at insertion"
    );
}

#[test]
fn section_splices_keep_exact_authored_musical_start_before_the_edit() {
    for kind in 0..3 {
        let (mut e, first, last) = fixture();
        let clip = e.project().tracks()[0].clips()[0].id();
        e.execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
        let authored = BeatTime::from_ratio(1, 7).unwrap();
        let frame = e.project().tempo_map().beat_to_frame(authored).unwrap();
        let mut snapshot = e.project().snapshot();
        let audio = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
        audio.edit.as_mut().unwrap().timing = hex_project::AudioClipTiming::Musical(authored);
        audio.start = frame;
        let mut e = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
        e.execute(match kind {
            0 => Command::ResizeArrangementSection {
                section: first,
                edge: hex_project::TrimEdge::Right,
                length: b(3),
            },
            1 => Command::DeleteArrangementSection(first),
            _ => Command::DuplicateArrangementSection(last),
        })
        .unwrap();
        assert_eq!(
            e.project().clip(clip).unwrap().audio_edit().unwrap().timing,
            hex_project::AudioClipTiming::Musical(authored)
        );
    }
}

#[test]
fn empty_prefix_holds_effective_touch_gain_without_restarting_unchanged_meter() {
    let (mut e, first, _) = fixture();
    let track = e.project().tracks()[0].id();
    e.execute(Command::SpliceGainTouch {
        track,
        expected_epoch: 0,
        span: hex_project::GainTouchSpan {
            source_start: b(1),
            start: b(1),
            end: b(5),
            context: hex_project::GainTouchContext {
                recording_epoch: 1,
                project_generation: 0,
                canonical_revision: 1,
                audio_revision: 1,
                transport_epoch: 1,
                gesture_epoch: 1,
                gesture_serial: 1,
                sample_rate: 8000,
                loop_pass: 0,
            },
            steps: vec![hex_project::GainTouchStep {
                beat: b(1),
                gain: 0.4,
                origin_beat: b(1),
                source_sequence: 1,
            }],
        },
    })
    .unwrap();
    e.execute(Command::ResizeArrangementSection {
        section: first,
        edge: hex_project::TrimEdge::Left,
        length: b(3),
    })
    .unwrap();
    let lane = e.project().track(track).unwrap().gain_lane();
    assert_eq!(
        lane.touch_gain_at(BeatTime::from_ratio(5, 2).unwrap()),
        None
    );
    let start = lane.base().iter().find(|p| p.beat == b(2)).unwrap();
    assert_eq!(start.gain, 0.4);
    assert_eq!(start.outgoing, hex_project::GainInterpolation::Step);
    assert_eq!(lane.touch_gain_at(b(3)), Some(0.4));
    assert_eq!(
        e.project().meter_map().position(b(3)).unwrap().bar_start(),
        b(0)
    );
}

#[test]
fn failed_section_splice_preparation_preserves_redo_ids_and_canonical_sources() {
    let (mut e, first, last) = fixture();
    e.execute(Command::RenameArrangementSection {
        section: first,
        name: "temporary".into(),
    })
    .unwrap();
    assert!(e.undo());
    let before = e.project().snapshot();
    for command in [
        Command::ResizeArrangementSection {
            section: first,
            edge: hex_project::TrimEdge::Left,
            length: b(3),
        },
        Command::ResizeArrangementSectionEdge {
            section: first,
            edge: hex_project::TrimEdge::Left,
            boundary: b(3),
        },
        Command::DeleteArrangementSection(first),
        Command::DuplicateArrangementSection(first),
        Command::MergeArrangementSections(vec![last, first]),
    ] {
        let result = e.execute_prepared::<(), hex_project::Invalid>(command, |_| {
            Err(hex_project::Invalid("injected preparation refusal"))
        });
        assert!(result.is_err());
        assert_eq!(e.project().snapshot(), before);
        assert!(e.can_redo());
    }
    for command in [
        Command::ResizeArrangementSection {
            section: first,
            edge: hex_project::TrimEdge::Left,
            length: b(0),
        },
        Command::ResizeArrangementSection {
            section: first,
            edge: hex_project::TrimEdge::Right,
            length: BeatTime::from_raw(u64::MAX),
        },
        Command::ResizeArrangementSectionEdge {
            section: first,
            edge: hex_project::TrimEdge::Left,
            boundary: b(4),
        },
        Command::ResizeArrangementSectionEdge {
            section: first,
            edge: hex_project::TrimEdge::Right,
            boundary: b(8),
        },
        Command::MergeArrangementSections(vec![first, first]),
        Command::MergeArrangementSections(vec![]),
    ] {
        assert!(e.execute(command).is_err());
        assert_eq!(e.project().snapshot(), before);
        assert!(e.can_redo());
    }
}

#[test]
fn shared_seam_preserves_outer_span_and_markers_or_clears_only_crossed_music_atomically() {
    for marker_only in [false, true] {
        for boundary in [b(3), b(5)] {
            let (mut editor, left, right) = fixture();
            editor
                .execute(Command::SetArrangementSectionRange {
                    section: right,
                    start: b(4),
                    end: b(10),
                })
                .unwrap();
            let other = section(&mut editor, "later", 12, 14);
            let before = editor.project().snapshot();
            let outside = editor.project().arrangement_section(other).unwrap().clone();
            let music = events(editor.project());
            let command = Command::ResizeArrangementSectionBoundary {
                left,
                right,
                boundary,
                marker_only,
            };
            assert_eq!(command.requires_section_publication(), !marker_only);
            let preview = editor
                .project()
                .preview_arrangement_section_edit(&command)
                .unwrap();
            editor.execute(command).unwrap();
            let p = editor.project();
            assert_eq!(p.arrangement_section(left).unwrap().start(), b(2));
            assert_eq!(p.arrangement_section(left).unwrap().end(), boundary);
            assert_eq!(p.arrangement_section(right).unwrap().start(), boundary);
            assert_eq!(p.arrangement_section(right).unwrap().end(), b(10));
            assert_eq!(p.arrangement_section(other), Some(&outside));
            assert_eq!(preview.sections(), p.arrangement_sections());
            assert_eq!(preview.tempo_map(), p.tempo_map());
            let after = p.snapshot();
            if marker_only {
                let mut without_markers = after.clone();
                without_markers.arrangement_sections = before.arrangement_sections.clone();
                assert_eq!(
                    without_markers, before,
                    "metadata resize preserves every other field"
                );
            } else {
                let lo = boundary.min(b(4)).raw();
                let hi = boundary.max(b(4)).raw();
                let mut expected = Vec::new();
                for (start, length, pitch) in music {
                    let end = start + length;
                    if start < lo {
                        expected.push((start, end.min(lo) - start, pitch));
                    }
                    if end > hi {
                        expected.push((start.max(hi), end - start.max(hi), pitch));
                    }
                }
                expected.sort();
                assert_eq!(
                    events(p),
                    expected,
                    "only music crossed by the moving seam is removed"
                );
            }
            assert!(editor.undo());
            let mut undone = editor.project().snapshot();
            undone.next_id = before.next_id;
            assert_eq!(undone, before);
            assert!(editor.redo());
            assert_eq!(editor.project().snapshot(), after);
        }
    }
}

#[test]
fn shared_seam_invalid_or_refused_preparation_preserves_redo_and_original_state() {
    let (mut editor, left, right) = fixture();
    // Gapped markers are not shared boundaries.
    assert!(
        editor
            .execute(Command::ResizeArrangementSectionBoundary {
                left,
                right,
                boundary: b(5),
                marker_only: true
            })
            .is_err()
    );
    editor
        .execute(Command::SetArrangementSectionRange {
            section: right,
            start: b(4),
            end: b(10),
        })
        .unwrap();
    editor
        .execute(Command::RenameArrangementSection {
            section: left,
            name: "draft".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().snapshot();
    for marker_only in [false, true] {
        for (a, c, boundary) in [
            (left, right, b(2)),
            (left, right, b(10)),
            (right, left, b(5)),
            (left, left, b(5)),
        ] {
            assert!(
                editor
                    .execute(Command::ResizeArrangementSectionBoundary {
                        left: a,
                        right: c,
                        boundary,
                        marker_only
                    })
                    .is_err()
            );
            assert_eq!(editor.project().snapshot(), before);
            assert!(editor.can_redo());
        }
        assert!(
            editor
                .execute_prepared::<(), hex_project::Invalid>(
                    Command::ResizeArrangementSectionBoundary {
                        left,
                        right,
                        boundary: b(5),
                        marker_only
                    },
                    |_| Err(hex_project::Invalid("refused shared seam preparation")),
                )
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
    }
}

#[test]
fn isolated_edges_preserve_other_sections_and_clear_only_the_crossed_interval() {
    for (edge, boundary) in [
        (hex_project::TrimEdge::Left, b(1)),
        (hex_project::TrimEdge::Left, b(3)),
        (hex_project::TrimEdge::Right, b(3)),
        (hex_project::TrimEdge::Right, b(5)),
    ] {
        let (mut editor, left, _) = fixture();
        let before = editor.project().snapshot();
        let original = editor.project().arrangement_section(left).unwrap().clone();
        let music = events(editor.project());
        let command = Command::ResizeArrangementSectionEdge {
            section: left,
            edge,
            boundary,
        };
        let preview = editor
            .project()
            .preview_arrangement_section_edit(&command)
            .unwrap();
        editor.execute(command).unwrap();
        let after = editor.project().snapshot();
        let marker = editor.project().arrangement_section(left).unwrap();
        let (expected_start, expected_end, old_edge) = match edge {
            hex_project::TrimEdge::Left => (boundary, original.end(), original.start()),
            hex_project::TrimEdge::Right => (original.start(), boundary, original.end()),
        };
        assert_eq!(
            (marker.start(), marker.end()),
            (expected_start, expected_end)
        );
        for section in editor
            .project()
            .arrangement_sections()
            .iter()
            .filter(|s| s.id() != left)
        {
            assert_eq!(
                before
                    .arrangement_sections
                    .iter()
                    .find(|s| s.id == section.id().value())
                    .unwrap(),
                &hex_project::ArrangementSectionSnapshot::from(section)
            );
        }
        assert_eq!(preview.sections(), editor.project().arrangement_sections());
        assert_eq!(preview.tempo_map(), editor.project().tempo_map());
        let lo = old_edge.min(boundary).raw();
        let hi = old_edge.max(boundary).raw();
        let mut expected = Vec::new();
        for (start, length, pitch) in music {
            let end = start + length;
            if start < lo {
                expected.push((start, end.min(lo) - start, pitch));
            }
            if end > hi {
                expected.push((start.max(hi), end - start.max(hi), pitch));
            }
        }
        expected.sort();
        assert_eq!(events(editor.project()), expected);
        assert!(editor.undo());
        let mut undone = editor.project().snapshot();
        undone.next_id = before.next_id;
        assert_eq!(undone, before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}

#[test]
fn marker_split_preserves_music_and_left_identity_with_one_history_step() {
    let (mut editor, id, other) = fixture();
    let before = editor.project().snapshot();
    let command = Command::SplitArrangementSection {
        section: id,
        at: b(3),
    };
    assert!(!command.requires_section_publication());
    let preview = editor
        .project()
        .preview_arrangement_section_edit(&command)
        .unwrap();
    let Created::ArrangementSection(right) = editor.execute(command).unwrap() else {
        panic!("split result")
    };
    assert_ne!(right, id);
    assert_ne!(right, other);
    let markers = editor.project().arrangement_sections();
    assert_eq!(
        (markers[0].id(), markers[0].start(), markers[0].end()),
        (id, b(2), b(3))
    );
    assert_eq!(
        (markers[1].id(), markers[1].start(), markers[1].end()),
        (right, b(3), b(4))
    );
    assert_eq!(
        (markers[0].name(), markers[0].color()),
        (markers[1].name(), markers[1].color())
    );
    for (actual, projected) in markers.iter().zip(preview.sections()) {
        assert_eq!(
            (actual.start(), actual.end(), actual.name(), actual.color()),
            (
                projected.start(),
                projected.end(),
                projected.name(),
                projected.color()
            )
        );
    }
    let after = editor.project().snapshot();
    let mut unchanged = after.clone();
    unchanged.arrangement_sections = before.arrangement_sections.clone();
    unchanged.next_id = before.next_id;
    assert_eq!(unchanged, before, "split changed musical/project state");
    assert!(editor.undo());
    let mut undone = editor.project().snapshot();
    undone.next_id = before.next_id;
    assert_eq!(undone, before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn marker_split_refusals_preserve_source_ids_and_redo() {
    let (mut editor, id, _) = fixture();
    editor
        .execute(Command::RenameArrangementSection {
            section: id,
            name: "renamed".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().snapshot();
    for at in [b(0), b(2), b(4), b(5), BeatTime::from_raw(u64::MAX)] {
        let command = Command::SplitArrangementSection { section: id, at };
        assert!(
            editor
                .project()
                .preview_arrangement_section_edit(&command)
                .is_err()
        );
        assert!(editor.execute(command).is_err());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.can_redo());
    }
    let command = Command::SplitArrangementSection {
        section: id,
        at: b(3),
    };
    let result: Result<(_, ()), hex_project::Invalid> = editor
        .execute_prepared(command.clone(), |_| {
            Err(hex_project::Invalid("injected split preparation refusal"))
        });
    assert!(result.is_err());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.can_redo());
    editor
        .execute(Command::RemoveArrangementSection(id))
        .unwrap();
    let removed = editor.project().snapshot();
    assert!(editor.execute(command).is_err());
    assert_eq!(editor.project().snapshot(), removed);

    let mut exhausted = before.clone();
    exhausted.next_id = u64::MAX;
    let mut exhausted = Editor::new(Project::from_snapshot(exhausted).unwrap(), 8).unwrap();
    let before = exhausted.project().snapshot();
    assert!(
        exhausted
            .execute(Command::SplitArrangementSection {
                section: id,
                at: b(3)
            })
            .is_err()
    );
    assert_eq!(exhausted.project().snapshot(), before);
    assert!(!exhausted.can_undo());
}

#[test]
fn marker_split_at_section_capacity_refuses_without_history_or_id_changes() {
    let (editor, id, _) = fixture();
    let mut snapshot = editor.project().snapshot();
    let source = snapshot
        .arrangement_sections
        .iter()
        .find(|s| s.id == id.value())
        .unwrap()
        .clone();
    snapshot.arrangement_sections = (0..hex_project::MAX_ARRANGEMENT_SECTIONS)
        .map(|index| {
            let mut marker = source.clone();
            if index > 0 {
                marker.id = 10_000 + index as u64;
            }
            marker.start = b(2 + index as u64 * 2);
            marker.end = b(4 + index as u64 * 2);
            marker
        })
        .collect();
    snapshot.next_id = 20_000;
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::SplitArrangementSection {
                section: id,
                at: b(3)
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
    assert!(!editor.can_undo());
}

fn meter_edit_command(operation: usize, a: SectionId, c: SectionId) -> Command {
    match operation {
        0 => Command::ReorderSectionBefore {
            section: c,
            before: a,
        },
        1 => Command::ReorderSectionAfter {
            section: a,
            after: c,
        },
        2 => Command::ResizeArrangementSection {
            section: a,
            edge: hex_project::TrimEdge::Right,
            length: b(3),
        },
        3 => Command::ResizeArrangementSection {
            section: a,
            edge: hex_project::TrimEdge::Left,
            length: b(1),
        },
        4 => Command::DeleteArrangementSection(a),
        5 => Command::DuplicateArrangementSection(a),
        6 => Command::ResizeArrangementSectionEdge {
            section: a,
            edge: hex_project::TrimEdge::Right,
            boundary: b(5),
        },
        7 => Command::ResizeArrangementSectionEdge {
            section: a,
            edge: hex_project::TrimEdge::Left,
            boundary: b(3),
        },
        _ => Command::ResizeArrangementSectionBoundary {
            left: a,
            right: c,
            boundary: b(5),
            marker_only: false,
        },
    }
}

#[test]
fn equal_meter_section_edits_do_not_create_boundaries_or_restart_bars() {
    for operation in 0..9 {
        let (mut e, a, c) = fixture();
        if operation == 8 {
            e.execute(Command::SetArrangementSectionRange {
                section: c,
                start: b(4),
                end: b(10),
            })
            .unwrap();
        }
        let before = e.project().snapshot();
        let before_meter = e.project().meter_map().clone();
        let command = meter_edit_command(operation, a, c);
        e.execute(command).unwrap();
        assert_eq!(
            e.project().meter_map(),
            &before_meter,
            "operation {operation} added synthetic meter boundaries"
        );
        for beat in [1, 2, 3, 4, 5, 7, 10, 12, 17] {
            assert_eq!(
                e.project().meter_map().position(b(beat)).unwrap(),
                before_meter.position(b(beat)).unwrap(),
                "operation {operation} reset bar numbering"
            );
        }
        let after = e.project().snapshot();
        assert!(e.undo());
        let mut restored = e.project().snapshot();
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        assert!(e.redo());
        assert_eq!(e.project().snapshot(), after);
    }
}

#[test]
fn reorder_preserves_real_meter_changes_and_explicit_origins_when_beat_zero_moves() {
    for mixed in [false, true] {
        for forward in [false, true] {
            let mut e = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
            let a = section(&mut e, "first", 0, 3);
            let c = section(&mut e, "second", 3, 8);
            if mixed {
                for (beat, meter) in [
                    (1, hex_project::Meter::new(3, 4).unwrap()),
                    (4, hex_project::Meter::new(7, 8).unwrap()),
                    (6, hex_project::Meter::COMMON_TIME),
                    (7, hex_project::Meter::COMMON_TIME),
                ] {
                    e.execute(Command::SetMeterPoint {
                        beat: b(beat),
                        meter,
                    })
                    .unwrap();
                }
            }
            let old = e.project().clone();
            let preview = if forward {
                old.preview_section_reorder_after(a, c)
            } else {
                old.preview_section_reorder(c, a)
            }
            .unwrap();
            e.execute(if forward {
                Command::ReorderSectionAfter {
                    section: a,
                    after: c,
                }
            } else {
                Command::ReorderSectionBefore {
                    section: c,
                    before: a,
                }
            })
            .unwrap();
            for eighth in 0..80 {
                let beat = BeatTime::from_ratio(eighth, 8).unwrap();
                assert_eq!(
                    e.project()
                        .meter_map()
                        .meter_at(preview.map_beat(beat).unwrap()),
                    old.meter_map().meter_at(beat),
                    "mapped beat {beat:?}, mixed={mixed}, forward={forward}"
                );
            }
            for point in old
                .meter_map()
                .points()
                .iter()
                .filter(|p| p.beat() != BeatTime::ZERO)
            {
                let mapped = preview.map_beat(point.beat()).unwrap();
                assert!(
                    e.project()
                        .meter_map()
                        .points()
                        .contains(&hex_project::MeterPoint::new(mapped, point.meter())),
                    "authored point {point:?} must survive"
                );
            }
            if !mixed {
                assert_eq!(e.project().meter_map(), old.meter_map());
            }
        }
    }
}

#[test]
fn repeated_equal_meter_reorders_do_not_accumulate_bar_resets() {
    let (mut e, a, c) = fixture();
    let original = e.project().meter_map().clone();
    for _ in 0..8 {
        e.execute(Command::ReorderSectionBefore {
            section: c,
            before: a,
        })
        .unwrap();
        e.execute(Command::ReorderSectionAfter {
            section: c,
            after: a,
        })
        .unwrap();
        assert_eq!(e.project().meter_map(), &original);
    }
}

#[test]
fn section_splices_preserve_effective_mixed_meter_on_all_retained_content() {
    for operation in 0..9 {
        let (mut e, a, c) = fixture();
        if operation == 8 {
            e.execute(Command::SetArrangementSectionRange {
                section: c,
                start: b(4),
                end: b(10),
            })
            .unwrap();
        }
        for (beat, meter) in [
            (1, hex_project::Meter::new(3, 4).unwrap()),
            (3, hex_project::Meter::new(7, 8).unwrap()),
            (6, hex_project::Meter::COMMON_TIME),
            (9, hex_project::Meter::COMMON_TIME),
        ] {
            e.execute(Command::SetMeterPoint {
                beat: b(beat),
                meter,
            })
            .unwrap();
        }
        let old = e.project().clone();
        let preview = match operation {
            0 => Some(old.preview_section_reorder(c, a).unwrap()),
            1 => Some(old.preview_section_reorder_after(a, c).unwrap()),
            _ => None,
        };
        e.execute(meter_edit_command(operation, a, c)).unwrap();
        for eighth in 0..112 {
            let beat = BeatTime::from_ratio(eighth, 8).unwrap();
            let in_range = |lo, hi| b(lo) <= beat && beat < b(hi);
            let mapped = match operation {
                0 | 1 => Some(preview.as_ref().unwrap().map_beat(beat).unwrap()),
                2 => Some(BeatTime::from_raw(
                    beat.raw() + if beat >= b(4) { b(1).raw() } else { 0 },
                )),
                3 => {
                    if in_range(2, 3) {
                        None
                    } else {
                        Some(BeatTime::from_raw(
                            beat.raw() - if beat >= b(3) { b(1).raw() } else { 0 },
                        ))
                    }
                }
                4 => {
                    if in_range(2, 4) {
                        None
                    } else {
                        Some(BeatTime::from_raw(
                            beat.raw() - if beat >= b(4) { b(2).raw() } else { 0 },
                        ))
                    }
                }
                5 => Some(BeatTime::from_raw(
                    beat.raw() + if beat >= b(4) { b(2).raw() } else { 0 },
                )),
                7 => {
                    if in_range(2, 3) {
                        None
                    } else {
                        Some(beat)
                    }
                }
                _ => {
                    if in_range(4, 5) {
                        None
                    } else {
                        Some(beat)
                    }
                }
            };
            if let Some(mapped) = mapped {
                assert_eq!(
                    e.project().meter_map().meter_at(mapped),
                    old.meter_map().meter_at(beat),
                    "operation {operation}, retained beat {beat:?}"
                );
            }
            if operation == 5 && in_range(2, 4) {
                assert_eq!(
                    e.project()
                        .meter_map()
                        .meter_at(BeatTime::from_raw(beat.raw() + b(2).raw())),
                    old.meter_map().meter_at(beat),
                    "duplicated meter at {beat:?}"
                );
            }
        }
    }
}

#[test]
fn moving_and_duplicating_sections_preserve_a_single_global_tempo_clock() {
    for rate in [8_000, 44_100, 48_000] {
        for micro_bpm in [120_000_000, 143_000_000, 180_000_000] {
            let mut editor = Editor::new(Project::new(rate).unwrap(), 32).unwrap();
            editor
                .execute(Command::SetProjectTempo {
                    tempo: Tempo::new(micro_bpm).unwrap(),
                })
                .unwrap();
            let first = section(&mut editor, "first", 0, 2);
            let last = section(&mut editor, "last", 4, 7);
            let clock = editor.project().tempo_map().clone();
            for command in [
                Command::ReorderSectionAfter {
                    section: first,
                    after: last,
                },
                Command::ReorderSectionBefore {
                    section: first,
                    before: last,
                },
                Command::DuplicateArrangementSection(first),
            ] {
                editor.execute(command).unwrap();
                assert_eq!(
                    editor.project().tempo_map(),
                    &clock,
                    "global tempo is not a splice event"
                );
                assert!(editor.undo());
                assert_eq!(editor.project().tempo_map(), &clock);
                assert!(editor.redo());
                assert_eq!(editor.project().tempo_map(), &clock);
            }
        }
    }
}

#[test]
fn repeated_duplicates_at_a_fractional_tempo_keep_musical_audio_editable() {
    // Reported on a saved 143 bpm project: after earlier section edits left a
    // musical clip one residual sample before a section's end fence,
    // duplicating that section refused with a rounding-residual error, and
    // every splice added same-tempo points.
    let mut e = Editor::new(Project::new(8_000).unwrap(), 64).unwrap();
    let Created::Imported { clip, .. } = e
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "loop.wav".into(),
            sample_rate: 8_000,
            frames: 9_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    e.execute(Command::SetTempoPoint {
        beat: b(0),
        tempo: Tempo::new(143_000_000).unwrap(),
    })
    .unwrap();
    e.execute(Command::SetAudioClipMusical {
        clip,
        enabled: true,
    })
    .unwrap();
    let mut snapshot = e.project().snapshot();
    let audio = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    audio.edit.as_mut().unwrap().timing = hex_project::AudioClipTiming::Musical(b(8));
    audio.start = e.project().tempo_map().beat_to_frame(b(8)).unwrap();
    let mut e = Editor::new(Project::from_snapshot(snapshot).unwrap(), 64).unwrap();
    let first = section(&mut e, "a", 0, 3);
    section(&mut e, "b", 3, 8);
    for round in 0..12 {
        let Created::ArrangementSection(copy) = e
            .execute(Command::DuplicateArrangementSection(first))
            .unwrap_or_else(|error| panic!("round {round} first: {error:?}"))
        else {
            panic!("copy identity")
        };
        // Explicit recoloring remains independent of the duplication color draw.
        e.execute(Command::SetArrangementSectionColor {
            section: copy,
            color: ElementColor::Cyan,
        })
        .unwrap();
        let ending_at_clip = e
            .project()
            .arrangement_sections()
            .iter()
            .rev()
            .find(|s| {
                e.project().clip(clip).unwrap().audio_edit().unwrap().timing
                    == hex_project::AudioClipTiming::Musical(s.end())
            })
            .map(|s| s.id());
        if let Some(section) = ending_at_clip {
            let Created::ArrangementSection(copy) = e
                .execute(Command::DuplicateArrangementSection(section))
                .unwrap_or_else(|error| panic!("round {round} fence: {error:?}"))
            else {
                panic!("copy identity")
            };
            e.execute(Command::SetArrangementSectionColor {
                section: copy,
                color: ElementColor::Cyan,
            })
            .unwrap();
        }
        assert_eq!(
            e.project().tempo_map().points().len(),
            1,
            "a constant tempo stays one point"
        );
    }
}
