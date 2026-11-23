//! Pure Arrangement MIDI cue projection through the canonical project tempo map.
use hex_project::{BeatTime, ClipId, Project};

pub(super) fn arrangement_destination(
    project: &Project,
    clip: ClipId,
    source_beat: BeatTime,
) -> Result<Option<u64>, &'static str> {
    let Some(clip) = project.midi_clip(clip) else {
        return Ok(None);
    };
    let timeline_offset = if !clip.looping() && clip.content_view().is_none() {
        let (scope_start, scope_end) = clip.scope_range().ok_or("midi source timing is invalid")?;
        if source_beat < scope_start || source_beat >= scope_end {
            return Err("cue is outside this clip's playable time");
        }
        source_beat.raw() - scope_start.raw()
    } else {
        let playback = clip
            .source_playback()
            .map_err(|_| "midi source timing is invalid")?;
        let occurrence = playback
            .note_occurrences(source_beat, BeatTime::from_raw(1))
            .map_err(|_| "cue is outside the playable midi source")?
            .iter()
            .next()
            .ok_or("cue is outside this clip's playable time")?;
        occurrence.timeline_start().raw()
    };
    let absolute = clip
        .start()
        .raw()
        .checked_add(timeline_offset)
        .ok_or("midi cue position overflow")?;
    project
        .tempo_map()
        .beat_to_frame(BeatTime::from_raw(absolute))
        .map(Some)
        .map_err(|_| "midi cue tempo conversion failed")
}

/// Clip-elapsed quarter-note beats at an Arrangement frame, mapped through the
/// tempo map; none outside the placement's half-open `start..start + length`.
pub(in crate::editor) fn arrangement_elapsed(
    tempo: &hex_project::TempoMap,
    start: BeatTime,
    length: BeatTime,
    frame: u64,
) -> Option<BeatTime> {
    let elapsed = tempo
        .frame_to_beat(frame)
        .ok()?
        .raw()
        .checked_sub(start.raw())?;
    (elapsed < length.raw()).then_some(BeatTime::from_raw(elapsed))
}

