use hex_project::{
    BeatTime, ClipId, Command, Created, Editor, Invalid, MidiClipGain, MidiClipVolume,
    MidiClipVolumePoint, Project, TrackId,
};
fn beat(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn volume() -> MidiClipVolume {
    MidiClipVolume::new(
        [(0, 0.0), (4, 4.0), (8, 0.0)]
            .into_iter()
            .map(|(b, g)| MidiClipVolumePoint::linear(beat(b), MidiClipGain::new(g).unwrap()))
            .collect(),
        beat(8),
    )
    .unwrap()
}
fn fixture() -> (Editor, TrackId, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "volume".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: beat(0),
            length: beat(8),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    (editor, track, clip)
}
#[test]
fn scalar_gain_survives_copy_split_history_and_curve_replacement() {
    let (mut editor, _, clip) = fixture();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: volume(),
        })
        .unwrap();
    let scalar = hex_project::ClipGain::new(-9_000).unwrap();
    editor
        .execute(Command::SetMidiClipGain { clip, gain: scalar })
        .unwrap();
    let Created::Clip(copy) = editor.execute(Command::DuplicateMidiClip(clip)).unwrap() else {
        panic!("copy")
    };
    assert_eq!(
        editor.project().midi_clip_volume(copy).unwrap().clip_gain(),
        scalar
    );
    let Created::Clip(right) = editor
        .execute(Command::SplitMidiClip {
            clip,
            beat: beat(4),
        })
        .unwrap()
    else {
        panic!("split")
    };
    for id in [clip, right, copy] {
        assert_eq!(
            editor.project().midi_clip_volume(id).unwrap().clip_gain(),
            scalar
        );
    }
    assert!(editor.undo());
    assert!(editor.project().midi_clip(right).is_none());
    assert_eq!(
        editor.project().midi_clip_volume(clip).unwrap().clip_gain(),
        scalar
    );
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: MidiClipVolume::default(),
        })
        .unwrap();
    assert_eq!(
        editor.project().midi_clip_volume(clip).unwrap().clip_gain(),
        scalar
    );
    assert_eq!(
        editor.project().midi_clip_volume(copy).unwrap().points(),
        volume().points()
    );
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
}

