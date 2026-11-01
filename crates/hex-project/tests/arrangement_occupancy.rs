//! Canonical placement, current-format refusal and preparation/history boundaries.
use hex_project::{
    AssetId, BeatTime, ClipId, Command, Created, Editor, Invalid, Project, Tempo,
    TrackContentSnapshot, TrackId,
};

struct Fixture {
    editor: Editor,
    track: TrackId,
    asset: AssetId,
    original: ClipId,
}
fn fixture() -> Fixture {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported {
        track,
        asset,
        clip: original,
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "original".into(),
            path: "immutable.wav".into(),
            sample_rate: 48_000,
            frames: 1_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    Fixture {
        editor,
        track,
        asset,
        original,
    }
}
fn audio(track: TrackId, asset: AssetId, start: u64, duration: u64) -> Command {
    Command::PlaceClip {
        track,
        asset,
        start,
        duration,
        source_offset: 0,
        source_length: duration,
        looping: false,
    }
}
fn time(frame: u64) -> BeatTime {
    BeatTime::from_ratio(frame, 24_000).unwrap()
}
fn midi(track: TrackId, start: u64, duration: u64) -> Command {
    Command::PlaceMidiClip {
        track,
        start: time(start),
        length: time(duration),
    }
}
fn second_track(editor: &mut Editor) -> TrackId {
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "other".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    track
}
fn assert_restored_with_monotonic_ids(editor: &Editor, before: &Project, after: &Project) {
    let restored = editor.project().snapshot();
    let mut expected = before.snapshot();
    // Existing history never reuses identities that were visible before Undo.
    assert_eq!(restored.next_id, after.snapshot().next_id);
    expected.next_id = restored.next_id;
    assert_eq!(restored, expected);
}
fn assert_carved(f: &Fixture, incoming: ClipId, is_midi: bool) {
    let project = f.editor.project();
    let row = project.track(f.track).unwrap();
    assert_eq!(row.clips().len(), if is_midi { 2 } else { 3 });
    assert_eq!(row.midi_clips().len(), usize::from(is_midi));
    let mut retained: Vec<_> = row.clips().iter().filter(|c| c.id() != incoming).collect();
    retained.sort_by_key(|c| c.start());
    assert_eq!(retained.len(), 2);
    assert_eq!(
        (
            retained[0].id(),
            retained[0].start(),
            retained[0].duration(),
            retained[0].source_offset(),
            retained[0].source_length()
        ),
        (f.original, 0, 400, 0, 400)
    );
    assert_eq!(
        (
            retained[1].start(),
            retained[1].duration(),
            retained[1].source_offset(),
            retained[1].source_length()
        ),
        (600, 400, 600, 400)
    );
    assert_ne!(retained[1].id(), f.original);
    assert_eq!(retained[0].asset(), f.asset);
    assert_eq!(retained[1].asset(), f.asset);
    assert_eq!(
        Project::from_snapshot(project.snapshot()).unwrap(),
        *project
    );
}

#[test]
fn raw_audio_and_midi_placement_carve_old_content_and_have_one_history_edge() {
    for is_midi in [false, true] {
        let mut f = fixture();
        let before = f.editor.project().clone();
        let command = if is_midi {
            midi(f.track, 400, 200)
        } else {
            audio(f.track, f.asset, 400, 200)
        };
        let Created::Clip(incoming) = f.editor.execute(command).unwrap() else {
            panic!("clip")
        };
        assert_carved(&f, incoming, is_midi);
        let after = f.editor.project().clone();
        assert!(f.editor.undo());
        assert_restored_with_monotonic_ids(&f.editor, &before, &after);
        assert!(f.editor.redo());
        assert_eq!(f.editor.project(), &after);
    }
}

#[test]
fn raw_audio_and_midi_move_carve_destination_and_keep_the_original_identity() {
    for is_midi in [false, true] {
        let mut f = fixture();
        let other = second_track(&mut f.editor);
        let command = if is_midi {
            midi(other, 1_200, 200)
        } else {
            audio(other, f.asset, 1_200, 200)
        };
        let Created::Clip(incoming) = f.editor.execute(command).unwrap() else {
            panic!("clip")
        };
        let before = f.editor.project().clone();
        let command = if is_midi {
            Command::MoveMidiClip {
                clip: incoming,
                track: f.track,
                start: time(400),
            }
        } else {
            Command::MoveClip {
                clip: incoming,
                track: f.track,
                start: 400,
            }
        };
        f.editor.execute(command).unwrap();
        assert_carved(&f, incoming, is_midi);
        let source = f.editor.project().track(other).unwrap();
        assert!(source.clips().is_empty() && source.midi_clips().is_empty());
        let after = f.editor.project().clone();
        assert!(f.editor.undo());
        assert_restored_with_monotonic_ids(&f.editor, &before, &after);
        assert!(f.editor.redo());
        assert_eq!(f.editor.project(), &after);
    }
}

#[test]
fn current_format_audio_and_mixed_overlap_refuses_instead_of_normalizing() {
    for is_midi in [false, true] {
        let mut f = fixture();
        let other = second_track(&mut f.editor);
        let command = if is_midi {
            midi(other, 400, 200)
        } else {
            audio(other, f.asset, 400, 200)
        };
        f.editor.execute(command).unwrap();
        let mut snapshot = f.editor.project().snapshot();
        let (left, right) = snapshot.tracks.split_at_mut(1);
        let TrackContentSnapshot::Hybrid {
            audio_clips: destination_audio,
            midi_clips: destination_midi,
        } = &mut left[0].content
        else {
            panic!("hybrid")
        };
        let TrackContentSnapshot::Hybrid {
            audio_clips: source_audio,
            midi_clips: source_midi,
        } = &mut right[0].content
        else {
            panic!("hybrid")
        };
        destination_audio.append(source_audio);
        destination_midi.append(source_midi);
        let result = Project::from_snapshot(snapshot);
        assert!(
            result
                .as_ref()
                .err()
                .is_some_and(|e| e.0.contains("overlap")),
            "invalid top-level occupancy was accepted: {result:?}"
        );
    }
}

#[test]
fn colliding_tempo_point_crops_the_audio_tail_like_a_global_tempo_edit() {
    let mut f = fixture();
    f.editor.execute(midi(f.track, 2_000, 400)).unwrap();
    let before = f.editor.project().clone();
    let mut prepared = false;
    // Four times faster: the MIDI clip's beat now lands at frame 500, inside
    // the audio clip's old 1,000-frame span. Onsets stay on their beats and
    // only the colliding audio tail is cropped, as for `SetProjectTempo`.
    f.editor
        .execute_prepared::<_, Invalid>(
            Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(480_000_000).unwrap(),
            },
            |_| {
                prepared = true;
                Ok(())
            },
        )
        .unwrap();
    assert!(prepared);
    let changed = f.editor.project();
    let audio = changed.clip(f.original).unwrap();
    assert_eq!((audio.start(), audio.duration()), (0, 500));
    assert_eq!(audio.asset(), f.asset);
    assert_eq!(
        changed
            .tempo_map()
            .beat_to_frame(changed.tracks()[0].midi_clips()[0].start())
            .unwrap(),
        500
    );
    assert!(f.editor.undo());
    assert_eq!(f.editor.project(), &before);
}

