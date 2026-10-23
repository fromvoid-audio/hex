use hex_project::{
    BeatTime, Command, Created, Editor, MidiOneShotBoundary as Boundary, Project,
    SessionCellContentSnapshot,
};

fn b(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn fixture(
    boundary: Boundary,
    looping: bool,
) -> (Project, hex_project::ClipId, hex_project::ClipId) {
    let mut e = Editor::new(Project::new(48000).unwrap(), 32).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "retained".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = e
        .execute(Command::PlaceMidiClip {
            track,
            start: b(1),
            length: b(12),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    for (start, duration, pitch) in [(4, 4, 60), (10, 2, 64)] {
        e.execute(Command::AddMidiNote {
            clip,
            start: b(start),
            duration: b(duration),
            pitch,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    }
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: b(12),
        source_start: b(2),
        loop_start: b(2),
        loop_end: b(6),
        looping,
        length: b(20),
    })
    .unwrap();
    let Created::Scene(scene) = e
        .execute(Command::AddScene {
            name: "retained".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(session) = e
        .execute(Command::CopyArrangementMidiToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("session")
    };
    let mut s = e.project().snapshot();
    let hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut s.tracks[0].content
    else {
        panic!("hybrid")
    };
    midi_clips[0].one_shot_boundary = boundary;
    let SessionCellContentSnapshot::Midi(cell) = &mut s.session.cells[0].content else {
        panic!("midi")
    };
    cell.source.one_shot_boundary = boundary;
    (Project::from_snapshot(s).unwrap(), clip, session)
}

#[test]
fn once_boundary_keeps_authored_notes_clips_gates_and_phase_at_exclusive_end() {
    for boundary in [Boundary::SourceEnd, Boundary::LoopEnd] {
        let (p, clip, session) = fixture(boundary, false);
        assert_eq!(
            p.midi_bounce_range(session).unwrap(),
            (
                0,
                24_000 * if boundary == Boundary::LoopEnd { 4 } else { 10 }
            )
        );
        for source in [
            p.midi_clip(clip).unwrap().source(),
            p.session_midi_clip(session).unwrap().source(),
        ] {
            assert_eq!(source.notes().len(), 2);
            assert_eq!(source.source_length(), b(12));
            assert_eq!(source.notes()[1].start(), b(10));
            let playback = source.playback_for_duration(b(20)).unwrap();
            let first = playback
                .note_occurrences(b(4), b(4))
                .unwrap()
                .iter()
                .next()
                .unwrap();
            assert_eq!(first.timeline_start(), b(2));
            assert_eq!(
                first.gate_duration(),
                b(if boundary == Boundary::LoopEnd { 2 } else { 4 })
            );
            assert_eq!(
                playback.note_occurrences(b(10), b(2)).unwrap().count(),
                u64::from(boundary == Boundary::SourceEnd)
            );
            let end = if boundary == Boundary::LoopEnd { 4 } else { 10 };
            assert!(playback.source_phase_at_elapsed(b(end - 1)).is_some());
            assert_eq!(playback.source_phase_at_elapsed(b(end)), None);
        }
    }
}

#[test]
fn repeating_sources_keep_their_existing_loop_law_for_both_policies() {
    let (a, clip, _) = fixture(Boundary::SourceEnd, true);
    let (z, _, _) = fixture(Boundary::LoopEnd, true);
    let a = a.midi_clip(clip).unwrap().source_playback().unwrap();
    let z = z.midi_clip(clip).unwrap().source_playback().unwrap();
    assert_eq!(a, z);
    assert_eq!(a.note_occurrences(b(4), b(4)).unwrap().count(), 5);
    assert_eq!(a.note_occurrences(b(10), b(2)).unwrap().count(), 0);
}

#[test]
fn once_boundary_source_edits_history_copies_and_validation_preserve_policy() {
    let (p, clip, session) = fixture(Boundary::LoopEnd, false);
    let mut e = Editor::new(p.clone(), 32).unwrap();
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: b(12),
        source_start: b(2),
        loop_start: b(2),
        loop_end: b(8),
        looping: false,
        length: b(20),
    })
    .unwrap();
    assert_eq!(
        e.project()
            .midi_clip(clip)
            .unwrap()
            .source()
            .effective_end(),
        b(8)
    );
    assert!(e.undo());
    assert_eq!(*e.project(), p);
    assert!(e.redo());
    assert_eq!(
        e.project()
            .midi_clip(clip)
            .unwrap()
            .source()
            .one_shot_boundary(),
        Boundary::LoopEnd
    );
    let before = e.project().clone();
    assert!(
        e.execute(Command::SetSessionMidiClipSource {
            clip: session,
            source_length: b(12),
            source_start: b(6),
            loop_start: b(2),
            loop_end: b(6),
            looping: false
        })
        .is_err()
    );
    assert_eq!(*e.project(), before);
    let Created::Scene(scene) = e
        .execute(Command::AddScene {
            name: "copy".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(copy) = e
        .execute(Command::CopyArrangementMidiToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("copy")
    };
    assert_eq!(
        e.project()
            .session_midi_clip(copy)
            .unwrap()
            .source()
            .one_shot_boundary(),
        Boundary::LoopEnd
    );
    let trim = e
        .project()
        .midi_clip(clip)
        .unwrap()
        .preview_edge_trim(hex_project::TrimEdge::Left, b(20))
        .unwrap();
    assert!(trim.source_start() < b(8));
    let Created::Clip(duplicate) = e.execute(Command::DuplicateMidiClip(clip)).unwrap() else {
        panic!("duplicate")
    };
    assert_eq!(
        e.project()
            .midi_clip(duplicate)
            .unwrap()
            .source()
            .one_shot_boundary(),
        Boundary::LoopEnd
    );
    let mut bad = before.snapshot();
    let hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut bad.tracks[0].content
    else {
        panic!("hybrid")
    };
    midi_clips[0].source_start_raw = b(8).raw();
    assert!(Project::from_snapshot(bad).is_err());
}

#[test]
fn once_boundary_materialized_volume_holds_the_selected_endpoint() {
    use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    let (project, clip, _) = fixture(Boundary::LoopEnd, false);
    let playback = project.midi_clip(clip).unwrap().source_playback().unwrap();
    let volume = MidiClipVolume::new(
        vec![
            MidiClipVolumePoint::linear(b(0), MidiClipGain::new(0.25).unwrap()),
            MidiClipVolumePoint::linear(b(6), MidiClipGain::new(0.75).unwrap()),
            MidiClipVolumePoint::linear(b(12), MidiClipGain::new(4.0).unwrap()),
        ],
        b(12),
    )
    .unwrap();
    let materialized = volume.materialized(playback, b(0), b(20)).unwrap();
    assert!((materialized.gain_at(b(3)).linear() - volume.gain_at(b(5)).linear()).abs() < 1e-12);
    assert_eq!(materialized.gain_at(b(4)), volume.gain_at(b(6)));
    assert_eq!(materialized.gain_at(b(10)), volume.gain_at(b(6)));
}

#[test]
fn section_edge_and_shared_seam_edits_keep_loop_end_silence_and_history() {
    for shared in [false, true] {
        let (project, clip, _) = fixture(Boundary::LoopEnd, false);
        let mut e = Editor::new(project, 32).unwrap();
        let mut sections = Vec::new();
        for (start, end) in [(0, 4), (4, 8)] {
            let Created::ArrangementSection(section) = e
                .execute(Command::AddArrangementSection {
                    name: "edge".into(),
                    color: hex_project::ElementColor::Cyan,
                    start: b(start),
                    end: b(end),
                })
                .unwrap()
            else {
                panic!("section")
            };
            sections.push(section);
        }
        let before = e.project().clone();
        e.execute(if shared {
            Command::ResizeArrangementSectionBoundary {
                left: sections[0],
                right: sections[1],
                boundary: b(3),
                marker_only: false,
            }
        } else {
            Command::ResizeArrangementSectionEdge {
                section: sections[0],
                edge: hex_project::TrimEdge::Right,
                boundary: b(2),
            }
        })
        .unwrap();
        // The original prefix retains the complete authored source, including
        // its inaudible late note; finite suffixes carry only audible gates.
        assert_eq!(
            e.project().midi_clip(clip).unwrap().source(),
            before.midi_clip(clip).unwrap().source()
        );
        let mut events = Vec::new();
        for clip in e.project().tracks().iter().flat_map(|t| t.midi_clips()) {
            for note in clip.notes() {
                for occurrence in clip
                    .source_playback()
                    .unwrap()
                    .note_occurrences(note.start(), note.duration())
                    .unwrap()
                    .iter()
                {
                    events.push((
                        clip.start().raw() + occurrence.timeline_start().raw(),
                        occurrence.gate_duration().raw(),
                        note.pitch(),
                    ));
                }
            }
        }
        assert_eq!(events, vec![(b(4).raw(), b(1).raw(), 60)]);
        let after = e.project().clone();
        assert!(e.undo());
        let mut restored = e.project().snapshot();
        let before = before.snapshot();
        assert!(restored.next_id >= before.next_id);
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        assert!(e.redo());
        assert_eq!(e.project(), &after);
    }
}

#[test]
fn once_boundary_projection_refuses_invalid_selected_ends() {
    use hex_project::MidiSourcePlayback;
    for end in [b(0), b(2), b(9)] {
        assert!(MidiSourcePlayback::new_with_initial_end(b(8), b(2), None, b(20), end).is_err());
    }
    assert!(
        MidiSourcePlayback::new_with_initial_end(b(8), b(2), Some((b(2), b(6))), b(20), b(7))
            .is_err()
    );
}
