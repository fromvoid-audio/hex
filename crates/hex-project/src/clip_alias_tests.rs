use crate::{BeatTime, ClipId, Command, Created, Editor, Project, TrackId};
use std::sync::Arc;
fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn midi() -> (Editor, TrackId, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(0),
            length: beat(4),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: beat(1),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    (editor, track, clip)
}
fn duplicate(editor: &mut Editor, clip: ClipId) -> ClipId {
    let Created::Clips { first, last } = editor
        .execute(Command::DuplicateAliasClips(vec![clip]))
        .unwrap()
    else {
        panic!("alias")
    };
    assert_eq!(first, last);
    first
}
#[test]
fn midi_alias_content_local_settings_unique_and_original_deletion_round_trip() {
    let (mut editor, _, original) = midi();
    let alias = duplicate(&mut editor, original);
    assert_eq!(
        editor.project().clip_alias(original),
        editor.project().clip_alias(alias)
    );
    assert!(Arc::ptr_eq(
        &editor.project().midi_source(original).unwrap().content,
        &editor.project().midi_source(alias).unwrap().content
    ));
    editor
        .execute(Command::AddMidiNote {
            clip: alias,
            start: beat(2),
            duration: beat(1),
            pitch: 64,
            channel: 0,
            velocity: 110,
        })
        .unwrap();
    assert_eq!(
        editor.project().midi_clip(original).unwrap().notes(),
        editor.project().midi_clip(alias).unwrap().notes()
    );
    assert!(editor.undo());
    assert_eq!(
        editor.project().midi_clip(original).unwrap().notes().len(),
        1
    );
    assert!(editor.redo());
    editor
        .execute(Command::SetMidiClipSource {
            clip: alias,
            source_length: beat(4),
            source_start: beat(1),
            loop_start: beat(1),
            loop_end: beat(3),
            looping: true,
            length: beat(3),
        })
        .unwrap();
    assert!(!editor.project().midi_clip(original).unwrap().looping());
    let start = editor.project().midi_clip(original).unwrap().start();
    editor
        .execute(Command::ApplyClipSettingsToAliases(alias))
        .unwrap();
    assert_eq!(editor.project().midi_clip(original).unwrap().start(), start);
    assert!(editor.project().midi_clip(original).unwrap().looping());
    let snapshot = editor.project().snapshot();
    assert_eq!(snapshot.aliases.sources.len(), 1);
    assert_eq!(Project::from_snapshot(snapshot).unwrap(), *editor.project());
    editor.execute(Command::RemoveMidiClip(original)).unwrap();
    assert_eq!(editor.project().midi_clip(alias).unwrap().notes().len(), 2);
    editor
        .execute(Command::MakeClipsUnique(vec![alias]))
        .unwrap();
    assert_eq!(editor.project().clip_alias(alias), None);
    assert!(editor.project().snapshot().aliases.sources.is_empty());
    assert!(editor.undo());
    assert!(editor.project().clip_alias(alias).is_some());
}
#[test]
fn console_alias_uses_empty_cell_and_shared_notes_with_atomic_history() {
    let (mut editor, track, _) = midi();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "first".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(original) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "cell".into(),
            source_length: beat(4),
        })
        .unwrap()
    else {
        panic!("cell")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: original,
            start: beat(1),
            duration: beat(1),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let before = editor.project().clone();
    let Created::SessionClip(alias) = editor
        .execute(Command::DuplicateSessionCell {
            track,
            scene,
            alias: true,
        })
        .unwrap()
    else {
        panic!("alias cell")
    };
    assert_eq!(editor.project().scenes().len(), before.scenes().len() + 1);
    editor
        .execute(Command::AddMidiNote {
            clip: alias,
            start: beat(2),
            duration: beat(1),
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    assert_eq!(
        editor
            .project()
            .midi_source(original)
            .unwrap()
            .notes()
            .len(),
        2
    );
    assert!(editor.undo());
    assert!(editor.undo());
    let mut restored = editor.project().snapshot();
    restored.next_id = before.snapshot().next_id;
    assert_eq!(Project::from_snapshot(restored).unwrap(), before);
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
}
#[test]
fn ordered_batch_edits_through_siblings_share_one_result() {
    let (mut editor, _, original) = midi();
    let alias = duplicate(&mut editor, original);
    let (_, ()) = editor
        .execute_many_prepared::<_, crate::Invalid>(
            vec![
                Command::AddMidiNote {
                    clip: original,
                    start: beat(2),
                    duration: beat(1),
                    pitch: 62,
                    channel: 0,
                    velocity: 100,
                },
                Command::AddMidiNote {
                    clip: alias,
                    start: beat(3),
                    duration: beat(1),
                    pitch: 64,
                    channel: 0,
                    velocity: 100,
                },
            ],
            |_, _| Ok(()),
        )
        .unwrap();
    assert_eq!(
        editor
            .project()
            .midi_source(original)
            .unwrap()
            .notes()
            .len(),
        3
    );
    assert_eq!(
        editor.project().midi_source(original).unwrap(),
        editor.project().midi_source(alias).unwrap()
    );
}

#[test]
fn closed_midi_alias_provenance_retains_every_peer_and_refuses_tampering() {
    let (mut editor, _, original) = midi();
    let alias = duplicate(&mut editor, original);
    let before = editor.project().clone();
    editor
        .execute_midi_prepared_transition::<_, crate::Invalid>(
            Command::AddMidiNote {
                clip: alias,
                start: beat(2),
                duration: beat(1),
                pitch: 64,
                channel: 0,
                velocity: 100,
            },
            |_, _| Ok(()),
        )
        .unwrap();
    let effect = editor
        .next_midi_history_effect(false)
        .expect("closed alias provenance");
    assert!(effect.validates_forward(&before, editor.project()));
    let mut damaged = editor.project().clone();
    damaged
        .midi_clips_mut(original)
        .unwrap()
        .iter_mut()
        .find(|clip| clip.id == original)
        .unwrap()
        .start = beat(1);
    assert!(!effect.validates_forward(&before, &damaged));
    assert!(editor.undo());
    assert!(editor.redo());
    let before_delete = editor.project().clone();
    editor
        .execute_midi_prepared_transition::<_, crate::Invalid>(
            Command::RemoveMidiClip(original),
            |_, _| Ok(()),
        )
        .unwrap();
    assert!(
        editor
            .next_midi_history_effect(false)
            .unwrap()
            .validates_forward(&before_delete, editor.project())
    );
}

#[test]
fn audio_alias_relative_pitch_local_fades_and_clock_changes_keep_positions() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "missing.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    let alias = duplicate(&mut editor, clip);
    editor
        .execute(Command::SetClipFades {
            clip: alias,
            fade_in: 100,
            fade_out: 200,
        })
        .unwrap();
    assert!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .retained_fade()
            .is_neutral()
    );
    editor
        .execute(Command::EditClips {
            clips: vec![clip, alias],
            edit: crate::ClipEdit::PitchBy { cents: 100 },
        })
        .unwrap();
    for id in [clip, alias] {
        assert_eq!(
            editor
                .project()
                .clip(id)
                .unwrap()
                .audio_edit()
                .unwrap()
                .domain
                .pitch_cents,
            100
        );
    }
    let positions: Vec<_> = [clip, alias]
        .iter()
        .map(|id| editor.project().clip(*id).unwrap().start())
        .collect();
    editor
        .execute(Command::SetAudioClipMusical {
            clip: alias,
            enabled: true,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: false,
        })
        .unwrap();
    for (id, position) in [clip, alias].into_iter().zip(positions) {
        let result = editor.project().clip(id).unwrap();
        assert_eq!(result.start(), position);
        assert!(matches!(
            result.audio_edit().unwrap().timing,
            crate::AudioClipTiming::Frozen { .. }
        ));
    }
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
}