#[test]
fn volume_edit_preserves_note_identity_has_own_history_and_does_not_change_copied_clip() {
    let (mut editor, _track, clip) = fixture();
    let before = editor.project().clone();
    let identity = before.midi_clip(clip).unwrap().source_identity();
    editor
        .execute_midi_prepared_transition::<_, Invalid>(
            Command::ReplaceMidiClipVolume {
                clip,
                volume: volume(),
            },
            |a, b| {
                assert_eq!(
                    a.midi_clip(clip).unwrap().source_identity(),
                    b.midi_clip(clip).unwrap().source_identity()
                );
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(
        editor.project().midi_clip(clip).unwrap().source_identity(),
        identity
    );
    let effect = editor.next_midi_history_effect(false).unwrap();
    assert!(effect.validates_forward(&before, editor.project()));
    let after = editor.project().clone();
    assert!(editor.undo());
    assert_eq!(editor.project(), &before);
    assert!(editor.redo());
    assert_eq!(editor.project(), &after);
    let Created::Clip(copy) = editor.execute(Command::DuplicateMidiClip(clip)).unwrap() else {
        panic!("copy")
    };
    assert!(
        editor
            .project()
            .midi_clip_volume(clip)
            .unwrap()
            .same_revision(editor.project().midi_clip_volume(copy).unwrap())
    );
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: MidiClipVolume::default(),
        })
        .unwrap();
    assert_eq!(editor.project().midi_clip_volume(copy), Some(&volume()));
    assert!(
        editor
            .project()
            .midi_clip_volume(clip)
            .unwrap()
            .points()
            .is_empty()
    );
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
}
#[test]
fn envelope_only_range_duplication_is_one_proven_history_edge_and_refusal_preserves_redo() {
    let (mut editor, _, clip) = fixture();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: volume(),
        })
        .unwrap();
    let before = editor.project().clone();
    let mut command = Command::DuplicateMidiSourceRange {
        clip,
        start: beat(1),
        end: beat(3),
    };
    assert!(command.prepare_sampler_midi_edit(editor.project()).unwrap());
    editor
        .execute_midi_prepared_transition::<_, Invalid>(command, |_, _| Ok(()))
        .unwrap();
    let after = editor.project().clone();
    let effect = editor.next_midi_history_effect(false).unwrap();
    assert!(effect.validates_forward(&before, &after));
    for (n, g) in [0.0, 1.0, 2.0, 1.0, 2.0, 3.0, 2.0, 1.0, 0.0]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            after
                .midi_clip_volume(clip)
                .unwrap()
                .gain_at(beat(n as u64))
                .linear(),
            g
        );
    }
    assert!(editor.undo());
    let prior = editor.project().clone();
    assert!(
        editor
            .execute(Command::SetMidiClipSource {
                clip,
                source_length: beat(4),
                source_start: beat(0),
                loop_start: beat(0),
                loop_end: beat(4),
                looping: false,
                length: beat(8)
            })
            .is_err()
    );
    assert_eq!(editor.project(), &prior);
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: volume(),
        })
        .unwrap();
    assert!(editor.redo(), "no-op or refused edit must preserve redo");
    assert_eq!(editor.project(), &after);
}
#[test]
fn console_copy_and_left_prefix_keep_independent_volume_and_source_coordinates() {
    let (mut editor, track, clip) = fixture();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: volume(),
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(console) = editor
        .execute(Command::CopyArrangementMidiToSession {
            source: clip,
            scene,
        })
        .unwrap()
    else {
        panic!("console")
    };
    assert_eq!(
        editor
            .project()
            .session_midi_clip(console)
            .unwrap()
            .volume(),
        &volume()
    );
    editor
        .execute(Command::MoveMidiClip {
            clip,
            track,
            start: beat(2),
        })
        .unwrap();
    editor
        .execute(Command::TrimMidiClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            beat: beat(0),
        })
        .unwrap();
    let shifted = editor.project().midi_clip_volume(clip).unwrap();
    assert_eq!(shifted.points()[0].beat, beat(2));
    assert_eq!(shifted.gain_at(beat(6)).linear(), 4.0);
    assert_eq!(
        editor
            .project()
            .session_midi_clip(console)
            .unwrap()
            .volume()
            .points()[0]
            .beat,
        beat(0)
    );
}

