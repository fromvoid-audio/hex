//! Finite Console render plans and atomic audio-cell replacement. A looping
//! source prints its initial pass plus one complete cycle; the latter becomes
//! the frozen audio cycle. Device variation after that captured cycle is frozen,
//! not an infinite live-DSP equivalence. A release tail remains outside the cycle.
use super::*;
use crate::{BeatTime, SessionAudioClip, SessionAudioLoop};

impl Project {
    /// Musical length of the finite Console MIDI print, including one complete
    /// extra cycle for looping sources. Playback offset stays on the source;
    /// tempo conversion is performed at the captured project's timeline origin.
    pub fn console_midi_bounce_length(&self, clip: ClipId) -> Result<BeatTime, Invalid> {
        let source = self
            .session_midi_clip(clip)
            .ok_or(Invalid("console midi clip no longer exists"))?
            .source();
        let first = source
            .effective_end()
            .raw()
            .checked_sub(source.source_start().raw())
            .filter(|n| *n > 0)
            .ok_or(Invalid("console midi source has no finite initial pass"))?;
        let cycle = if source.looping() {
            source.loop_end().raw() - source.loop_start().raw()
        } else {
            0
        };
        Ok(BeatTime::from_raw(
            first
                .checked_add(cycle)
                .ok_or(Invalid("console bounce length overflows"))?,
        ))
    }
}
pub(super) fn plan(project: &Project, track: TrackId) -> Result<Vec<BounceTrack>, Invalid> {
    let mut items = Vec::new();
    for cell in project
        .session_cells()
        .iter()
        .filter(|cell| cell.track() == track)
    {
        let (clip, end, loop_range) = match cell.content() {
            SessionCellContent::Stop => continue,
            SessionCellContent::Midi(clip) => {
                let length = project.console_midi_bounce_length(clip.id())?;
                let end = project.tempo_map().beat_to_frame(length)?;
                let range = if clip.source().looping() {
                    let first =
                        clip.source().effective_end().raw() - clip.source().source_start().raw();
                    let first = project
                        .tempo_map()
                        .beat_to_frame(BeatTime::from_raw(first))?;
                    Some(SessionAudioLoop::new(first, end)?)
                } else {
                    None
                };
                (clip.id(), end, range)
            }
            SessionCellContent::Audio(clip) => {
                let (end, range) = if let Some(range) = clip.loop_range() {
                    let first = range.end() - clip.source_start();
                    let end = first
                        .checked_add(range.end() - range.start())
                        .ok_or(Invalid("console bounce length overflows"))?;
                    (end, Some(SessionAudioLoop::new(first, end)?))
                } else {
                    (clip.source_end() - clip.source_start(), None)
                };
                (clip.id(), end, range)
            }
        };
        if end == 0 || end > MAX_ARRANGEMENT_BOUNCE_FRAMES {
            return Err(Invalid("console bounce exceeds its finite frame budget"));
        }
        items.push(BounceTrack {
            track,
            start: 0,
            end,
            clips: vec![clip],
            source: BounceSource::Console { clip, loop_range },
        });
    }
    Ok(items)
}
pub(super) fn adopt(
    project: &mut Project,
    item: &BounceTrack,
    file: BounceFile,
) -> Result<ClipId, Invalid> {
    let BounceSource::Console { clip, loop_range } = item.source else {
        return Err(Invalid("console bounce source is missing"));
    };
    let (active, flavor, name) = project
        .session
        .cells
        .iter()
        .find_map(|cell| {
            if cell.track != item.track {
                return None;
            }
            match &cell.content {
                SessionCellContent::Midi(c) if c.id() == clip => {
                    Some((c.active(), c.flavor(), c.name().to_owned()))
                }
                SessionCellContent::Audio(c) if c.id() == clip => {
                    Some((c.active(), c.flavor(), c.name().to_owned()))
                }
                _ => None,
            }
        })
        .ok_or(Invalid("console bounce cell changed"))?;
    if let Some(range) = loop_range {
        range.validate_source(0, file.frames)?;
    }
    let Created::Asset(asset) = project.apply(Command::AddAsset {
        path: file.path,
        sample_rate: project.sample_rate(),
        frames: file.frames,
    })?
    else {
        return Err(Invalid("console bounce asset was not created"));
    };
    let cell = project
        .session
        .cells
        .iter_mut()
        .find(|cell| match &cell.content {
            SessionCellContent::Midi(c) => c.id() == clip,
            SessionCellContent::Audio(c) => c.id() == clip,
            SessionCellContent::Stop => false,
        })
        .ok_or(Invalid("console bounce cell changed"))?;
    cell.content = SessionCellContent::Audio(SessionAudioClip {
        active,
        flavor,
        id: clip,
        name,
        asset,
        source_start: 0,
        source_end: file.frames,
        loop_range,
        gain: crate::ClipGain::UNITY,
    });
    Ok(clip)
}