#[test]
fn midi_left_extension_preserves_repeating_reversed_peer_occurrences_and_volume() {
    let (mut editor, track, original) = midi();
    editor
        .execute(Command::MoveMidiClip {
            clip: original,
            track,
            start: beat(4),
        })
        .unwrap();
    let alias = duplicate(&mut editor, original);
    editor
        .execute(Command::SetMidiClipSource {
            clip: alias,
            source_length: beat(4),
            source_start: beat(0),
            loop_start: beat(1),
            loop_end: beat(3),
            looping: true,
            length: beat(4),
        })
        .unwrap();
    editor.execute(Command::ReverseClips(vec![alias])).unwrap();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip: alias,
            volume: crate::MidiClipVolume::new(
                vec![
                    crate::MidiClipVolumePoint::linear(
                        beat(0),
                        crate::MidiClipGain::new(0.25).unwrap(),
                    ),
                    crate::MidiClipVolumePoint::linear(
                        beat(4),
                        crate::MidiClipGain::new(1.0).unwrap(),
                    ),
                ],
                beat(4),
            )
            .unwrap(),
        })
        .unwrap();
    let read = |project: &Project| {
        let clip = project.midi_clip(alias).unwrap();
        let playback = clip.source_playback().unwrap();
        let notes: Vec<_> = clip
            .notes()
            .iter()
            .flat_map(|note| {
                playback
                    .note_occurrences(note.start(), note.duration())
                    .unwrap()
                    .iter()
                    .map(|occurrence| {
                        (
                            note.id(),
                            note.pitch(),
                            occurrence.timeline_start(),
                            occurrence.gate_duration(),
                        )
                    })
            })
            .collect();
        let gains: Vec<_> = (0..16)
            .map(|index| {
                let elapsed = BeatTime::from_raw(BeatTime::ONE.raw() * index / 4);
                playback
                    .source_phase_at_elapsed(elapsed)
                    .map(|phase| clip.volume().gain_at(phase))
            })
            .collect();
        (
            clip.start(),
            clip.length(),
            clip.playback_offset(),
            clip.content_view(),
            notes,
            gains,
        )
    };
    let before = read(editor.project());
    editor
        .execute(Command::TrimMidiClipEdge {
            clip: original,
            edge: crate::TrimEdge::Left,
            beat: beat(3),
        })
        .unwrap();
    assert_eq!(read(editor.project()), before);
    assert_eq!(
        editor.project().midi_clip(original).unwrap().start(),
        beat(3)
    );
    assert_eq!(
        editor
            .project()
            .midi_source(original)
            .unwrap()
            .source_length(),
        beat(5)
    );
    assert!(editor.undo());
    assert_eq!(read(editor.project()), before);
    assert!(editor.redo());
    assert_eq!(read(editor.project()), before);
}