/// Source-beat playback cursor inside an Arrangement MIDI clip. Repeating clips
/// fold through their loop law; unlooped clips stay visible across their full
/// Arrangement length even past `source_length` or a retained `loop_end` bound.
pub(super) fn arrangement_source_beat(
    placement: &hex_project::MidiClip,
    elapsed: BeatTime,
) -> Option<BeatTime> {
    if elapsed >= placement.arrangement_length() {
        return None;
    }
    if placement.looping() {
        return placement
            .source_playback()
            .ok()?
            .source_phase_at_elapsed(elapsed);
    }
    let (offset, read) = match placement.content_view() {
        Some(view) if view.reversed => (
            view.start,
            placement
                .arrangement_length()
                .raw()
                .checked_sub(elapsed.raw())?
                .checked_sub(1)?,
        ),
        Some(view) => (view.start, elapsed.raw()),
        None => (placement.playback_offset().raw(), elapsed.raw()),
    };
    placement
        .source_start()
        .raw()
        .checked_add(offset)?
        .checked_add(read)
        .map(BeatTime::from_raw)
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Tempo};
    fn beat(value: u64) -> BeatTime {
        BeatTime::from_quarters(value).unwrap()
    }
    #[test]
    fn arrangement_cue_projects_intro_repeat_and_absolute_tempo_before_seeking() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack { name: "cue".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(8),
                length: beat(4),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        let source = |length| Command::SetMidiClipSource {
            clip,
            source_length: beat(8),
            source_start: beat(4),
            loop_start: beat(2),
            loop_end: beat(6),
            looping: true,
            length: beat(length),
        };
        editor.execute(source(4)).unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: beat(10),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        assert_eq!(
            arrangement_destination(editor.project(), clip, beat(4)),
            Ok(Some(192_000))
        );
        assert_eq!(
            arrangement_destination(editor.project(), clip, beat(5)),
            Ok(Some(216_000))
        );
        assert_eq!(
            arrangement_destination(editor.project(), clip, beat(3)),
            Ok(Some(288_000))
        );
        assert!(arrangement_destination(editor.project(), clip, beat(1)).is_err());
        assert!(arrangement_destination(editor.project(), clip, beat(6)).is_err());
        editor.execute(source(2)).unwrap();
        assert!(arrangement_destination(editor.project(), clip, beat(3)).is_err());
    }

    #[test]
    fn arrangement_playhead_elapsed_follows_a_tempo_change_and_hides_outside_the_clip() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        // 120 bpm until beat 10, then 60 bpm: beat 10 is 240_000 frames and
        // each later beat lasts 48_000 frames.
        editor
            .execute(Command::SetTempoPoint {
                beat: beat(10),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        let tempo = editor.project().tempo_map();
        let elapsed = |frame| arrangement_elapsed(tempo, beat(8), beat(4), frame);
        assert_eq!(elapsed(191_999), None);
        assert_eq!(elapsed(192_000), Some(BeatTime::ZERO));
        assert_eq!(elapsed(240_000), Some(beat(2)));
        assert_eq!(elapsed(264_000), Some(BeatTime::from_ratio(5, 2).unwrap()));
        assert_eq!(elapsed(335_999).map(|b| b < beat(4)), Some(true));
        assert_eq!(elapsed(336_000), None);
    }

    #[test]
    fn arrangement_playhead_stays_visible_past_source_and_scope_bounds_inside_unlooped_clip() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack { name: "cue".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip_id) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(0),
                length: BeatTime::from_ratio(1, 2).unwrap(),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        // Extend the unlooped Arrangement clip to 4b while source/loop remain 0.5b..1.25b.
        editor
            .execute(Command::TrimMidiClipEdge {
                clip: clip_id,
                edge: hex_project::TrimEdge::Right,
                beat: beat(4),
            })
            .unwrap();
        editor
            .execute(Command::EditMidiNotesWithSourceExtent {
                clip: clip_id,
                remove: Vec::new(),
                insert: vec![hex_project::MidiNoteInsert {
                    active: true,
                    start: beat(1),
                    duration: BeatTime::from_ratio(1, 4).unwrap(),
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                }],
            })
            .unwrap();
        let clip = editor.project().midi_clip(clip_id).unwrap();
        assert_eq!(clip.scope_range(), Some((beat(0), beat(4))));
        // Playback marker remains visible past loop_end (0.5b) and source_length (1.25b).
        assert_eq!(
            arrangement_source_beat(clip, BeatTime::from_ratio(3, 4).unwrap()),
            Some(BeatTime::from_ratio(3, 4).unwrap())
        );
        assert_eq!(arrangement_source_beat(clip, beat(2)), Some(beat(2)));
        assert_eq!(
            arrangement_source_beat(clip, BeatTime::from_ratio(15, 4).unwrap()),
            Some(BeatTime::from_ratio(15, 4).unwrap())
        );
        assert_eq!(arrangement_source_beat(clip, beat(4)), None);
        // Cueing at 2.5b inside the 4b unlooped Arrangement clip also succeeds.
        assert_eq!(
            arrangement_destination(
                editor.project(),
                clip_id,
                BeatTime::from_ratio(5, 2).unwrap()
            ),
            Ok(Some(60_000))
        );
        assert!(arrangement_destination(editor.project(), clip_id, beat(4)).is_err());

        // Even with a LoopEnd one-shot boundary retained at 0.5b, playhead stays visible across 0..4b.
        let mut snap = editor.project().snapshot();
        if let hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } =
            &mut snap.tracks[0].content
        {
            midi_clips[0].one_shot_boundary = hex_project::MidiOneShotBoundary::LoopEnd;
        }
        let bounded = Project::from_snapshot(snap).unwrap();
        let bounded_clip = bounded.midi_clip(clip_id).unwrap();
        assert_eq!(
            arrangement_source_beat(bounded_clip, beat(3)),
            Some(beat(3))
        );

        // Re-enabling looping wraps inside the loop cycle as before.
        editor
            .execute(Command::SetMidiClipSource {
                clip: clip_id,
                source_length: beat(4),
                source_start: beat(0),
                loop_start: beat(0),
                loop_end: beat(1),
                looping: true,
                length: beat(4),
            })
            .unwrap();
        let looped = editor.project().midi_clip(clip_id).unwrap();
        assert_eq!(
            arrangement_source_beat(looped, BeatTime::from_ratio(5, 2).unwrap()),
            Some(BeatTime::from_ratio(1, 2).unwrap())
        );
    }
}