#[test]
fn frame_range_clipboard_preserves_volume_across_source_and_destination_tempo_changes() {
    let (mut editor, track, clip) = fixture();
    editor
        .execute(Command::SetTempoPoint {
            beat: beat(2),
            tempo: hex_project::Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: beat(10),
            tempo: hex_project::Tempo::new(180_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: beat(14),
            tempo: hex_project::Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: volume(),
        })
        .unwrap();
    let scalar = hex_project::ClipGain::new(-6_000).unwrap();
    editor
        .execute(Command::SetMidiClipGain { clip, gain: scalar })
        .unwrap();
    let map = editor.project().tempo_map().clone();
    let start = map.beat_to_frame(beat(1)).unwrap();
    let end = map.beat_to_frame(beat(7)).unwrap();
    let destination = map.beat_to_frame(beat(12)).unwrap();
    let clipboard = editor
        .project()
        .capture_time_range(&hex_project::TimeRange::new(vec![track], start, end).unwrap())
        .unwrap();
    let before = editor.project().clone();
    editor
        .execute(Command::PasteTimeRange {
            clipboard: std::sync::Arc::new(clipboard),
            track,
            frame: destination,
        })
        .unwrap();
    let pasted = editor
        .project()
        .track(track)
        .unwrap()
        .midi_clips()
        .iter()
        .find(|owner| owner.id() != clip)
        .unwrap();
    assert_eq!(pasted.volume().clip_gain(), scalar);
    for offset in [
        0,
        12000,
        24000,
        48000,
        64000,
        96000,
        144000,
        192000,
        end - start,
    ] {
        let original_beat = map.frame_to_beat(start + offset).unwrap();
        let pasted_beat = BeatTime::from_raw(
            map.frame_to_beat(destination + offset).unwrap().raw() - pasted.start().raw(),
        );
        let actual = pasted.volume().gain_at(pasted_beat).linear();
        let expected = volume().gain_at(original_beat).linear();
        assert!(
            (actual - expected).abs() <= 1.0e-8,
            "frame {offset}: actual {actual}, expected {expected}"
        );
    }
    assert!(editor.undo());
    let mut expected = before.snapshot();
    expected.next_id = editor.project().next_identity();
    assert_eq!(editor.project().snapshot(), expected);
    assert!(editor.redo());
}

#[test]
fn frame_clipboard_keeps_latest_equal_frame_knot_limits() {
    let (mut editor, track, clip) = fixture();
    let after = BeatTime::from_raw(beat(1).raw() + 1);
    let authored = MidiClipVolume::new(
        [
            (beat(0), 0.0, 0.0),
            (beat(1), 1.0, 2.0),
            (after, 3.0, 0.0),
            (beat(2), 4.0, 4.0),
        ]
        .into_iter()
        .map(|(beat, incoming, value)| MidiClipVolumePoint {
            beat,
            incoming: MidiClipGain::new(incoming).unwrap(),
            value: MidiClipGain::new(value).unwrap(),
        })
        .collect(),
        beat(8),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: authored,
        })
        .unwrap();
    let map = editor.project().tempo_map();
    assert_eq!(
        map.beat_to_frame(beat(1)).unwrap(),
        map.beat_to_frame(after).unwrap()
    );
    let end = map.beat_to_frame(beat(2)).unwrap();
    let destination = map.beat_to_frame(beat(12)).unwrap();
    let clipboard = editor
        .project()
        .capture_time_range(&hex_project::TimeRange::new(vec![track], 0, end).unwrap())
        .unwrap();
    editor
        .execute(Command::PasteTimeRange {
            clipboard: std::sync::Arc::new(clipboard),
            track,
            frame: destination,
        })
        .unwrap();
    let pasted = editor
        .project()
        .track(track)
        .unwrap()
        .midi_clips()
        .iter()
        .find(|owner| owner.id() != clip)
        .unwrap();
    assert_eq!(pasted.volume().gain_before(beat(1)).linear(), 3.0);
    assert_eq!(pasted.volume().gain_at(beat(1)).linear(), 0.0);
    assert_eq!(
        pasted
            .volume()
            .gain_at(BeatTime::from_raw(beat(1).raw() / 2))
            .linear(),
        1.5
    );
    let saved = pasted.volume().clone();
    assert!(editor.undo());
    assert!(editor.redo());
    let pasted = editor
        .project()
        .track(track)
        .unwrap()
        .midi_clips()
        .iter()
        .find(|owner| owner.id() != clip)
        .unwrap();
    assert_eq!(pasted.volume(), &saved);
}

#[test]
fn frame_range_keeps_volume_when_authored_clip_edges_share_earlier_inverse_beats() {
    let (mut editor, track, clip) = fixture();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: volume(),
        })
        .unwrap();
    let start = BeatTime::from_raw(beat(2).raw() + 1);
    editor
        .execute(Command::MoveMidiClip { clip, track, start })
        .unwrap();
    let map = editor.project().tempo_map();
    let first = map.beat_to_frame(start).unwrap();
    assert!(map.frame_to_beat(first).unwrap() < start);
    let end = map
        .beat_to_frame(BeatTime::from_raw(start.raw() + beat(8).raw()))
        .unwrap();
    let copy = editor
        .project()
        .capture_time_range(&hex_project::TimeRange::new(vec![track], first, end).unwrap())
        .unwrap();
    let destination = map.beat_to_frame(beat(12)).unwrap();
    editor
        .execute(Command::PasteTimeRange {
            clipboard: std::sync::Arc::new(copy),
            track,
            frame: destination,
        })
        .unwrap();
    let pasted = editor
        .project()
        .track(track)
        .unwrap()
        .midi_clips()
        .iter()
        .find(|owner| owner.id() != clip)
        .unwrap();
    assert_eq!(pasted.volume().gain_at(BeatTime::ZERO).linear(), 0.0);
    assert_eq!(pasted.volume().gain_at(beat(4)).linear(), 4.0);
    assert_eq!(pasted.volume().gain_at(beat(8)).linear(), 0.0);
}