#[test]
fn audio_aliases_share_one_clock_transform_across_distinct_tempo_segments() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "missing.wav".into(),
            sample_rate: 48_000,
            frames: 96_000,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: crate::AudioEventEdit::Split { at: 24_000 },
        })
        .unwrap();
    let alias = duplicate(&mut editor, clip);
    editor
        .execute(Command::TrimClipEdge {
            clip: alias,
            edge: crate::TrimEdge::Right,
            frame: 168_000,
        })
        .unwrap();
    editor
        .execute(Command::SetClipFades {
            clip: alias,
            fade_in: 128,
            fade_out: 256,
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    snapshot.tempo_points = vec![
        crate::TempoPoint::new(beat(0), crate::Tempo::new(120_000_000).unwrap()),
        crate::TempoPoint::new(beat(4), crate::Tempo::new(180_000_000).unwrap()),
    ];
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    let settings = crate::default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        crate::WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let a = editor.project().clip(clip).unwrap();
    let b = editor.project().clip(alias).unwrap();
    assert_eq!((a.start(), b.start(), b.duration()), (0, 96_000, 48_000));
    assert!(Arc::ptr_eq(
        &a.audio_edit().unwrap().domain.content,
        &b.audio_edit().unwrap().domain.content
    ));
    assert_eq!(b.audio_edit().unwrap().view_end, beat(3).raw());
    assert!(!b.retained_fade().is_neutral());
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
}

#[test]
fn alias_refusal_and_malformed_snapshots_preserve_ids_and_history() {
    let (mut editor, _, original) = midi();
    let before = editor.project().clone();
    let refused = editor.execute_prepared::<(), crate::Invalid>(
        Command::DuplicateAliasClips(vec![original]),
        |_| Err(crate::Invalid("fixture admission refusal")),
    );
    assert!(refused.is_err());
    assert_eq!(editor.project(), &before);
    let alias = duplicate(&mut editor, original);
    let snapshot = editor.project().snapshot();
    let mut contradictory = snapshot.clone();
    let notes = match &snapshot.aliases.sources[0].content {
        crate::AliasContentSnapshot::Midi { notes, .. } => notes.clone(),
        _ => panic!("midi"),
    };
    let midi = match &mut contradictory.tracks[0].content {
        crate::TrackContentSnapshot::Hybrid {
            midi_clips: clips, ..
        } => clips,
        _ => panic!("midi content"),
    };
    midi.iter_mut()
        .find(|clip| clip.id == alias.value())
        .unwrap()
        .notes = notes;
    assert!(Project::from_snapshot(contradictory).is_err());
    let mut missing = snapshot.clone();
    missing.aliases.sources.clear();
    assert!(Project::from_snapshot(missing).is_err());
    let mut duplicate_member = snapshot.clone();
    duplicate_member
        .aliases
        .members
        .push(duplicate_member.aliases.members[0]);
    assert!(Project::from_snapshot(duplicate_member).is_err());
    let mut orphan = snapshot;
    orphan.aliases.members.clear();
    assert!(Project::from_snapshot(orphan).is_err());
}

#[test]
fn unrelated_edits_retain_effective_source_identity_for_every_alias() {
    let (mut editor, track, original) = midi();
    let alias = duplicate(&mut editor, original);
    let first = Arc::clone(&editor.project().midi_clip(original).unwrap().source);
    let second = Arc::clone(&editor.project().midi_clip(alias).unwrap().source);
    editor
        .execute(Command::RenameTrack {
            track,
            name: "renamed".into(),
        })
        .unwrap();
    assert!(Arc::ptr_eq(
        &first,
        &editor.project().midi_clip(original).unwrap().source
    ));
    assert!(Arc::ptr_eq(
        &second,
        &editor.project().midi_clip(alias).unwrap().source
    ));
}

#[test]
fn explicit_snapshot_projection_keeps_surviving_alias_content_and_drops_unused_sources() {
    let (mut editor, _, original) = midi();
    let alias = duplicate(&mut editor, original);
    let before = editor.project().snapshot();
    let mut projected = before.clone();
    let crate::TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut projected.tracks[0].content
    else {
        panic!("hybrid")
    };
    midi_clips.retain(|clip| clip.id == alias.value());
    projected.retain_clip_aliases_after_projection();
    assert_eq!(projected.aliases.members.len(), 1);
    assert_eq!(projected.aliases.sources, before.aliases.sources);
    let loaded = Project::from_snapshot(projected).unwrap();
    assert_eq!(
        loaded.midi_clip(alias).unwrap().notes(),
        editor.project().midi_clip(original).unwrap().notes()
    );
    assert_eq!(loaded.clip_alias(alias), editor.project().clip_alias(alias));
    let mut empty = loaded.snapshot();
    empty.tracks.clear();
    empty.retain_clip_aliases_after_projection();
    assert!(empty.aliases.members.is_empty() && empty.aliases.sources.is_empty());
    assert!(Project::from_snapshot(empty).is_ok());
    assert_eq!(editor.project().snapshot(), before);
}

/// Control-side metadata/history workload; no audio rendering or native deadlines.
#[test]
#[ignore = "release-only control metadata measurement"]
fn profile_alias_gain_history() {
    let (mut editor, track, mut last) = midi();
    for _ in 1..1_000 {
        last = duplicate(&mut editor, last);
    }
    let mut timings = Vec::with_capacity(400);
    for index in 0..430 {
        let start = std::time::Instant::now();
        editor
            .execute(Command::Gain {
                track,
                gain: if index % 2 == 0 { 0.5 } else { 1.0 },
            })
            .unwrap();
        std::hint::black_box(editor.project());
        if index >= 30 {
            timings.push(start.elapsed().as_nanos());
        }
    }
    timings.sort_unstable();
    println!(
        "alias_control_gain clips=1000 groups=1 history=32 sample_rate=48000 warmup=30 measured=400 p50_ns={} p95_ns={} p99_ns={} max_ns={}",
        timings[200], timings[380], timings[396], timings[399]
    );
}

/// Pulse #3966: commands that replace, delete or re-kind a member outside the
/// shared-edit scope must still detach it, so the project reopens and the
/// surviving sibling stays editable.
#[test]
fn conversion_section_delete_and_overwrite_detach_members_and_keep_the_project_reopenable() {
    for case in ["convert", "delete section", "overwrite"] {
        let (mut editor, track, original) = midi();
        let alias = duplicate(&mut editor, original);
        match case {
            "convert" => {
                let frames = editor.project().midi_bounce_range(original).unwrap().1;
                editor
                    .execute(Command::ConvertMidiClipToAudio {
                        clip: original,
                        path: std::env::temp_dir().join("hex-alias-bounce.wav"),
                        frames,
                    })
                    .unwrap();
            }
            "delete section" => {
                let Created::ArrangementSection(section) = editor
                    .execute(Command::AddArrangementSection {
                        name: "intro".into(),
                        color: crate::ElementColor::Rose,
                        start: beat(0),
                        end: beat(4),
                    })
                    .unwrap()
                else {
                    panic!("section")
                };
                editor
                    .execute(Command::DeleteArrangementSection(section))
                    .unwrap();
            }
            _ => {
                editor
                    .execute(Command::PlaceMidiClipWithOverwrite {
                        track,
                        start: beat(0),
                        length: beat(4),
                    })
                    .unwrap();
            }
        }
        Project::from_snapshot(editor.project().snapshot())
            .unwrap_or_else(|error| panic!("{case}: reopen refused: {error:?}"));
        editor
            .execute(Command::AddMidiNote {
                clip: alias,
                start: beat(2),
                duration: beat(1),
                pitch: 64,
                channel: 0,
                velocity: 100,
            })
            .unwrap_or_else(|error| panic!("{case}: sibling edit refused: {error:?}"));
        Project::from_snapshot(editor.project().snapshot())
            .unwrap_or_else(|error| panic!("{case}: reopen after edit refused: {error:?}"));
    }
}

#[test]
fn alias_at_destination_preserves_originals_and_has_one_history_step() {
    use crate::ClipGroupPosition;
    let (mut editor, source_track, midi) = midi();
    let Created::Track(target) = editor
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(replaced) = editor
        .execute(Command::PlaceMidiClip {
            track: target,
            start: beat(8),
            length: beat(1),
        })
        .unwrap()
    else {
        panic!("destination clip")
    };
    let mut before = editor.project().snapshot();
    let Created::Clips { first: alias, last } = editor
        .execute(Command::DuplicateAliasClipsAt {
            clips: vec![midi],
            anchor: midi,
            track: target,
            position: ClipGroupPosition::Midi(beat(8)),
        })
        .unwrap()
    else {
        panic!("alias")
    };
    assert_eq!(alias, last);
    assert_eq!(
        editor.project().track(source_track).unwrap().midi_clips()[0].id(),
        midi
    );
    assert_eq!(
        editor.project().track(target).unwrap().midi_clips()[0].id(),
        alias
    );
    assert_eq!(editor.project().midi_clip(alias).unwrap().start(), beat(8));
    assert!(
        editor.project().midi_clip(replaced).is_none(),
        "unrelated destination overlap is replaced"
    );
    assert_eq!(
        editor.project().clip_alias(midi),
        editor.project().clip_alias(alias)
    );
    let after = editor.project().snapshot();
    before.next_id = after.next_id;
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
    editor
        .execute(Command::AddMidiNote {
            clip: alias,
            start: beat(2),
            duration: beat(1),
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    assert_eq!(
        editor.project().midi_clip(midi).unwrap().notes(),
        editor.project().midi_clip(alias).unwrap().notes()
    );
}

#[test]
fn alias_at_refuses_overlapping_originals_and_invalid_destinations_atomically() {
    use crate::ClipGroupPosition;
    let (mut editor, track, clip) = midi();
    let before = editor.project().snapshot();
    for (target, position) in [
        (track, beat(1)),
        (track, beat(0)),
        (TrackId(u64::MAX), beat(8)),
    ] {
        assert!(
            editor
                .execute(Command::DuplicateAliasClipsAt {
                    clips: vec![clip],
                    anchor: clip,
                    track: target,
                    position: ClipGroupPosition::Midi(position),
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
    }
}

#[test]
fn audio_alias_at_other_track_shares_content_without_moving_original() {
    use crate::ClipGroupPosition;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Imported { clip, track, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    let Created::Track(target) = editor
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let mut before = editor.project().snapshot();
    let Created::Clips { first: alias, .. } = editor
        .execute(Command::DuplicateAliasClipsAt {
            clips: vec![clip],
            anchor: clip,
            track: target,
            position: ClipGroupPosition::Audio(96_000),
        })
        .unwrap()
    else {
        panic!("alias")
    };
    assert_eq!(editor.project().track(track).unwrap().clips()[0].start(), 0);
    assert_eq!(
        editor.project().track(target).unwrap().clips()[0].start(),
        96_000
    );
    assert_eq!(
        editor.project().clip_alias(clip),
        editor.project().clip_alias(alias)
    );
    let after = editor.project().snapshot();
    before.next_id = after.next_id;
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn alias_to_new_track_is_atomic_for_audio_and_midi() {
    use crate::ClipGroupPosition;
    for midi_source in [false, true] {
        let (mut editor, original) = if midi_source {
            let (editor, _, clip) = midi();
            (editor, clip)
        } else {
            let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
            let Created::Imported { clip, .. } = editor
                .execute(Command::ImportAudioTrack {
                    name: "audio".into(),
                    path: "source.wav".into(),
                    sample_rate: 48_000,
                    frames: 48_000,
                })
                .unwrap()
            else {
                panic!("audio")
            };
            (editor, clip)
        };
        let first_track = editor.project().tracks()[0].id();
        let position = if midi_source {
            ClipGroupPosition::Midi(beat(8))
        } else {
            ClipGroupPosition::Audio(96_000)
        };
        let mut before = editor.project().snapshot();
        let Created::Clips { first: alias, last } = editor
            .execute(Command::DuplicateAliasClipToNewTrack {
                clip: original,
                name: "new track".into(),
                before_track: Some(first_track),
                position,
            })
            .unwrap()
        else {
            panic!("alias")
        };
        assert_eq!(alias, last);
        assert_eq!(editor.project().tracks().len(), 2);
        assert_ne!(editor.project().tracks()[0].id(), first_track);
        assert_eq!(
            editor.project().clip_alias(original),
            editor.project().clip_alias(alias)
        );
        if midi_source {
            assert_eq!(editor.project().tracks()[0].midi_clips()[0].id(), alias);
            assert_eq!(editor.project().midi_clip(alias).unwrap().start(), beat(8));
        } else {
            assert_eq!(editor.project().tracks()[0].clips()[0].id(), alias);
            assert_eq!(editor.project().clip(alias).unwrap().start(), 96_000);
        }
        let after = editor.project().snapshot();
        before.next_id = after.next_id;
        assert!(editor.undo());
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}

#[test]
fn alias_to_new_track_refuses_bad_anchor_or_position_without_mutation() {
    use crate::ClipGroupPosition;
    let (mut editor, _, clip) = midi();
    let before = editor.project().snapshot();
    for (anchor, position) in [
        (Some(TrackId(u64::MAX)), ClipGroupPosition::Midi(beat(8))),
        (None, ClipGroupPosition::Audio(100)),
    ] {
        assert!(
            editor
                .execute(Command::DuplicateAliasClipToNewTrack {
                    clip,
                    name: "new".into(),
                    before_track: anchor,
                    position,
                })
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
    }
}

#[test]
fn alias_to_new_track_respects_track_capacity_without_mutation() {
    let (mut editor, _, clip) = midi();
    while editor.project().tracks().len() < crate::MAX_TRACKS {
        editor
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .unwrap();
    }
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::DuplicateAliasClipToNewTrack {
                clip,
                name: "new".into(),
                before_track: None,
                position: crate::ClipGroupPosition::Midi(beat(8)),
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
}