#[test]
fn audio_trim_fade_crop_and_repeat_resize_carve_only_the_newly_occupied_region() {
    use hex_project::{AudioViewResizeOperation, TrimEdge};
    for mode in 0..4 {
        let mut f = fixture();
        f.editor
            .execute(if mode == 2 {
                Command::TrimClipFadeEdge {
                    clip: f.original,
                    edge: TrimEdge::Right,
                    frame: 400,
                }
            } else {
                Command::TrimClipEdge {
                    clip: f.original,
                    edge: TrimEdge::Right,
                    frame: 400,
                }
            })
            .unwrap();
        let Created::Clip(peer) = f.editor.execute(audio(f.track, f.asset, 600, 400)).unwrap()
        else {
            panic!("peer")
        };
        let before = f.editor.project().clone();
        let command = match mode {
            0 => Command::TrimClip {
                clip: f.original,
                start: 0,
                duration: 800,
                source_offset: 0,
                source_length: 800,
            },
            1 => Command::TrimClipEdge {
                clip: f.original,
                edge: TrimEdge::Right,
                frame: 800,
            },
            2 => Command::TrimClipFadeEdge {
                clip: f.original,
                edge: TrimEdge::Right,
                frame: 800,
            },
            _ => Command::ResizeAudioClipView {
                clip: f.original,
                operation: AudioViewResizeOperation::RepeatOuterEnd { project_frame: 800 },
            },
        };
        f.editor.execute(command).unwrap();
        let after = f.editor.project().clone();
        assert_eq!(after.clip(f.original).unwrap().duration(), 800);
        let retained = after.clip(peer).unwrap();
        assert_eq!(
            (
                retained.start(),
                retained.duration(),
                retained.source_offset(),
                retained.source_length()
            ),
            (800, 200, 200, 200)
        );
        assert_eq!(after.track(f.track).unwrap().clips().len(), 2);
        assert_eq!(
            after.assets().collect::<Vec<_>>(),
            before.assets().collect::<Vec<_>>()
        );
        assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
        assert!(f.editor.undo());
        assert_restored_with_monotonic_ids(&f.editor, &before, &after);
        assert!(f.editor.redo());
        assert_eq!(f.editor.project(), &after);
    }
}

