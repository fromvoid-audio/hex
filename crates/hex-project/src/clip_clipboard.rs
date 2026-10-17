//! Bounded whole-clip metadata copied on control; no PCM, selection or UI state.
use crate::{
    Asset, AssetId, BeatTime, Clip, ClipId, Created, Invalid, MAX_CLIPS, MidiClip, Project, TrackId,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
enum Content {
    Audio(Clip),
    Midi(MidiClip),
}
#[derive(Clone, Debug)]
struct Captured {
    track: TrackId,
    offset: u64,
    content: Content,
}
/// An immutable snapshot of complete Arrangement clips, scoped to one project.
/// It retains original clip/source domains rather than cutting a time rectangle.
#[derive(Clone, Debug)]
pub struct ClipClipboard {
    sample_rate: u32,
    tracks: Vec<TrackId>,
    clips: Vec<Captured>,
    assets: Vec<(AssetId, Asset)>,
}
struct Prepared {
    track: TrackId,
    frame: u64,
    end: u64,
    beat: BeatTime,
    content: Content,
}
impl Project {
    pub fn capture_clips(&self, ids: &[ClipId]) -> Result<ClipClipboard, Invalid> {
        if ids.is_empty() || ids.len() > MAX_CLIPS {
            return Err(Invalid("clip selection exceeds clip limits"));
        }
        let selected = ids.iter().copied().collect::<BTreeSet<_>>();
        if selected.len() != ids.len() {
            return Err(Invalid("clip selection contains duplicate ids"));
        }
        let mut clipboard = ClipClipboard {
            sample_rate: self.sample_rate(),
            tracks: Vec::new(),
            clips: Vec::new(),
            assets: Vec::new(),
        };
        clipboard
            .clips
            .try_reserve(ids.len())
            .map_err(|_| Invalid("could not reserve clip clipboard"))?;
        let mut origin = u64::MAX;
        for track in self.tracks() {
            let before = clipboard.clips.len();
            for clip in track.clips().iter().filter(|c| selected.contains(&c.id())) {
                origin = origin.min(clip.start());
                if !clipboard.assets.iter().any(|(id, _)| *id == clip.asset()) {
                    let asset = self
                        .asset(clip.asset())
                        .cloned()
                        .ok_or(Invalid("copied asset no longer exists"))?;
                    clipboard.assets.push((clip.asset(), asset));
                }
                clipboard.clips.push(Captured {
                    track: track.id(),
                    offset: clip.start(),
                    content: Content::Audio(clip.clone()),
                });
            }
            for clip in track
                .midi_clips()
                .iter()
                .filter(|c| selected.contains(&c.id()))
            {
                let frame = self.tempo_map().beat_to_frame(clip.start())?;
                origin = origin.min(frame);
                clipboard.clips.push(Captured {
                    track: track.id(),
                    offset: frame,
                    content: Content::Midi(clip.clone()),
                });
            }
            if clipboard.clips.len() != before {
                clipboard.tracks.push(track.id());
            }
        }
        if clipboard.clips.len() != ids.len() {
            return Err(Invalid("selected arrangement clip no longer exists"));
        }
        for clip in &mut clipboard.clips {
            clip.offset -= origin;
        }
        Ok(clipboard)
    }
}
impl ClipClipboard {
    fn prepare(
        &self,
        project: &Project,
        target: TrackId,
        frame: u64,
    ) -> Result<Vec<Prepared>, Invalid> {
        if self.sample_rate != project.sample_rate() {
            return Err(Invalid("clip clipboard belongs to another sample rate"));
        }
        for (id, expected) in &self.assets {
            if project.asset(*id) != Some(expected) {
                return Err(Invalid("copied media no longer matches the project"));
            }
        }
        let mut prepared = Vec::new();
        prepared
            .try_reserve(self.clips.len())
            .map_err(|_| Invalid("could not reserve clip paste"))?;
        for original in &self.clips {
            let track = if self.tracks.len() == 1 {
                target
            } else {
                original.track
            };
            project
                .track(track)
                .ok_or(Invalid("paste track no longer exists"))?;
            let start = frame
                .checked_add(original.offset)
                .ok_or(Invalid("paste clip start overflow"))?;
            let beat = match &original.content {
                Content::Audio(_) => BeatTime::ZERO,
                Content::Midi(_) => project.tempo_map().frame_to_beat(start)?,
            };
            let (start, end) = match &original.content {
                Content::Audio(clip) => {
                    let mut projected = clip.clone();
                    crate::audio_edit::move_clip(&mut projected, start, project.tempo_map())?;
                    (
                        start,
                        start
                            .checked_add(projected.duration())
                            .ok_or(Invalid("paste clip end overflow"))?,
                    )
                }
                Content::Midi(clip) => {
                    let end = beat
                        .raw()
                        .checked_add(clip.length().raw())
                        .ok_or(Invalid("paste midi end overflow"))?;
                    (
                        project.tempo_map().beat_to_frame(beat)?,
                        project.tempo_map().beat_to_frame(BeatTime::from_raw(end))?,
                    )
                }
            };
            prepared.push(Prepared {
                track,
                frame: start,
                end,
                beat,
                content: original.content.clone(),
            });
        }
        prepared.sort_by_key(|copy| (copy.track, copy.frame));
        Ok(prepared)
    }
}
pub(crate) fn paste(
    project: &mut Project,
    clipboard: &ClipClipboard,
    target: TrackId,
    frame: u64,
    insert: bool,
) -> Result<Created, Invalid> {
    let prepared = clipboard.prepare(project, target, frame)?;
    if insert {
        let end = prepared
            .iter()
            .map(|c| c.end)
            .max()
            .ok_or(Invalid("clip clipboard is empty"))?;
        let duration = end
            .checked_sub(frame)
            .filter(|n| *n > 0)
            .ok_or(Invalid("pasted clip span collapsed"))?;
        let tracks = if clipboard.tracks.len() == 1 {
            vec![target]
        } else {
            clipboard.tracks.clone()
        };
        crate::time_range::insert_empty(project, tracks, frame, duration)?;
    }
    // Apply the existing replacement policy only to old destination content.
    // Batch members may overlap in a valid project and must not erase each other.
    // Empty gaps remain outside the replacement, just as with a single clip copy.
    for copy in &prepared {
        match &copy.content {
            Content::Audio(_) => {
                crate::edit::overwrite_audio_interval(
                    project,
                    copy.track,
                    copy.frame,
                    copy.end,
                    ClipId(0),
                )?;
                crate::midi::overwrite_midi_in_frames(
                    project,
                    copy.track,
                    copy.frame,
                    copy.end,
                    ClipId(0),
                )?;
            }
            Content::Midi(clip) => {
                let end = BeatTime::from_raw(
                    copy.beat
                        .raw()
                        .checked_add(clip.length().raw())
                        .ok_or(Invalid("midi timeline end overflow"))?,
                );
                crate::edit::overwrite_audio_in_beats(
                    project,
                    copy.track,
                    copy.beat,
                    end,
                    ClipId(0),
                )?;
                crate::midi::overwrite_midi_interval(
                    project,
                    copy.track,
                    copy.beat,
                    end,
                    ClipId(0),
                )?;
            }
        }
    }
    for copy in prepared {
        match copy.content {
            Content::Audio(mut clip) => {
                crate::audio_edit::move_clip(&mut clip, copy.frame, project.tempo_map())?;
                crate::clip_copy::append_audio(project, clip, copy.track)?;
            }
            Content::Midi(clip) => {
                crate::clip_copy::append_midi(project, clip, copy.track, copy.beat)?;
            }
        }
    }
    Project::from_snapshot(project.snapshot())?;
    Ok(Created::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Editor, NoteId};
    use std::sync::Arc;
    fn beat(value: u64) -> BeatTime {
        BeatTime::from_quarters(value).unwrap()
    }
    fn fixture() -> (Editor, TrackId, TrackId, ClipId, ClipId, ClipId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let Created::Imported {
            track,
            asset,
            clip: imported,
        } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "clip.wav".into(),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .unwrap()
        else {
            panic!("import")
        };
        editor.execute(Command::RemoveClip(imported)).unwrap();
        let Created::Clip(audio) = editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: 48_000,
                source_offset: 100,
                source_length: 12_000,
                looping: true,
            })
            .unwrap()
        else {
            panic!("audio")
        };
        editor
            .execute(Command::SetClipSourcePhase {
                clip: audio,
                phase: 701,
            })
            .unwrap();
        editor
            .execute(Command::SetClipFades {
                clip: audio,
                fade_in: 1000,
                fade_out: 2000,
            })
            .unwrap();
        let Created::Clip(midi) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beat(4),
                length: beat(2),
            })
            .unwrap()
        else {
            panic!("midi")
        };
        editor
            .execute(Command::SetMidiClipSource {
                clip: midi,
                source_length: beat(4),
                source_start: beat(1),
                loop_start: beat(1),
                loop_end: beat(3),
                looping: true,
                length: beat(2),
            })
            .unwrap();
        for start in [BeatTime::ZERO, beat(1), beat(3)] {
            editor
                .execute(Command::AddMidiNote {
                    clip: midi,
                    start,
                    duration: beat(1),
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
        let Created::Track(target) = editor
            .execute(Command::AddTrack {
                name: "target".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(gap) = editor
            .execute(Command::PlaceClip {
                track: target,
                asset,
                start: 300_000,
                duration: 12_000,
                source_offset: 0,
                source_length: 12_000,
                looping: false,
            })
            .unwrap()
        else {
            panic!("gap")
        };
        (editor, track, target, audio, midi, gap)
    }
    fn equal_audio_content(original: &Clip, copy: &Clip) {
        assert_ne!(original.id(), copy.id());
        let mut normalized = copy.clone();
        normalized.id = original.id;
        normalized.start = original.start;
        assert_eq!(&normalized, original);
    }
    fn equal_midi_content(original: &MidiClip, copy: &MidiClip) {
        assert_ne!(original.id(), copy.id());
        let mut normalized = copy.clone();
        normalized.id = original.id;
        normalized.start = original.start;
        let source = Arc::make_mut(&mut normalized.source);
        assert_eq!(source.notes.len(), original.notes().len());
        for (note, old) in source.notes.iter_mut().zip(original.notes()) {
            assert_ne!(note.id(), old.id());
            note.id = old.id();
        }
        assert_eq!(&normalized, original);
    }
    #[test]
    fn whole_copy_survives_source_removal_preserves_domains_gaps_fresh_ids_and_one_undo() {
        let (mut editor, _, target, audio, midi, gap) = fixture();
        let original_audio = editor.project().clip(audio).unwrap().clone();
        let original_midi = editor.project().midi_clip(midi).unwrap().clone();
        let gap_clip = editor.project().clip(gap).unwrap().clone();
        assert!(editor.project().capture_clips(&[]).is_err());
        assert!(editor.project().capture_clips(&[audio, audio]).is_err());
        let clipboard = Arc::new(editor.project().capture_clips(&[midi, audio]).unwrap());
        editor.execute(Command::RemoveClip(audio)).unwrap();
        editor.execute(Command::RemoveMidiClip(midi)).unwrap();
        let mut before = editor.project().snapshot();
        editor
            .execute(Command::PasteClips {
                clipboard: Arc::clone(&clipboard),
                track: target,
                frame: 240_000,
                insert: false,
            })
            .unwrap();
        let destination = editor.project().track(target).unwrap();
        let copy_audio = destination
            .clips()
            .iter()
            .find(|c| c.start() == 240_000)
            .unwrap();
        let copy_midi = destination
            .midi_clips()
            .iter()
            .find(|c| c.start() == beat(14))
            .unwrap();
        equal_audio_content(&original_audio, copy_audio);
        equal_midi_content(&original_midi, copy_midi);
        assert_eq!(
            editor.project().clip(gap),
            Some(&gap_clip),
            "whole clips retain empty gaps"
        );
        let first_audio = copy_audio.id();
        let first_notes = copy_midi
            .notes()
            .iter()
            .map(|n| n.id())
            .collect::<BTreeSet<NoteId>>();
        let after = editor.project().snapshot();
        assert!(editor.undo());
        before.next_id = after.next_id;
        assert_eq!(editor.project().snapshot(), before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
        editor
            .execute(Command::PasteClips {
                clipboard,
                track: target,
                frame: 480_000,
                insert: false,
            })
            .unwrap();
        let destination = editor.project().track(target).unwrap();
        let second_audio = destination
            .clips()
            .iter()
            .find(|c| c.start() == 480_000)
            .unwrap();
        let second_midi = destination
            .midi_clips()
            .iter()
            .find(|c| c.start() == beat(24))
            .unwrap();
        assert_ne!(second_audio.id(), first_audio);
        assert!(
            second_midi
                .notes()
                .iter()
                .all(|n| !first_notes.contains(&n.id()))
        );
        equal_audio_content(&original_audio, second_audio);
        equal_midi_content(&original_midi, second_midi);
    }
    #[test]
    fn overlapping_whole_members_survive_without_erasing_each_other() {
        let (editor, _, target, audio, midi, _) = fixture();
        let mut snapshot = editor.project().snapshot();
        for track in &mut snapshot.tracks {
            for clip in &mut track.content.midi_clips {
                if clip.id == midi.value() {
                    clip.start_raw = beat(1).raw();
                }
            }
        }
        let project = Project::from_snapshot(snapshot).unwrap();
        let original_audio = project.clip(audio).unwrap().clone();
        let original_midi = project.midi_clip(midi).unwrap().clone();
        let clipboard = Arc::new(project.capture_clips(&[audio, midi]).unwrap());
        let mut editor = Editor::new(project, 8).unwrap();
        editor
            .execute(Command::PasteClips {
                clipboard,
                track: target,
                frame: 240_000,
                insert: false,
            })
            .unwrap();
        let destination = editor.project().track(target).unwrap();
        let a = destination
            .clips()
            .iter()
            .find(|c| c.start() == 240_000)
            .unwrap();
        let m = destination
            .midi_clips()
            .iter()
            .find(|c| c.start() == beat(11))
            .unwrap();
        equal_audio_content(&original_audio, a);
        equal_midi_content(&original_midi, m);
    }

    #[test]
    fn multi_track_normal_and_ripple_keep_original_track_ids_and_refuse_overflow_atomically() {
        for insert in [false, true] {
            let (mut editor, source, target, audio, midi, _) = fixture();
            let Created::Track(other) = editor
                .execute(Command::AddTrack {
                    name: "other".into(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            editor
                .execute(Command::MoveMidiClip {
                    clip: midi,
                    track: other,
                    start: beat(4),
                })
                .unwrap();
            let clipboard = Arc::new(editor.project().capture_clips(&[audio, midi]).unwrap());
            let target_before = editor.project().track(target).unwrap().clone();
            let mut before = editor.project().snapshot();
            editor
                .execute(Command::PasteClips {
                    clipboard: Arc::clone(&clipboard),
                    track: target,
                    frame: if insert { 0 } else { 240_000 },
                    insert,
                })
                .unwrap();
            assert_eq!(editor.project().track(target), Some(&target_before));
            assert_eq!(editor.project().track(source).unwrap().clips().len(), 2);
            assert_eq!(editor.project().track(other).unwrap().midi_clips().len(), 2);
            if insert {
                assert_eq!(editor.project().clip(audio).unwrap().start(), 144_000);
                assert_eq!(editor.project().midi_clip(midi).unwrap().start(), beat(10));
            }
            let after = editor.project().snapshot();
            assert!(
                editor
                    .execute(Command::PasteClips {
                        clipboard,
                        track: target,
                        frame: u64::MAX,
                        insert
                    })
                    .is_err()
            );
            assert_eq!(editor.project().snapshot(), after);
            assert!(editor.undo());
            before.next_id = after.next_id;
            assert_eq!(editor.project().snapshot(), before);
            assert!(editor.redo());
            assert_eq!(editor.project().snapshot(), after);
        }
    }
}
