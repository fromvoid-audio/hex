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
    assert!(
        seed.execute(Command::ReorderSectionBefore {
            section: c,
            before: a
        })
        .is_err()
    );
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
    assert!(
        e.execute(Command::ReorderSectionBefore {
            section: c,
            before: a
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    assert!(!e.can_undo());
}

#[test]
fn unsupported_outer_audio_placements_refuse_transactionally_instead_of_partial_song_edits() {
    for mode in [
        hex_project::AudioViewMode::Once,
        hex_project::AudioViewMode::Repeat,
    ] {
        let (seed, a, c) = fixture();
        let clip = seed.project().tracks()[0].clips()[0].clone();
        let mut edit = clip.audio_event_root();
        edit.placement = Some(hex_project::AudioViewPlacement {
            mode,
            outer_length: hex_project::AudioDomainClock::Frames(clip.duration()),
            cycle_phase: 173,
        });
        let mut snapshot = seed.project().snapshot();
        snapshot.tracks[0].audio_clips_mut().unwrap()[0].edit = Some(edit);
        let mut e = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
        let before = e.project().snapshot();
        let mut prepared = false;
        let error = e
            .execute_prepared::<(), hex_project::Invalid>(
                Command::ReorderSectionBefore {
                    section: c,
                    before: a,
                },
                |_| {
                    prepared = true;
                    Ok(())
                },
            )
            .unwrap_err();
        assert!(error.0.contains("outer audio once/repeat"));
        assert!(!prepared);
        assert_eq!(e.project().snapshot(), before);
        assert!(!e.can_undo());
        assert!(!e.can_redo());
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
    for seam in [b(2), b(5), b(10)] {
        assert_eq!(
            e.project().meter_map().position(seam).unwrap().bar_start(),
            seam
        );
    }
}
