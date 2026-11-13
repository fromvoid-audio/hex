//! Atomic Arrangement content reflection. Only finite read metadata changes;
//! source media, source notes, processing settings, placement and IDs are retained.
use crate::{
    AudioClipEdit, AudioContentView, BeatTime, ClipId, ContentWindow, Created, Invalid, MAX_CLIPS,
    Project,
};
use std::{collections::BTreeSet, sync::Arc};

pub(crate) fn apply(project: &mut Project, ids: Vec<ClipId>) -> Result<Created, Invalid> {
    if ids.is_empty() || ids.len() > MAX_CLIPS {
        return Err(Invalid(
            "reverse requires a bounded arrangement clip selection",
        ));
    }
    let selected = ids.iter().copied().collect::<BTreeSet<_>>();
    if selected.len() != ids.len() {
        return Err(Invalid("reverse selection contains duplicate clips"));
    }
    let found = project
        .tracks
        .iter()
        .map(|t| {
            t.clips()
                .iter()
                .filter(|c| selected.contains(&c.id()))
                .count()
                + t.midi_clips()
                    .iter()
                    .filter(|c| selected.contains(&c.id()))
                    .count()
        })
        .sum::<usize>();
    if found != ids.len() {
        return Err(Invalid("selected arrangement clip no longer exists"));
    }
    // Project's transaction owns rollback for any subsequent validation/admission refusal.
    for track in &mut project.tracks {
        for clip in &mut track.content.audio_clips {
            if !selected.contains(&clip.id) {
                continue;
            }
            let plain = clip.edit.is_none();
            let mut edit = clip
                .audio_edit()
                .cloned()
                .unwrap_or_else(|| AudioClipEdit::capture(clip));
            if let Some(mut view) = edit.content_view {
                view.window.reversed = !view.window.reversed;
                let identity = !view.window.reversed
                    && view.base_start_delta == 0
                    && view.window.start == 0
                    && view.window.end == view.base_length()?
                    && view.base_view_start == edit.view_start
                    && view.base_view_end == edit.view_end
                    && view.base_placement == edit.placement;
                edit.content_view = (!identity).then_some(view);
                if identity && view.restore_plain && edit == AudioClipEdit::capture(clip) {
                    clip.edit = None;
                } else {
                    clip.edit = Some(Arc::new(edit));
                }
            } else {
                let length = edit
                    .placement
                    .map_or(edit.view_end - edit.view_start, |p| p.outer_length.length());
                edit.content_view = Some(AudioContentView {
                    restore_plain: plain,
                    base_view_start: edit.view_start,
                    base_view_end: edit.view_end,
                    base_placement: edit.placement,
                    base_start_delta: 0,
                    window: ContentWindow::new(0, length, true)?,
                    read_resampler: crate::WarpResampler::default(),
                });
                clip.edit = Some(Arc::new(edit));
            }
        }
        for clip in &mut track.content.midi_clips {
            if !selected.contains(&clip.id) {
                continue;
            }
            let end = clip
                .playback_offset
                .raw()
                .checked_add(clip.length.raw())
                .ok_or(Invalid("midi reverse window overflow"))?;
            let mut view = clip.content_view.unwrap_or(ContentWindow::new(
                clip.playback_offset.raw(),
                end,
                false,
            )?);
            view.reversed = !view.reversed;
            clip.content_view =
                (!(!view.reversed && view.start == clip.playback_offset.raw() && view.end == end))
                    .then_some(view);
            clip.source_playback()?;
        }
    }
    Project::from_snapshot(project.snapshot())?;
    Ok(Created::None)
}