#[test]
fn midi_extent_changes_and_both_edge_extensions_overwrite_audio_peers() {
    use hex_project::TrimEdge;
    for mode in 0..3 {
        let mut f = fixture();
        f.editor.execute(Command::RemoveClip(f.original)).unwrap();
        let (start, length, peer_start, peer_length) = if mode == 2 {
            (400, 400, 0, 200)
        } else {
            (0, 400, 600, 400)
        };
        let Created::Clip(original) = f.editor.execute(midi(f.track, start, length)).unwrap()
        else {
            panic!("midi")
        };
        let Created::Clip(peer) = f
            .editor
            .execute(audio(f.track, f.asset, peer_start, peer_length))
            .unwrap()
        else {
            panic!("peer")
        };
        let before = f.editor.project().clone();
        let command = match mode {
            0 => Command::TrimMidiClipEdge {
                clip: original,
                edge: TrimEdge::Right,
                beat: time(800),
            },
            1 => Command::SetMidiClipSource {
                clip: original,
                source_length: time(400),
                source_start: BeatTime::ZERO,
                loop_start: BeatTime::ZERO,
                loop_end: time(400),
                looping: false,
                length: time(800),
            },
            _ => Command::TrimMidiClipEdge {
                clip: original,
                edge: TrimEdge::Left,
                beat: time(100),
            },
        };
        f.editor.execute(command).unwrap();
        let after = f.editor.project().clone();
        let retained = after.clip(peer).unwrap();
        assert_eq!(
            (
                retained.start(),
                retained.duration(),
                retained.source_offset()
            ),
            if mode == 2 {
                (0, 100, 0)
            } else {
                (800, 200, 200)
            }
        );
        let placed = after.midi_clip(original).unwrap();
        let span = (
            after.tempo_map().beat_to_frame(placed.start()).unwrap(),
            after
                .tempo_map()
                .beat_to_frame(BeatTime::from_raw(
                    placed.start().raw() + placed.length().raw(),
                ))
                .unwrap(),
        );
        assert_eq!(span, if mode == 2 { (100, 800) } else { (0, 800) });
        assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
        assert!(f.editor.undo());
        assert_restored_with_monotonic_ids(&f.editor, &before, &after);
        assert!(f.editor.redo());
        assert_eq!(f.editor.project(), &after);
    }
}

#[test]
fn audio_and_midi_duplicate_overwrite_the_next_clip_as_one_copy() {
    for is_midi in [false, true] {
        let mut f = fixture();
        let original = if is_midi {
            f.editor.execute(Command::RemoveClip(f.original)).unwrap();
            let Created::Clip(id) = f.editor.execute(midi(f.track, 0, 1_000)).unwrap() else {
                panic!("midi")
            };
            id
        } else {
            f.original
        };
        let Created::Clip(peer) = f
            .editor
            .execute(audio(f.track, f.asset, 1_000, 1_000))
            .unwrap()
        else {
            panic!("peer")
        };
        let before = f.editor.project().clone();
        let Created::Clip(copied) = f
            .editor
            .execute(if is_midi {
                Command::DuplicateMidiClip(original)
            } else {
                Command::DuplicateClip(original)
            })
            .unwrap()
        else {
            panic!("duplicate")
        };
        let after = f.editor.project().clone();
        assert_ne!(copied, original);
        assert!(after.clip(peer).is_none());
        assert_eq!(
            after.track(f.track).unwrap().clips().len()
                + after.track(f.track).unwrap().midi_clips().len(),
            2
        );
        if is_midi {
            assert_eq!(after.midi_clip(copied).unwrap().start(), time(1_000));
            assert_eq!(after.midi_source(original), before.midi_source(original));
        } else {
            assert_eq!(after.clip(copied).unwrap().start(), 1_000);
            assert_eq!(after.clip(original), before.clip(original));
        }
        assert!(f.editor.undo());
        assert_restored_with_monotonic_ids(&f.editor, &before, &after);
        assert!(f.editor.redo());
        assert_eq!(f.editor.project(), &after);
    }
}

