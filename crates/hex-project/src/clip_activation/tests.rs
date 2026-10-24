use super::*;
use crate::{Command, Editor, MidiSourcePlayback, TrackContentSnapshot};
fn b(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn fixture() -> (Editor, crate::TrackId, ClipId) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Track(track) = e
        .execute(Command::AddTrack {
            name: "midi".into(),
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
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: b(4),
        source_start: b(0),
        loop_start: b(1),
        loop_end: b(4),
        looping: true,
        length: b(12),
    })
    .unwrap();
    e.execute(Command::AddMidiNote {
        clip,
        start: b(2),
        duration: b(2),
        pitch: 60,
        channel: 0,
        velocity: 100,
    })
    .unwrap();
    (e, track, clip)
}
#[test]
fn span_activation_preserves_source_law_fresh_ids_and_history() {
    let (mut e, track, clip) = fixture();
    let mut before = e.project().snapshot();
    let original = e.project().midi_clip(clip).unwrap().clone();
    let frame = |n| e.project().tempo_map().beat_to_frame(b(n)).unwrap();
    let range = TimeRange::new(vec![track], frame(3), frame(8)).unwrap();
    assert_eq!(
        e.project().time_range_has_active_clips(&range),
        Ok(Some(true))
    );
    e.execute(Command::SetTimeRangeActive {
        range: range.clone(),
        active: false,
    })
    .unwrap();
    let clips = e.project().track(track).unwrap().midi_clips();
    assert_eq!(clips.len(), 3);
    assert_eq!(
        clips
            .iter()
            .map(|c| (c.start(), c.length(), c.playback_offset(), c.active()))
            .collect::<Vec<_>>(),
        vec![
            (b(0), b(3), b(0), true),
            (b(3), b(5), b(3), false),
            (b(8), b(4), b(8), true)
        ]
    );
    assert_eq!(clips[0].id(), clip);
    assert_eq!(clips[0].notes()[0].id(), original.notes()[0].id());
    assert_ne!(clips[1].notes()[0].id(), clips[2].notes()[0].id());
    for part in clips {
        assert_eq!(part.source_start(), original.source_start());
        assert_eq!(part.loop_start(), original.loop_start());
        assert_eq!(part.loop_end(), original.loop_end());
        assert!(part.looping());
        assert_eq!(part.notes()[0].start(), b(2));
    }
    let tail = clips[2]
        .source_playback()
        .unwrap()
        .note_occurrences(b(2), b(2))
        .unwrap()
        .iter()
        .next()
        .unwrap();
    assert_eq!(
        (
            tail.timeline_start(),
            tail.gate_duration(),
            tail.iteration()
        ),
        (b(0), b(2), 2)
    );
    assert_eq!(
        e.project().time_range_has_active_clips(&range),
        Ok(Some(false))
    );
    let after = e.project().snapshot();
    // History preserves the monotonic ID high-water mark while restoring content.
    before.next_id = after.next_id;
    assert!(e.undo());
    assert_eq!(e.project().snapshot(), before);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}
#[test]
fn window_chases_crossing_gates_and_counts_tiny_huge_offset_without_expansion() {
    let source = MidiSourcePlayback::new(b(4), b(0), Some((b(1), b(4))), b(1))
        .unwrap()
        .with_playback_offset(b(3))
        .unwrap();
    let plan = source.note_occurrences(b(2), b(2)).unwrap();
    assert_eq!(plan.count(), 1);
    let hit = plan.iter().next().unwrap();
    assert_eq!(
        (hit.timeline_start(), hit.gate_duration(), hit.iteration()),
        (b(0), b(1), 0)
    );
    let huge = MidiSourcePlayback::new(
        BeatTime::from_raw(4),
        BeatTime::ZERO,
        Some((BeatTime::from_raw(1), BeatTime::from_raw(4))),
        BeatTime::from_raw(1),
    )
    .unwrap()
    .with_playback_offset(BeatTime::from_raw(u64::MAX - 2))
    .unwrap();
    let plan = huge
        .note_occurrences(BeatTime::from_raw(1), BeatTime::from_raw(3))
        .unwrap();
    assert_eq!(plan.count(), plan.iter().count() as u64);
    assert_eq!(plan.count(), 1);
    assert!(plan.iter().next().unwrap().iteration() > 1_000_000_000);
    let silent = MidiSourcePlayback::one_shot(b(4))
        .unwrap()
        .with_playback_offset(b(8))
        .unwrap();
    assert_eq!(silent.note_occurrences(b(0), b(4)).unwrap().count(), 0);
}
#[test]
fn activation_refusal_preserves_ids_state_and_redo() {
    let (mut e, _, clip) = fixture();
    e.execute(Command::SetClipActive {
        clip,
        active: false,
    })
    .unwrap();
    assert!(e.undo());
    let before = e.project().snapshot();
    let range = TimeRange::new(vec![crate::TrackId(u64::MAX)], 1, 2).unwrap();
    assert!(
        e.execute(Command::SetTimeRangeActive {
            range,
            active: false
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    assert!(e.can_redo());
    let invalid = e
        .project()
        .midi_clip(clip)
        .unwrap()
        .source_playback()
        .unwrap()
        .with_playback_offset(BeatTime::from_raw(u64::MAX));
    assert!(invalid.is_err());
}
#[test]
fn offset_left_trim_rewinds_to_origin_then_existing_prefix_law() {
    let (e, _, clip) = fixture();
    let mut snapshot = e.project().snapshot();
    let TrackContentSnapshot::Hybrid { midi_clips, .. } = &mut snapshot.tracks[0].content else {
        panic!("hybrid")
    };
    midi_clips[0].start_raw = b(8).raw();
    midi_clips[0].playback_offset_raw = b(3).raw();
    let p = Project::from_snapshot(snapshot).unwrap();
    let clip = p.midi_clip(clip).unwrap();
    let inward = clip.preview_edge_trim(crate::TrimEdge::Left, b(9)).unwrap();
    assert_eq!(inward.playback_offset(), b(4));
    let rewind = clip.preview_edge_trim(crate::TrimEdge::Left, b(6)).unwrap();
    assert_eq!(rewind.playback_offset(), b(1));
    let before_origin = clip.preview_edge_trim(crate::TrimEdge::Left, b(4)).unwrap();
    assert_eq!(before_origin.playback_offset(), b(0));
}

#[test]
fn fragmented_id_exhaustion_is_atomic_and_same_activation_does_not_split() {
    let (e, track, clip) = fixture();
    let mut snapshot = e.project().snapshot();
    snapshot.next_id = u64::MAX - 1;
    let mut e = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    let range = TimeRange::new(
        vec![track],
        e.project().tempo_map().beat_to_frame(b(3)).unwrap(),
        e.project().tempo_map().beat_to_frame(b(8)).unwrap(),
    )
    .unwrap();
    let before = e.project().snapshot();
    e.execute(Command::SetTimeRangeActive {
        range: range.clone(),
        active: true,
    })
    .unwrap();
    assert_eq!(e.project().snapshot(), before);
    assert!(!e.can_undo());
    assert!(
        e.execute(Command::SetTimeRangeActive {
            range,
            active: false
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    assert!(!e.can_undo());
    e.execute(Command::SetClipActive {
        clip,
        active: false,
    })
    .unwrap();
    assert_eq!(e.project().clip_active(clip), Some(false));
}
