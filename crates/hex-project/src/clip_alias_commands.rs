//! Alias commands run only inside the Editor's private preparation candidate.
use super::clip_alias::Content;
use crate::{
    AliasSourceId, AudioClipEdit, ClipId, Command, Created, Invalid, Project, SceneId,
    SessionCellContent, TrackId,
};
use std::{collections::BTreeSet, sync::Arc};

pub(crate) fn source(project: &mut Project, clip: ClipId) -> Result<AliasSourceId, Invalid> {
    if let Some(id) = project.clip_alias(clip) {
        return Ok(id);
    }
    let content = if let Some(midi) = project.midi_source(clip) {
        Content::Midi(Arc::clone(&midi.content))
    } else if let Some(audio) = project.clip(clip).cloned() {
        let edit = audio
            .edit
            .clone()
            .unwrap_or_else(|| Arc::new(AudioClipEdit::capture(&audio)));
        if matches!(edit.domain.clock, crate::AudioDomainClock::Frames(length)
            if length > crate::MAX_AUDIO_DOMAIN_FRAMES)
        {
            return Err(Invalid(
                "linked audio exceeds the supported frame event extent",
            ));
        }
        let content = Content::Audio {
            asset: audio.asset,
            content: Arc::clone(&edit.domain.content),
        };
        project.clip_mut(clip)?.edit = Some(edit);
        content
    } else {
        let audio = project
            .session
            .cells
            .iter()
            .find_map(|cell| match &cell.content {
                SessionCellContent::Audio(audio) if audio.id == clip => Some(audio),
                _ => None,
            })
            .ok_or(Invalid("alias clip no longer exists"))?;
        Content::ConsoleAudio(audio.asset)
    };
    let id = AliasSourceId(project.allocate_id()?);
    project.aliases.sources.insert(id, content);
    project.aliases.members.insert(clip, id);
    Ok(id)
}
pub(crate) fn bind_copy(
    project: &mut Project,
    original: ClipId,
    copied: ClipId,
) -> Result<(), Invalid> {
    let id = project
        .clip_alias(original)
        .ok_or(Invalid("alias source was not captured"))?;
    let content = project
        .aliases
        .sources
        .get(&id)
        .cloned()
        .ok_or(Invalid("alias source missing"))?;
    match content {
        Content::Midi(content) => project.midi_source_mut(copied)?.content = content,
        Content::Audio { asset, content } => {
            let clip = project.clip_mut(copied)?;
            if clip.asset != asset {
                return Err(Invalid("alias copied media differs"));
            }
            let edit = clip
                .audio_edit_mut()
                .ok_or(Invalid("alias copied root missing"))?;
            Arc::make_mut(&mut edit.domain).content = content;
        }
        Content::ConsoleAudio(_) => (),
    }
    project.aliases.members.insert(copied, id);
    Ok(())
}
pub(crate) fn prune(project: &mut Project) {
    if project.aliases.members.is_empty() {
        project.aliases.sources.clear();
        return;
    }
    let live: BTreeSet<_> = project
        .tracks
        .iter()
        .flat_map(|track| {
            track
                .clips()
                .iter()
                .map(|clip| clip.id)
                .chain(track.midi_clips().iter().map(|clip| clip.id))
        })
        .chain(
            project
                .session
                .cells
                .iter()
                .filter_map(|cell| match &cell.content {
                    SessionCellContent::Audio(clip) => Some(clip.id),
                    SessionCellContent::Midi(clip) => Some(clip.id),
                    SessionCellContent::Stop => None,
                }),
        )
        .collect();
    project
        .aliases
        .members
        .retain(|clip, _| live.contains(clip));
    let used: BTreeSet<_> = project.aliases.members.values().copied().collect();
    project.aliases.sources.retain(|id, _| used.contains(id));
}
pub(crate) fn unique(project: &mut Project, clips: Vec<ClipId>) -> Result<Created, Invalid> {
    if clips.is_empty()
        || clips.len() > crate::MAX_CLIPS
        || clips.iter().copied().collect::<BTreeSet<_>>().len() != clips.len()
    {
        return Err(Invalid("invalid make unique selection"));
    }
    let selected: BTreeSet<_> = clips.iter().copied().collect();
    let mut final_notes = crate::midi::note_count(project);
    for (source, content) in &project.aliases.sources {
        let Content::Midi(content) = content else {
            continue;
        };
        let total = project
            .aliases
            .members
            .values()
            .filter(|id| **id == *source)
            .count();
        let detached = project
            .aliases
            .members
            .iter()
            .filter(|(clip, id)| **id == *source && selected.contains(clip))
            .count();
        if detached == 0 {
            continue;
        }
        let extra_sources = detached + usize::from(detached < total) - 1;
        let extra_notes = extra_sources
            .checked_mul(content.notes.len())
            .ok_or(Invalid("make unique note count overflow"))?;
        final_notes = final_notes
            .checked_add(extra_notes)
            .filter(|count| *count <= crate::MAX_NOTES)
            .ok_or(Invalid("make unique exceeds midi note limit"))?;
    }
    for clip in clips {
        if project.midi_source(clip).is_none() && project.clip(clip).is_none() && project.session.cells.iter().all(|cell| !matches!(&cell.content, SessionCellContent::Audio(audio) if audio.id == clip)) { return Err(Invalid("selected clip no longer exists")); }
        if project.aliases.members.remove(&clip).is_none() {
            continue;
        }
        if let Some(source) = project.midi_source(clip) {
            let mut content = source.content.as_ref().clone();
            for note in &mut content.notes {
                note.id = crate::NoteId(project.allocate_id()?);
            }
            project.midi_source_mut(clip)?.content = Arc::new(content);
        }
    }
    prune(project);
    Ok(Created::None)
}
pub(crate) fn duplicate_console(
    project: &mut Project,
    track: TrackId,
    scene: SceneId,
    alias: bool,
) -> Result<Created, Invalid> {
    let original = project
        .session
        .cells
        .iter()
        .find(|cell| cell.track == track && cell.scene == scene)
        .map(|cell| cell.content.clone())
        .ok_or(Invalid("console cell is empty"))?;
    let clip = match &original {
        SessionCellContent::Audio(clip) => clip.id,
        SessionCellContent::Midi(clip) => clip.id,
        SessionCellContent::Stop => return Err(Invalid("cannot duplicate a stop cell")),
    };
    let index = project
        .session
        .scenes
        .iter()
        .position(|item| item.id == scene)
        .ok_or(Invalid("scene no longer exists"))?;
    let destination = project
        .session
        .scenes
        .iter()
        .skip(index + 1)
        .find(|item| project.session_cell(track, item.id).is_none())
        .map(|item| item.id);
    let destination = match destination {
        Some(scene) => scene,
        None => match project.apply(Command::AddScene {
            name: "scene".into(),
        })? {
            Created::Scene(scene) => scene,
            _ => return Err(Invalid("scene creation returned no scene")),
        },
    };
    if !alias {
        return project.apply(match original {
            SessionCellContent::Midi(_) => Command::DuplicateSessionMidiCell {
                track,
                from_scene: scene,
                to_scene: destination,
            },
            SessionCellContent::Audio(_) => Command::DuplicateSessionAudioCell {
                track,
                from_scene: scene,
                to_scene: destination,
            },
            SessionCellContent::Stop => return Err(Invalid("cannot duplicate a stop cell")),
        });
    }
    source(project, clip)?;
    if crate::midi::clip_count(project) >= crate::MAX_CLIPS
        || project.session.cells.len() >= crate::MAX_SESSION_CELLS
    {
        return Err(Invalid("console clip limit reached"));
    }
    project
        .session
        .cells
        .try_reserve(1)
        .map_err(|_| Invalid("could not reserve alias cell"))?;
    let id = ClipId(project.allocate_id()?);
    let content = match original {
        SessionCellContent::Midi(mut clip) => {
            clip.id = id;
            SessionCellContent::Midi(clip)
        }
        SessionCellContent::Audio(mut clip) => {
            clip.id = id;
            SessionCellContent::Audio(clip)
        }
        SessionCellContent::Stop => return Err(Invalid("cannot duplicate a stop cell")),
    };
    project.session.cells.push(crate::SessionCell {
        track,
        scene: destination,
        content,
    });
    bind_copy(project, clip, id)?;
    Ok(Created::SessionClip(id))
}
pub(crate) fn apply_settings(project: &mut Project, picked: ClipId) -> Result<Created, Invalid> {
    let group = project
        .clip_alias(picked)
        .ok_or(Invalid("clip is not an alias"))?;
    let members: Vec<_> = project
        .aliases
        .members
        .iter()
        .filter_map(|(clip, source)| (*source == group && *clip != picked).then_some(*clip))
        .collect();
    if let Some(original) = project.clip(picked).cloned() {
        for id in members {
            let mut replacement = original.clone();
            let target = project
                .clip(id)
                .ok_or(Invalid("alias audio member missing"))?;
            replacement.id = id;
            crate::audio_edit::move_clip(&mut replacement, target.start, project.tempo_map())?;
            *project.clip_mut(id)? = replacement;
        }
    } else if let Some(original) = project.midi_clip(picked).cloned() {
        for id in members {
            if let Some(target) = project.midi_clip(id) {
                let mut replacement = original.clone();
                replacement.id = id;
                replacement.start = target.start;
                *project
                    .midi_clips_mut(id)?
                    .iter_mut()
                    .find(|clip| clip.id == id)
                    .ok_or(Invalid("alias midi member missing"))? = replacement;
            } else {
                let target = project
                    .session
                    .cells
                    .iter_mut()
                    .find_map(|cell| match &mut cell.content {
                        SessionCellContent::Midi(clip) if clip.id == id => Some(clip),
                        _ => None,
                    })
                    .ok_or(Invalid("alias midi cell missing"))?;
                target.source = Arc::clone(&original.source);
                target.active = original.active;
                target.name.clone_from(&original.name);
                target.flavor = original.flavor;
                target.volume = original.volume.clone();
                target.playback_offset = original.playback_offset;
            }
        }
    } else {
        let original = project
            .session
            .cells
            .iter()
            .find_map(|cell| match &cell.content {
                SessionCellContent::Midi(clip) if clip.id == picked => {
                    Some(SessionCellContent::Midi(clip.clone()))
                }
                SessionCellContent::Audio(clip) if clip.id == picked => {
                    Some(SessionCellContent::Audio(clip.clone()))
                }
                _ => None,
            })
            .ok_or(Invalid("alias cell missing"))?;
        for id in members {
            if let SessionCellContent::Midi(original) = &original
                && project.midi_clip(id).is_some()
            {
                let target = project
                    .midi_clips_mut(id)?
                    .iter_mut()
                    .find(|clip| clip.id == id)
                    .ok_or(Invalid("alias midi member missing"))?;
                target.source = Arc::clone(&original.source);
                target.active = original.active;
                target.name.clone_from(&original.name);
                target.flavor = original.flavor;
                target.volume = original.volume.clone();
                target.playback_offset = original.playback_offset;
            } else {
                let target = project
                    .session
                    .cells
                    .iter_mut()
                    .find_map(|cell| match &cell.content {
                        SessionCellContent::Midi(clip) if clip.id == id => Some(&mut cell.content),
                        SessionCellContent::Audio(clip) if clip.id == id => Some(&mut cell.content),
                        _ => None,
                    })
                    .ok_or(Invalid("alias cell missing"))?;
                *target = original.clone();
                match target {
                    SessionCellContent::Midi(clip) => clip.id = id,
                    SessionCellContent::Audio(clip) => clip.id = id,
                    SessionCellContent::Stop => {
                        return Err(Invalid("alias settings cannot be a stop cell"));
                    }
                }
            }
        }
    }
    Ok(Created::None)
}
