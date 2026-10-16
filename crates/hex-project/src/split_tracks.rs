//! Atomic Arrangement splitting at one timeline frame across multiple tracks.

use crate::{Command, Created, Invalid, MAX_TRACKS, Project, TrackId};
use std::collections::HashSet;

pub(crate) fn apply(
    project: &mut Project,
    tracks: &[TrackId],
    frame: u64,
) -> Result<Created, Invalid> {
    if tracks.is_empty() {
        return Err(Invalid("split requires at least one track"));
    }
    if tracks.len() > MAX_TRACKS {
        return Err(Invalid("split track count exceeds project limit"));
    }

    let mut seen = HashSet::new();
    seen.try_reserve(tracks.len())
        .map_err(|_| Invalid("could not reserve split track validation"))?;
    for track_id in tracks {
        if !seen.insert(*track_id) {
            return Err(Invalid("split tracks must be unique"));
        }
        if project.track(*track_id).is_none() {
            return Err(Invalid("split track no longer exists"));
        }
    }

    project.refuse_automation_transform(tracks)?;
    let beat = project.tempo_map().frame_to_beat(frame)?;
    let mut commands = Vec::new();
    let clip_capacity = tracks
        .iter()
        .filter_map(|id| project.track(*id))
        .map(|track| track.clips().len() + track.midi_clips().len())
        .sum::<usize>();
    commands
        .try_reserve(clip_capacity)
        .map_err(|_| Invalid("could not reserve split commands"))?;

    for track_id in tracks {
        let track = project
            .track(*track_id)
            .ok_or(Invalid("split track no longer exists"))?;
        for clip in track.clips() {
            let end = clip
                .start()
                .checked_add(clip.duration())
                .ok_or(Invalid("clip timeline end overflow"))?;
            if clip.start() < frame && frame < end {
                commands.push(Command::SplitClip {
                    clip: clip.id(),
                    frame,
                });
            }
        }
        for clip in track.midi_clips() {
            let end = clip
                .start()
                .raw()
                .checked_add(clip.length().raw())
                .ok_or(Invalid("midi clip timeline end overflow"))?;
            if clip.start() < beat && beat.raw() < end {
                commands.push(Command::SplitMidiClip {
                    clip: clip.id(),
                    beat,
                });
            }
        }
    }
    if commands.is_empty() {
        return Err(Invalid("split cursor must be inside a clip to split"));
    }

    // Apply every split to a private candidate. A failure in a later clip must
    // not leave earlier clips changed when this command is used outside Editor.
    let mut candidate = project.clone();
    for command in commands {
        candidate.apply(command)?;
    }
    *project = candidate;
    Ok(Created::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BeatTime, Editor};
    use std::path::PathBuf;

    fn fixture() -> (Editor, TrackId, TrackId) {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let Created::Track(audio_track) = editor
            .execute(Command::AddTrack {
                name: "audio".into(),
            })
            .expect("audio track")
        else {
            panic!("expected audio track")
        };
        let Created::Track(midi_track) = editor
            .execute(Command::AddTrack {
                name: "midi".into(),
            })
            .expect("midi track")
        else {
            panic!("expected midi track")
        };
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: PathBuf::from("fixture.wav"),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .expect("asset")
        else {
            panic!("expected asset")
        };
        editor
            .execute(Command::PlaceClip {
                track: audio_track,
                asset,
                start: 0,
                duration: 96_000,
                source_offset: 0,
                source_length: 96_000,
                looping: false,
            })
            .expect("audio clip");
        editor
            .execute(Command::PlaceMidiClip {
                track: midi_track,
                start: BeatTime::ZERO,
                length: BeatTime::from_raw(4 * BeatTime::ONE.raw()),
            })
            .expect("midi clip");
        let midi_clip = editor
            .project()
            .track(midi_track)
            .expect("midi track")
            .midi_clips()[0]
            .id();
        editor
            .execute(Command::AddMidiNote {
                clip: midi_clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .expect("midi note");
        (editor, audio_track, midi_track)
    }

    #[test]
    fn mixed_audio_midi_splits_are_one_undoable_edit() {
        let (fixture, audio_track, midi_track) = fixture();
        let before = fixture.project().clone();
        let mut editor = Editor::new(before.clone(), 8).expect("fresh history");
        assert_eq!(
            editor
                .execute(Command::SplitTracksAtFrame {
                    tracks: vec![audio_track, midi_track],
                    frame: 24_000,
                })
                .expect("split all tracks"),
            Created::None
        );
        assert_eq!(
            editor.project().track(audio_track).unwrap().clips().len(),
            2
        );
        assert_eq!(
            editor
                .project()
                .track(midi_track)
                .unwrap()
                .midi_clips()
                .len(),
            2
        );
        assert!(editor.can_undo());
        assert!(editor.undo());
        assert_eq!(
            editor.project().track(audio_track).unwrap().clips(),
            before.track(audio_track).unwrap().clips()
        );
        assert_eq!(
            editor.project().track(midi_track).unwrap().midi_clips(),
            before.track(midi_track).unwrap().midi_clips()
        );
        assert!(!editor.can_undo(), "all splits share one history step");
        assert!(editor.can_redo());
        assert!(editor.redo());
        assert_eq!(
            editor.project().track(audio_track).unwrap().clips().len(),
            2
        );
        assert_eq!(
            editor
                .project()
                .track(midi_track)
                .unwrap()
                .midi_clips()
                .len(),
            2
        );
    }

    #[test]
    fn split_scales_to_one_hundred_mixed_tracks_in_one_history_step() {
        let mut builder = Editor::new(Project::new(48_000).expect("project"), 8).expect("builder");
        let Created::Asset(asset) = builder
            .execute(Command::AddAsset {
                path: PathBuf::from("fixture.wav"),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .expect("asset")
        else {
            panic!("expected asset")
        };
        let mut tracks = Vec::new();
        for index in 0..100 {
            let Created::Track(track) = builder
                .execute(Command::AddTrack {
                    name: format!("track {index}"),
                })
                .expect("track")
            else {
                panic!("expected track")
            };
            tracks.push(track);
            if index % 2 == 0 {
                builder
                    .execute(Command::PlaceClip {
                        track,
                        asset,
                        start: 0,
                        duration: 96_000,
                        source_offset: 0,
                        source_length: 96_000,
                        looping: false,
                    })
                    .expect("audio clip");
            } else {
                builder
                    .execute(Command::PlaceMidiClip {
                        track,
                        start: BeatTime::ZERO,
                        length: BeatTime::from_raw(4 * BeatTime::ONE.raw()),
                    })
                    .expect("midi clip");
            }
        }
        let before = builder.project().clone();
        let mut editor = Editor::new(before, 8).expect("fresh history");
        editor
            .execute(Command::SplitTracksAtFrame {
                tracks,
                frame: 24_000,
            })
            .expect("split one hundred tracks");
        for (index, track) in editor.project().tracks().iter().enumerate() {
            if index % 2 == 0 {
                assert_eq!(track.clips().len(), 2);
                assert!(track.midi_clips().is_empty());
            } else {
                assert_eq!(track.midi_clips().len(), 2);
                assert!(track.clips().is_empty());
            }
        }
        assert!(editor.undo());
        assert!(
            editor
                .project()
                .tracks()
                .iter()
                .all(|track| { track.clips().len() + track.midi_clips().len() == 1 })
        );
        assert!(!editor.can_undo());
    }

    #[test]
    fn invalid_track_sets_and_boundary_cursor_are_atomic() {
        let (fixture, audio_track, midi_track) = fixture();
        let mut project = fixture.project().clone();
        let Created::Track(missing) = project
            .apply(Command::AddTrack {
                name: "temporary".into(),
            })
            .expect("temporary track")
        else {
            panic!("expected temporary track")
        };
        project
            .apply(Command::RemoveTrack(missing))
            .expect("remove temporary track");
        let before = project.clone();
        let mut editor = Editor::new(project, 8).expect("fresh history");
        for tracks in [
            Vec::new(),
            vec![audio_track, audio_track],
            vec![audio_track, missing],
        ] {
            assert!(
                editor
                    .execute(Command::SplitTracksAtFrame {
                        tracks,
                        frame: 24_000
                    })
                    .is_err()
            );
            assert_eq!(editor.project(), &before);
            assert!(!editor.can_undo());
        }

        // Frame 96,000 is the clips' shared end: touching the edge is not a split.
        assert!(
            editor
                .execute(Command::SplitTracksAtFrame {
                    tracks: vec![audio_track, midi_track],
                    frame: 96_000,
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_undo());

        assert!(
            editor
                .execute(Command::SplitTracksAtFrame {
                    tracks: vec![audio_track, midi_track],
                    frame: u64::MAX,
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_undo());
    }

    #[test]
    fn project_apply_rejects_late_failure_without_partial_split() {
        let (editor, audio_track, midi_track) = fixture();
        let mut project = editor.project().clone();
        // The audio split can allocate the final usable ID, then the following
        // MIDI split fails while allocating its new clip ID.
        project.next_id = u64::MAX - 1;
        let before = project.clone();
        assert!(
            project
                .apply(Command::SplitTracksAtFrame {
                    tracks: vec![audio_track, midi_track],
                    frame: 24_000,
                })
                .is_err()
        );
        assert_eq!(project, before);
    }
}