#[test]
fn aggregate_volume_budget_refuses_a_copy_atomically_with_ids_history_and_redo_intact() {
    let (mut editor, _, mut clip) = fixture();
    let gain = MidiClipGain::new(0.5).unwrap();
    let points = (0..hex_project::MAX_MIDI_VOLUME_POINTS_PER_CLIP)
        .map(|raw| MidiClipVolumePoint::linear(BeatTime::from_raw(raw as u64), gain))
        .collect();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: MidiClipVolume::new(points, beat(8)).unwrap(),
        })
        .unwrap();
    for _ in 1..hex_project::MAX_MIDI_VOLUME_POINTS / hex_project::MAX_MIDI_VOLUME_POINTS_PER_CLIP {
        let Created::Clip(next) = editor.execute(Command::DuplicateMidiClip(clip)).unwrap() else {
            panic!("copy")
        };
        clip = next;
    }
    editor
        .execute(Command::AddTrack {
            name: "redo sentinel".into(),
        })
        .unwrap();
    assert!(editor.undo());
    let before = editor.project().clone();
    assert!(editor.execute(Command::DuplicateMidiClip(clip)).is_err());
    assert_eq!(editor.project(), &before);
    assert!(
        editor.redo(),
        "capacity refusal must preserve prior redo and identity allocation"
    );
}

#[test]
fn split_and_arrangement_overwrite_materialize_real_loop_seam_limits_without_changing_left_source()
{
    let (mut editor, track, clip) = fixture();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: volume(),
        })
        .unwrap();
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: beat(8),
            source_start: beat(1),
            loop_start: beat(1),
            loop_end: beat(6),
            looping: true,
            length: beat(12),
        })
        .unwrap();
    let before = editor.project().clone();
    let identity = before.midi_clip(clip).unwrap().source_identity();
    let Created::Clip(right) = editor
        .execute(Command::SplitMidiClip {
            clip,
            beat: beat(4),
        })
        .unwrap()
    else {
        panic!("split")
    };
    let right_volume = editor.project().midi_clip_volume(right).unwrap();
    assert_eq!(right_volume.gain_at(BeatTime::ZERO).linear(), 3.0);
    assert_eq!(right_volume.gain_before(beat(1)).linear(), 2.0);
    assert_eq!(right_volume.gain_at(beat(1)).linear(), 1.0);
    assert_eq!(right_volume.gain_at(beat(2)).linear(), 2.0);
    assert_eq!(
        editor.project().midi_clip(clip).unwrap().source_identity(),
        identity
    );
    assert_eq!(editor.project().midi_clip_volume(clip), Some(&volume()));
    assert!(editor.undo());
    editor
        .execute(Command::PlaceMidiClipWithOverwrite {
            track,
            start: beat(2),
            length: beat(2),
        })
        .unwrap();
    let tail = editor
        .project()
        .track(track)
        .unwrap()
        .midi_clips()
        .iter()
        .find(|owner| owner.start() == beat(4))
        .unwrap();
    assert_eq!(tail.volume().gain_at(BeatTime::ZERO).linear(), 3.0);
    assert_eq!(tail.volume().gain_before(beat(1)).linear(), 2.0);
    assert_eq!(tail.volume().gain_at(beat(1)).linear(), 1.0);
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
}