#[test]
fn preparation_refusal_after_real_carving_keeps_ids_project_and_redo_exact() {
    let mut f = fixture();
    f.editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    assert!(f.editor.undo());
    let before = f.editor.project().clone();
    let mut called = false;
    let result =
        f.editor
            .execute_prepared::<(), Invalid>(audio(f.track, f.asset, 400, 200), |candidate| {
                called = true;
                assert_eq!(candidate.track(f.track).unwrap().clips().len(), 3);
                Err(Invalid("actual preparation refused"))
            });
    assert!(called);
    assert_eq!(result, Err(Invalid("actual preparation refused")));
    assert_eq!(f.editor.project(), &before);
    assert!(f.editor.can_redo());
    assert!(f.editor.redo());
    assert_eq!(f.editor.project().master_gain(), 0.5);
}

fn import_at(track: TrackId, start: u64, frames: u64, name: &str) -> Command {
    Command::ImportPreparedAudioClip {
        track,
        start,
        path: name.into(),
        source_sample_rate: 48_000,
        source_frames: frames,
        playback_frames: frames,
        conversion_version: hex_project::IDENTITY_CONVERSION,
    }
}

#[test]
fn audio_import_batch_admits_final_capacity_and_keeps_one_atomic_history_edge() {
    let mut f = fixture();
    f.editor
        .execute(audio(f.track, f.asset, 1_200, 400))
        .unwrap();
    let filler_track = second_track(&mut f.editor);
    f.editor
        .execute(audio(filler_track, f.asset, 0, 1))
        .unwrap();
    let mut snapshot = f.editor.project().snapshot();
    let mut next = snapshot.next_id;
    let row = snapshot
        .tracks
        .iter_mut()
        .find(|row| row.id == filler_track.value())
        .unwrap();
    let TrackContentSnapshot::Hybrid { audio_clips, .. } = &mut row.content else {
        panic!("hybrid")
    };
    let filler = audio_clips[0].clone();
    // Two destination clips plus these fillers leave one free slot. The first
    // import alone would exceed capacity (left + right + new), but the complete
    // adjacent batch removes that right fragment and the second old victim.
    for start in 1..hex_project::MAX_CLIPS - 3 {
        let mut clip = filler.clone();
        clip.id = next;
        next += 1;
        clip.start = start as u64;
        audio_clips.push(clip);
    }
    snapshot.next_id = next;
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    let before = editor.project().clone();
    // The same final-row admission is required for a multi-row rectangle:
    // splitting its first row at full capacity is paid for by later removals.
    let mut rectangle = Editor::new(before.clone(), 8).unwrap();
    rectangle
        .execute(audio(filler_track, f.asset, 1_000_000, 1))
        .unwrap();
    let full_before = rectangle.project().clone();
    assert_eq!(
        full_before
            .tracks()
            .iter()
            .map(|row| row.clips().len())
            .sum::<usize>(),
        hex_project::MAX_CLIPS
    );
    rectangle
        .execute(Command::DeleteTimeRange {
            range: hex_project::TimeRange::new(vec![f.track, filler_track], 400, 500).unwrap(),
        })
        .unwrap();
    let cleared = rectangle.project().clone();
    assert_eq!(
        cleared
            .tracks()
            .iter()
            .map(|row| row.clips().len())
            .sum::<usize>(),
        hex_project::MAX_CLIPS - 99
    );
    let right = cleared
        .track(f.track)
        .unwrap()
        .clips()
        .iter()
        .find(|clip| clip.start() == 500)
        .unwrap();
    assert_eq!((right.duration(), right.source_offset()), (500, 500));
    assert!(rectangle.undo());
    assert_restored_with_monotonic_ids(&rectangle, &full_before, &cleared);
    assert!(rectangle.redo());
    assert_eq!(rectangle.project(), &cleared);
    let commands = || {
        vec![
            import_at(f.track, 400, 100, "first.wav"),
            import_at(f.track, 500, 1_500, "second.wav"),
        ]
    };
    let refused = editor.execute_audio_imports_prepared(commands(), |candidate, results| {
        assert_eq!(results.len(), 2);
        assert_eq!(candidate.track(f.track).unwrap().clips().len(), 3);
        Err::<(), _>(Invalid("preparation deliberately refused"))
    });
    assert!(refused.is_err());
    assert_eq!(editor.project(), &before);
    let (results, ()) = editor
        .execute_audio_imports_prepared(commands(), |candidate, _| {
            Project::from_snapshot(candidate.snapshot())?;
            Ok::<_, Invalid>(())
        })
        .unwrap();
    let after = editor.project().clone();
    assert_eq!(
        after
            .tracks()
            .iter()
            .map(|row| row.clips().len() + row.midi_clips().len())
            .sum::<usize>(),
        hex_project::MAX_CLIPS
    );
    let retained = after.clip(f.original).unwrap();
    assert_eq!(
        (
            retained.start(),
            retained.duration(),
            retained.source_offset()
        ),
        (0, 400, 0)
    );
    for (index, result) in results.iter().enumerate() {
        let Created::Imported { track, asset, clip } = result else {
            panic!("import result")
        };
        assert_eq!(*track, f.track);
        let clip = after.clip(*clip).unwrap();
        assert_eq!(clip.asset(), *asset);
        assert_eq!(
            (clip.start(), clip.duration()),
            if index == 0 { (400, 100) } else { (500, 1_500) }
        );
        assert_eq!(
            after.asset(*asset).unwrap().path().to_str().unwrap(),
            if index == 0 {
                "first.wav"
            } else {
                "second.wav"
            }
        );
    }
    assert!(editor.undo());
    assert_restored_with_monotonic_ids(&editor, &before, &after);
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
}