pub(crate) fn split_midi(
    project: &mut Project,
    track: crate::TrackId,
    original: crate::MidiClip,
    beat: BeatTime,
) -> Result<Created, Invalid> {
    let cut = beat
        .raw()
        .checked_sub(original.start().raw())
        .filter(|n| *n > 0 && *n < original.length().raw())
        .ok_or(Invalid("split cursor must be inside the midi clip"))?;
    if project
        .tracks()
        .iter()
        .map(|t| t.clips().len() + t.midi_clips().len())
        .sum::<usize>()
        >= MAX_CLIPS
    {
        return Err(Invalid("clip limit reached"));
    }
    if crate::midi::note_count(project)
        .checked_add(original.notes().len())
        .is_none_or(|n| n > crate::MAX_NOTES)
    {
        return Err(Invalid("midi note limit reached"));
    }
    let window = original
        .content_view()
        .ok_or(Invalid("directed midi split requires a content view"))?;
    let mut left = original.clone();
    left.length = BeatTime::from_raw(cut);
    left.content_view = Some(window.cropped(0..cut)?);
    let mut right = original.clone();
    right.start = beat;
    right.length = BeatTime::from_raw(original.length().raw() - cut);
    right.content_view = Some(window.cropped(cut..original.length().raw())?);
    right.id = ClipId(project.allocate_id()?);
    for note in &mut Arc::make_mut(&mut right.source).notes {
        note.id = crate::NoteId(project.allocate_id()?);
    }
    let id = right.id;
    crate::clip_group::replace_midi(project, left)?;
    crate::clip_group::place_one(project, track, crate::ClipGroupContent::Midi(right), false)?;
    Ok(Created::Clip(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioDomainClock, Command, Editor, WarpResampler};

    #[test]
    fn plain_reversed_source_reports_exact_extent_and_rejects_processing() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 8_000,
            })
            .unwrap()
        else {
            panic!("audio fixture")
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: crate::TrimEdge::Left,
                frame: 1_000,
            })
            .unwrap();
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let clip = editor.project().clip(clip).unwrap();
        let edit = clip.audio_edit().unwrap();
        assert_eq!(
            edit.plain_reversed_source(clip),
            Some((clip.source_offset(), clip.duration()))
        );

        let mut pitch = edit.clone();
        Arc::make_mut(&mut pitch.domain).pitch_cents = 100;
        assert_eq!(pitch.plain_reversed_source(clip), None);
        let mut looping = edit.clone();
        Arc::make_mut(&mut looping.domain).looping = true;
        assert_eq!(looping.plain_reversed_source(clip), None);
        let mut crop = edit.clone();
        crop.view_start += 1;
        assert_eq!(crop.plain_reversed_source(clip), None);
        let mut forward = edit.clone();
        forward.content_view.as_mut().unwrap().window.reversed = false;
        assert_eq!(forward.plain_reversed_source(clip), None);
    }
    #[test]
    fn musical_unwarped_reverse_projects_the_retained_source_cycle_edges() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "loop".into(),
                path: "loop.wav".into(),
                sample_rate: 48_000,
                frames: 8_000,
            })
            .unwrap()
        else {
            panic!("fixture")
        };
        for command in [
            Command::SetClipLooping {
                clip,
                looping: true,
            },
            Command::SetClipSourcePhase { clip, phase: 2_000 },
            Command::TrimClipEdge {
                clip,
                edge: crate::TrimEdge::Right,
                frame: 18_000,
            },
            Command::SetAudioClipMusical {
                clip,
                enabled: true,
            },
            Command::ReverseClips(vec![clip]),
        ] {
            editor.execute(command).unwrap();
        }
        let project = editor.project();
        let clip = project.clip(clip).unwrap();
        assert_eq!((clip.start(), clip.duration()), (0, 18_000));
        let projection = clip.repeat_cycles(project.tempo_map()).unwrap().unwrap();
        let plan = projection.cycles();
        let frame = |elapsed| projection.frame_at_elapsed(elapsed).unwrap();
        let seams = plan.seams().unwrap();
        assert_eq!(plan.count(), 3);
        assert_eq!(frame(i128::from(seams.boundary(0).unwrap())), 4_000);
        assert_eq!(frame(i128::from(seams.boundary(1).unwrap())), 12_000);
        let (first, last) = plan.full_bounds(0).unwrap();
        assert_eq!((frame(first), frame(last)), (-4_000, 4_000));
    }

    #[test]
    fn reverse_twice_preserves_exact_plain_and_edited_audio_snapshots_and_undo() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "Original".into(),
                path: "Original.wav".into(),
                sample_rate: 48_000,
                frames: 8_000,
            })
            .unwrap()
        else {
            panic!("fixture")
        };
        for edited in [false, true] {
            if edited {
                editor
                    .execute(Command::SetAudioClipPitch { clip, cents: 50 })
                    .unwrap();
            }
            let before = editor.project().snapshot();
            editor.execute(Command::ReverseClips(vec![clip])).unwrap();
            let reversed = editor.project().snapshot();
            assert_ne!(before, reversed);
            editor.execute(Command::ReverseClips(vec![clip])).unwrap();
            assert_eq!(editor.project().snapshot(), before);
            assert!(editor.undo());
            assert_eq!(editor.project().snapshot(), reversed);
            assert!(editor.redo());
            assert_eq!(editor.project().snapshot(), before);
        }
        let before = editor.project().snapshot();
        assert!(
            editor
                .execute(Command::ReverseClips(vec![clip, clip]))
                .is_err()
        );
        assert_eq!(editor.project().snapshot(), before);
    }
    #[test]
    fn new_views_read_with_v2_and_edits_keep_a_migrated_v1_law() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "musical".into(),
                path: "musical.wav".into(),
                sample_rate: 48_000,
                frames: 48_000,
            })
            .unwrap()
        else {
            panic!("fixture")
        };
        editor
            .execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })
            .unwrap();
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let edit = editor.project().clip(clip).unwrap().audio_edit().unwrap();
        let view = edit.content_view.unwrap();
        assert!(matches!(edit.domain.clock, AudioDomainClock::Beats(_)));
        assert_eq!(view.read_resampler, WarpResampler::V2);
        assert!(view.has_explicit_read_resampler(edit.domain.clock));
        assert!(!view.has_explicit_read_resampler(AudioDomainClock::Frames(1)));
        // A saved musical view migrates to v1; later direction and trim edits
        // keep that law rather than silently changing its rendered PCM.
        let mut snapshot = editor.project().snapshot();
        let crate::TrackContentSnapshot::Hybrid { audio_clips, .. } =
            &mut snapshot.tracks[0].content
        else {
            panic!("hybrid track")
        };
        let migrated = audio_clips[0].edit.as_mut().unwrap();
        let clock = migrated.domain.clock;
        let view = migrated.content_view.as_mut().unwrap();
        view.read_resampler = AudioContentView::implied_read_resampler(clock);
        view.window = ContentWindow::new(view.window.start, view.window.end, false).unwrap();
        let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let start = editor.project().clip(clip).unwrap().start();
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: crate::TrimEdge::Right,
                frame: start + 12_000,
            })
            .unwrap();
        let view = editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .content_view
            .unwrap();
        assert!(view.window.reversed);
        assert_eq!(view.read_resampler, WarpResampler::V1);
    }
    #[test]
    fn reverse_midi_trim_split_and_transfer_refusal_preserve_editable_source() {
        let b = |n| BeatTime::from_quarters(n).unwrap();
        let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
        let Created::Track(track) = e
            .execute(Command::AddMidiTrack {
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
                length: b(2),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        e.execute(Command::SetMidiClipSource {
            clip,
            source_length: b(8),
            source_start: b(2),
            loop_start: b(0),
            loop_end: b(8),
            looping: false,
            length: b(2),
        })
        .unwrap();
        e.execute(Command::AddMidiNote {
            clip,
            start: b(1),
            duration: b(3),
            pitch: 60,
            channel: 2,
            velocity: 99,
        })
        .unwrap();
        let original = e.project().snapshot();
        e.execute(Command::ReverseClips(vec![clip])).unwrap();
        let source = e.project().midi_clip(clip).unwrap().source_identity();
        e.execute(Command::TrimMidiClipEdge {
            clip,
            edge: crate::TrimEdge::Right,
            beat: b(3),
        })
        .unwrap();
        let c = e.project().midi_clip(clip).unwrap();
        assert_eq!(c.notes()[0].start(), b(1));
        assert_eq!(c.source_start(), b(1));
        let tuples = |c: &crate::MidiClip| {
            c.source_playback()
                .unwrap()
                .note_occurrences(c.notes()[0].start(), c.notes()[0].duration())
                .unwrap()
                .iter()
                .map(|o| (o.timeline_start(), o.gate_duration()))
                .collect::<Vec<_>>()
        };
        assert_eq!(tuples(c), vec![(b(0), b(3))]);
        assert_ne!(
            c.source_identity(),
            source,
            "explicit source-start rewind uses copy-on-write source metadata"
        );
        let Created::Scene(scene) = e
            .execute(Command::AddScene {
                name: "scene".into(),
            })
            .unwrap()
        else {
            panic!("scene")
        };
        let before = e.project().snapshot();
        for target in [
            crate::SessionSceneTarget::Existing(scene),
            crate::SessionSceneTarget::Insert { before: None },
        ] {
            assert!(
                e.execute(Command::CopyArrangementMidiToNewSessionTrack {
                    initial_gain: None,
                    source: clip,
                    before_track: None,
                    scene: target,
                })
                .is_err(),
                "a new console track must not discard directed content"
            );
            assert_eq!(
                e.project().snapshot(),
                before,
                "refusal preserves tracks, scenes and next identity"
            );
        }
        assert!(
            e.execute(Command::CopyArrangementMidiToSession {
                source: clip,
                scene
            })
            .is_err()
        );
        assert_eq!(e.project().snapshot(), before);
        e.execute(Command::SplitMidiClip { clip, beat: b(1) })
            .unwrap();
        let clips = e.project().track(track).unwrap().midi_clips();
        assert_eq!(clips.len(), 2);
        assert_eq!(tuples(&clips[0]), vec![(b(0), b(1))]);
        assert_eq!(tuples(&clips[1]), vec![(b(0), b(2))]);
        assert_eq!(clips[0].source_start(), b(1));
        assert_eq!(clips[1].source_start(), b(1));
        assert_eq!(original.tracks[0].id, track.value());
    }
}
