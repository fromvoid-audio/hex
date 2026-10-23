//! Identity-preserving source-kind replacements. Rendering belongs to control workers.
use crate::{
    BeatTime, Clip, ClipFade, ClipGain, ClipId, Created, Invalid, MidiClip, MidiSource, Project,
    SessionAudioClip, SessionCellContent, SessionMidiClip,
};
use std::{path::PathBuf, sync::Arc};

pub(crate) fn audio_to_midi(project: &mut Project, id: ClipId) -> Result<Created, Invalid> {
    if let Some((track, index)) = project.tracks.iter().enumerate().find_map(|(track, item)| {
        item.clips()
            .iter()
            .position(|clip| clip.id == id)
            .map(|index| (track, index))
    }) {
        let clip = &project.tracks[track].content.audio_clips[index];
        let start = project.tempo_map().frame_to_beat(clip.start)?;
        let end = project.tempo_map().frame_to_beat(
            clip.start
                .checked_add(clip.duration)
                .ok_or(Invalid("clip end overflow"))?,
        )?;
        let length = beat_length(start, end)?;
        let name = clip.name.clone();
        let flavor = clip.flavor;
        project.tracks[track].content.audio_clips.remove(index);
        project.tracks[track].content.midi_clips.push(MidiClip {
            name,
            flavor,
            id,
            start,
            length,
            volume: crate::MidiClipVolume::default(),
            source: Arc::new(MidiSource::defaults(length)),
        });
        return Ok(Created::Clip(id));
    }
    let cell = project
        .session
        .cells
        .iter()
        .position(|cell| {
            matches!(&cell.content,
        SessionCellContent::Audio(clip) if clip.id == id)
        })
        .ok_or(Invalid("audio clip no longer exists"))?;
    let SessionCellContent::Audio(clip) = &project.session.cells[cell].content else {
        return Err(Invalid("audio clip no longer exists"));
    };
    let length = project
        .tempo_map()
        .frame_to_beat(clip.source_end - clip.source_start)?;
    let length = beat_length(BeatTime::ZERO, length)?;
    project.session.cells[cell].content = SessionCellContent::Midi(SessionMidiClip {
        flavor: clip.flavor,
        id,
        name: clip.name.clone(),
        volume: crate::MidiClipVolume::default(),
        source: Arc::new(MidiSource::defaults(length)),
    });
    Ok(Created::SessionClip(id))
}

fn beat_length(start: BeatTime, end: BeatTime) -> Result<BeatTime, Invalid> {
    end.raw()
        .checked_sub(start.raw())
        .filter(|length| *length > 0)
        .map(BeatTime::from_raw)
        .ok_or(Invalid("clip is too short for midi conversion"))
}

pub(crate) fn midi_to_audio(
    project: &mut Project,
    id: ClipId,
    path: PathBuf,
    frames: u64,
) -> Result<Created, Invalid> {
    let (start, expected) = project.midi_bounce_range(id)?;
    if frames == 0 || frames != expected {
        return Err(Invalid("bounce length differs from midi clip"));
    }
    let asset = match project.apply(crate::Command::AddAsset {
        path,
        sample_rate: project.sample_rate(),
        frames,
    })? {
        Created::Asset(asset) => asset,
        _ => return Err(Invalid("bounce asset was not created")),
    };
    if let Some((track, index)) = project.tracks.iter().enumerate().find_map(|(track, item)| {
        item.midi_clips()
            .iter()
            .position(|clip| clip.id == id)
            .map(|index| (track, index))
    }) {
        let original = project.tracks[track].content.midi_clips.remove(index);
        project.tracks[track].content.audio_clips.push(Clip {
            name: original.name,
            flavor: original.flavor,
            id,
            asset,
            start,
            duration: frames,
            source_offset: 0,
            source_length: frames,
            looping: false,
            source_phase: 0,
            fade: ClipFade::neutral(frames)?,
            gain: ClipGain::default(),
            edit: None,
        });
        return Ok(Created::Clip(id));
    }
    let cell = project
        .session
        .cells
        .iter_mut()
        .find(|cell| {
            matches!(&cell.content,
        SessionCellContent::Midi(clip) if clip.id == id)
        })
        .ok_or(Invalid("midi clip no longer exists"))?;
    let SessionCellContent::Midi(clip) = &cell.content else {
        return Err(Invalid("midi clip no longer exists"));
    };
    cell.content = SessionCellContent::Audio(SessionAudioClip {
        flavor: clip.flavor,
        id,
        name: clip.name.clone(),
        asset,
        source_start: 0,
        source_end: frames,
        gain: ClipGain::default(),
    });
    Ok(Created::SessionClip(id))
}

impl Project {
    /// Finite bounce window: Arrangement placement, or a Session source's initial
    /// pass through its loop end (source end for one-shots), at the current tempo.
    pub fn midi_bounce_range(&self, id: ClipId) -> Result<(u64, u64), Invalid> {
        let (start, end) = if let Some(clip) = self.midi_clip(id) {
            (
                clip.start,
                BeatTime::from_raw(
                    clip.start
                        .raw()
                        .checked_add(clip.length.raw())
                        .ok_or(Invalid("midi clip end overflow"))?,
                ),
            )
        } else if let Some(clip) = self.session_midi_clip(id) {
            let source = clip.source();
            let end = source.effective_end();
            (BeatTime::ZERO, beat_length(source.source_start(), end)?)
        } else {
            return Err(Invalid("midi clip no longer exists"));
        };
        let start = self.tempo_map().beat_to_frame(start)?;
        let end = self.tempo_map().beat_to_frame(end)?;
        let frames = end
            .checked_sub(start)
            .filter(|frames| *frames > 0)
            .ok_or(Invalid("midi clip has no audible frame interval"))?;
        Ok((start, frames))
    }
}