#[test]
fn replacement_spanning_disjoint_hybrid_victims_preserves_both_outer_sources_and_other_tracks() {
    let mut f = fixture();
    f.editor
        .execute(Command::TrimClip {
            clip: f.original,
            start: 0,
            duration: 300,
            source_offset: 0,
            source_length: 300,
        })
        .unwrap();
    let Created::Clip(middle) = f.editor.execute(midi(f.track, 400, 100)).unwrap() else {
        panic!("midi")
    };
    let Created::Clip(right) = f.editor.execute(audio(f.track, f.asset, 600, 300)).unwrap() else {
        panic!("audio")
    };
    f.editor
        .execute(Command::SetClipFades {
            clip: right,
            fade_in: 100,
            fade_out: 100,
        })
        .unwrap();
    let other = second_track(&mut f.editor);
    f.editor.execute(audio(other, f.asset, 200, 500)).unwrap();
    let before = f.editor.project().clone();
    let Created::Clip(new) = f.editor.execute(audio(f.track, f.asset, 200, 500)).unwrap() else {
        panic!("incoming")
    };
    let after = f.editor.project().clone();
    assert_eq!(after.track(other), before.track(other));
    assert!(after.midi_clip(middle).is_none());
    assert_eq!(after.track(f.track).unwrap().clips().len(), 3);
    let left = after.clip(f.original).unwrap();
    assert_eq!(
        (left.start(), left.duration(), left.source_offset()),
        (0, 200, 0)
    );
    let retained = after.clip(right).unwrap();
    assert_eq!(
        (
            retained.start(),
            retained.duration(),
            retained.source_offset(),
            retained.source_length()
        ),
        (700, 200, 100, 200)
    );
    let original = before.clip(right).unwrap();
    for offset in 0..retained.duration() {
        assert_eq!(
            retained.fade().gain_at(offset),
            original.fade().gain_at(100 + offset)
        );
    }
    assert_eq!(
        (
            after.clip(new).unwrap().start(),
            after.clip(new).unwrap().duration()
        ),
        (200, 500)
    );
    assert_eq!(Project::from_snapshot(after.snapshot()).unwrap(), after);
    assert!(f.editor.undo());
    assert_restored_with_monotonic_ids(&f.editor, &before, &after);
    assert!(f.editor.redo());
    assert_eq!(f.editor.project(), &after);
}
