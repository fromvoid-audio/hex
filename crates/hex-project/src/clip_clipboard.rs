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
    /// Distance from the selection origin in the member's own time domain:
    /// frames for audio, raw beats for MIDI. MIDI spacing therefore survives
    /// tempo changes and never takes a whole-frame round trip.
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
        let mut origin_beat = BeatTime::from_raw(u64::MAX);
        for track in self.tracks() {
            let before = clipboard.clips.len();
            for clip in track.clips().iter().filter(|c| selected.contains(&c.id())) {
                origin = origin.min(clip.start());
                origin_beat = origin_beat.min(self.tempo_map().frame_to_beat(clip.start())?);
                for id in clip.referenced_assets() {
                    if !clipboard.assets.iter().any(|(asset, _)| *asset == id) {
                        let asset = self
                            .asset(id)
                            .cloned()
                            .ok_or(Invalid("copied asset no longer exists"))?;
                        clipboard.assets.push((id, asset));
                    }
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
                origin = origin.min(self.tempo_map().beat_to_frame(clip.start())?);
                origin_beat = origin_beat.min(clip.start());
                clipboard.clips.push(Captured {
                    track: track.id(),
                    offset: clip.start().raw(),
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
            clip.offset -= match clip.content {
                Content::Audio(_) => origin,
                Content::Midi(_) => origin_beat.raw(),
            };
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
        // Convert the paste anchor once; each member then offsets in its domain.
        let anchor_beat = project.tempo_map().frame_to_beat(frame)?;
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
            let (beat, start, end) = match &original.content {
                Content::Audio(clip) => {
                    let start = frame
                        .checked_add(original.offset)
                        .ok_or(Invalid("paste clip start overflow"))?;
                    let mut projected = clip.clone();
                    crate::audio_edit::move_clip(&mut projected, start, project.tempo_map())?;
                    let end = start
                        .checked_add(projected.duration())
                        .ok_or(Invalid("paste clip end overflow"))?;
                    (BeatTime::ZERO, start, end)
                }
                Content::Midi(clip) => {
                    let beat = anchor_beat
                        .raw()
                        .checked_add(original.offset)
                        .ok_or(Invalid("paste clip start overflow"))?;
                    let end = beat
                        .checked_add(clip.length().raw())
                        .ok_or(Invalid("paste midi end overflow"))?;
                    let beat = BeatTime::from_raw(beat);
                    (
                        beat,
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
    // Plan all members together; gaps do not replace destination content.
    let mut incoming = Vec::new();
    incoming
        .try_reserve_exact(prepared.len())
        .map_err(|_| Invalid("could not reserve clipboard placement"))?;
    for copy in prepared {
        let content = match copy.content {
            Content::Audio(mut clip) => {
                crate::audio_edit::move_clip(&mut clip, copy.frame, project.tempo_map())?;
                crate::ClipGroupContent::Audio(clip)
            }
            Content::Midi(mut clip) => {
                clip.start = copy.beat;
                crate::ClipGroupContent::Midi(clip)
            }
        };
        incoming.push(crate::ClipGroupPreview {
            track: copy.track,
            content,
        });
    }
    let created = crate::clip_group::place_batch(project, incoming, &[])?;
    Project::from_snapshot(project.snapshot())?;
    // `first..=last` names only pasted clips: overwrite fragments (and ripple
    // splits from `insert`) take their IDs before the first pasted clip, and
    // the pasted MIDI note IDs inside the range are not clip IDs.
    Ok(match created {
        Created::Clips { first, last } if first == last => Created::Clip(first),
        other => other,
    })
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
    fn disjoint_whole_members_preserve_their_gap_and_source_content() {
        let (editor, _, target, audio, midi, _) = fixture();
        let project = editor.project().clone();
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
            .find(|c| c.start() == beat(14))
            .unwrap();
        equal_audio_content(&original_audio, a);
        equal_midi_content(&original_midi, m);
    }

    /// Copy an audio clip at frame 0 and MIDI clips at `starts`, then paste
    /// at the frame of beat 16; returns the anchor beat and pasted MIDI starts.
    fn paste_midi_spacing(tempo_change: bool, starts: &[u64]) -> (BeatTime, Vec<BeatTime>) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: crate::Tempo::new(117_000_000).unwrap(),
            })
            .unwrap();
        if tempo_change {
            editor
                .execute(Command::SetTempoPoint {
                    beat: beat(18),
                    tempo: crate::Tempo::new(61_000_000).unwrap(),
                })
                .unwrap();
        }
        let Created::Imported { track, clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "clip.wav".into(),
                sample_rate: 48_000,
                frames: 4_800,
            })
            .unwrap()
        else {
            panic!("import")
        };
        let mut ids = vec![clip];
        for start in starts {
            let Created::Clip(midi) = editor
                .execute(Command::PlaceMidiClip {
                    track,
                    start: beat(*start),
                    length: beat(1),
                })
                .unwrap()
            else {
                panic!("midi")
            };
            ids.push(midi);
        }
        let clipboard = Arc::new(editor.project().capture_clips(&ids).unwrap());
        let Created::Track(target) = editor
            .execute(Command::AddTrack {
                name: "target".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let frame = editor
            .project()
            .tempo_map()
            .beat_to_frame(beat(16))
            .unwrap();
        editor
            .execute(Command::PasteClips {
                clipboard,
                track: target,
                frame,
                insert: false,
            })
            .unwrap();
        let project = editor.project();
        let anchor = project.tempo_map().frame_to_beat(frame).unwrap();
        let destination = project.track(target).unwrap();
        assert_eq!(destination.clips()[0].start(), frame);
        let pasted = destination.midi_clips().iter().map(|c| c.start()).collect();
        (anchor, pasted)
    }

    #[test]
    fn pasted_midi_keeps_exact_beat_offsets_at_non_integer_frame_tempo() {
        let (anchor, pasted) = paste_midi_spacing(false, &[4]);
        assert_eq!(pasted, [BeatTime::from_raw(anchor.raw() + beat(4).raw())]);
    }

    #[test]
    fn pasted_midi_keeps_beat_spacing_across_a_tempo_change() {
        let (anchor, pasted) = paste_midi_spacing(true, &[1, 5]);
        let expected = [1, 5].map(|b| BeatTime::from_raw(anchor.raw() + beat(b).raw()));
        assert_eq!(pasted, expected);
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

    fn clips_in(project: &Project, first: ClipId, last: ClipId) -> BTreeSet<ClipId> {
        project
            .tracks()
            .iter()
            .flat_map(|t| {
                t.clips()
                    .iter()
                    .map(|c| c.id())
                    .chain(t.midi_clips().iter().map(|c| c.id()))
            })
            .filter(|id| *id >= first && *id <= last)
            .collect()
    }
    #[test]
    fn paste_reports_exactly_the_pasted_clips_and_never_overwrite_fragments() {
        let (mut editor, _, target, audio, midi, gap) = fixture();
        let several = Arc::new(editor.project().capture_clips(&[audio, midi]).unwrap());
        let Created::Clips { first, last } = editor
            .execute(Command::PasteClips {
                clipboard: several,
                track: target,
                frame: 240_000,
                insert: false,
            })
            .unwrap()
        else {
            panic!("several pasted clips report a clip range")
        };
        let destination = editor.project().track(target).unwrap();
        let pasted = BTreeSet::from([
            destination
                .clips()
                .iter()
                .find(|c| c.start() == 240_000)
                .unwrap()
                .id(),
            destination
                .midi_clips()
                .iter()
                .find(|c| c.start() == beat(14))
                .unwrap()
                .id(),
        ]);
        assert_eq!(
            clips_in(editor.project(), first, last),
            pasted,
            "pasted note IDs inside the range are not clips"
        );

        // A ripple insert inside the gap clip splits it; that fragment is
        // born before the pasted clip and stays outside the reported result.
        let one = Arc::new(editor.project().capture_clips(&[audio]).unwrap());
        let before: BTreeSet<ClipId> = editor
            .project()
            .track(target)
            .unwrap()
            .clips()
            .iter()
            .map(|c| c.id())
            .collect();
        let Created::Clip(clip) = editor
            .execute(Command::PasteClips {
                clipboard: one,
                track: target,
                frame: 306_000,
                insert: true,
            })
            .unwrap()
        else {
            panic!("one pasted clip reports Created::Clip")
        };
        let destination = editor.project().track(target).unwrap();
        assert_eq!(
            destination
                .clips()
                .iter()
                .find(|c| c.start() == 306_000)
                .map(|c| c.id()),
            Some(clip)
        );
        let fragments: Vec<ClipId> = destination
            .clips()
            .iter()
            .map(|c| c.id())
            .filter(|id| !before.contains(id) && *id != clip)
            .collect();
        assert!(!fragments.is_empty(), "insert split the gap clip");
        assert!(fragments.iter().all(|id| *id < clip));
        assert!(editor.project().clip(gap).is_some());
        assert_eq!(
            clips_in(editor.project(), clip, clip),
            BTreeSet::from([clip])
        );
    }
}
