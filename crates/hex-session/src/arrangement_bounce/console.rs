//! Disposable Arrangement projection of one Console source. Existing finite
//! export owns all DSP, device resources and bounded release capture. Only this
//! private projection forces activation; adoption retains the authored flag.
use super::*;
use hex_project::{BounceSource, ClipSnapshot, MidiClipSnapshot, SessionCellContent};

pub(super) fn content(
    project: &Project,
    item: &BounceTrack,
    next_id: &mut u64,
) -> Result<TrackContentSnapshot, String> {
    let BounceSource::Console { clip, .. } = item.source else {
        return Err("console bounce source is missing".into());
    };
    let cell = project
        .session_cells()
        .iter()
        .find(|cell| {
            cell.track() == item.track
                && match cell.content() {
                    SessionCellContent::Midi(c) => c.id() == clip,
                    SessionCellContent::Audio(c) => c.id() == clip,
                    SessionCellContent::Stop => false,
                }
        })
        .ok_or("console bounce cell changed")?;
    match cell.content() {
        SessionCellContent::Midi(c) => {
            let source = c.source();
            let length = project
                .console_midi_bounce_length(clip)
                .map_err(|e| e.to_string())?;
            Ok(TrackContentSnapshot::Hybrid {
                audio_clips: Vec::new(),
                midi_clips: vec![MidiClipSnapshot {
                    active: true,
                    name: c.name().to_owned(),
                    flavor: c.flavor(),
                    id: c.id().value(),
                    content_view: None,
                    one_shot_boundary: source.one_shot_boundary(),
                    playback_offset_raw: c.playback_offset().raw(),
                    volume: c.volume().clone(),
                    start_raw: 0,
                    length_raw: length.raw(),
                    source_length_raw: source.source_length().raw(),
                    source_start_raw: source.source_start().raw(),
                    loop_start_raw: source.loop_start().raw(),
                    loop_end_raw: source.loop_end().raw(),
                    looping: source.looping(),
                    notes: source
                        .notes()
                        .iter()
                        .map(|n| hex_project::MidiNoteSnapshot {
                            id: n.id().value(),
                            active: n.active(),
                            start_raw: n.start().raw(),
                            duration_raw: n.duration().raw(),
                            pitch: n.pitch(),
                            velocity: n.velocity(),
                            channel: n.channel(),
                        })
                        .collect(),
                }],
            })
        }
        SessionCellContent::Audio(c) => {
            let mut clips = Vec::new();
            let first_end = c.loop_range().map_or(c.source_end(), |range| range.end());
            let first_length = first_end - c.source_start();
            let make = |id, start, offset, length| -> Result<ClipSnapshot, String> {
                Ok(ClipSnapshot {
                    active: true,
                    name: c.name().to_owned(),
                    flavor: c.flavor(),
                    id,
                    asset: c.asset().value(),
                    start,
                    duration: length,
                    source_offset: offset,
                    source_length: length,
                    looping: false,
                    source_phase: 0,
                    gain: c.gain(),
                    edit: None,
                    fade: hex_project::ClipFade::neutral(length).map_err(|e| e.to_string())?,
                })
            };
            clips.push(make(c.id().value(), 0, c.source_start(), first_length)?);
            if let Some(range) = c.loop_range() {
                let id = *next_id;
                *next_id = next_id
                    .checked_add(1)
                    .ok_or("console projection identity overflows")?;
                clips.push(make(
                    id,
                    first_length,
                    range.start(),
                    range.end() - range.start(),
                )?);
            }
            Ok(TrackContentSnapshot::Hybrid {
                audio_clips: clips,
                midi_clips: Vec::new(),
            })
        }
        SessionCellContent::Stop => Err("console bounce source is a stop cell".into()),
    }
}
