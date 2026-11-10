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
    let playback = clip
        .source_playback()
        .map_err(|_| "midi source timing is invalid")?;
    let occurrence = playback
        .note_occurrences(source_beat, BeatTime::from_raw(1))
        .map_err(|_| "cue is outside the playable midi source")?
        .iter()
        .next()
        .ok_or("cue is outside this clip's playable time")?;
    let absolute = clip
        .start()
        .raw()
        .checked_add(occurrence.timeline_start().raw())
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
}
