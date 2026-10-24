//! One bounded, callback-local gain cache shared by every active analytic voice
//! of a Console clip. Musical conversions occur only at original ramp changes.
use super::*;
use crate::midi_clip_volume::{PreparedMidiClipVolume, VolumePlacement, VolumeRamp};
use crate::prepared_tempo::MidiTempoBreaks;

pub(super) fn prepare_gains<T: MidiTempoBreaks + ?Sized>(
    program: &PreparedMidiClipVolume,
    origin: SessionMidiOrigin,
    timing: &T,
    start_frame: u64,
    frames: usize,
    spans: &MidiSpanBuffer,
) -> Result<Option<[f64; MIDI_TEMPLATE_QUERY_FRAMES as usize]>, MidiTemplateError> {
    if frames > MIDI_TEMPLATE_QUERY_FRAMES as usize {
        return Err(MidiTemplateError::InvalidFrameRange);
    }
    if frames == 0 || spans.is_empty() {
        return Ok(None);
    }
    let end_frame = start_frame
        .checked_add(frames as u64)
        .ok_or(MidiTemplateError::ArithmeticOverflow)?;
    let mut active = [false; MIDI_TEMPLATE_QUERY_FRAMES as usize];
    for span in spans.spans().filter(|span| span.voice.is_some()) {
        let start = span.start_frame.max(start_frame);
        let end = span.end_frame.min(end_frame);
        if start < end {
            active[(start - start_frame) as usize..(end - start_frame) as usize].fill(true);
        }
    }
    if !active[..frames].iter().any(|active| *active) {
        return Ok(None);
    }
    let placement = VolumePlacement::Session { origin };
    let historical = program
        .historical_end_gain(timing, placement)
        .map_err(|error| MidiTemplateError::InvalidSource(error.0))?;
    let endpoint = if historical.is_some() {
        None
    } else {
        program
            .finite_endpoint(timing, placement)
            .map_err(|error| MidiTemplateError::InvalidSource(error.0))?
    };
    let mut gains = [1.0; MIDI_TEMPLATE_QUERY_FRAMES as usize];
    let mut cached: Option<VolumeRamp> = None;
    for index in 0..frames {
        if !active[index] {
            continue;
        }
        if let Some(gain) = historical {
            gains[index] = gain;
            continue;
        }
        let frame = start_frame + index as u64;
        if let Some(endpoint) = endpoint.filter(|endpoint| frame >= endpoint.frame) {
            gains[index] = endpoint.gain;
            continue;
        }
        let ramp = match cached.filter(|ramp| {
            frame >= ramp.start_frame && ramp.next_boundary.is_none_or(|next| frame < next)
        }) {
            Some(ramp) => ramp,
            None => program
                .segment_at(timing, placement, frame, frames - index)
                .map_err(|error| MidiTemplateError::InvalidSource(error.0))?
                .ok_or(MidiTemplateError::InvalidSource(
                    "active analytic voice has no clip volume segment",
                ))?,
        };
        gains[index] = ramp
            .gain_at(frame)
            .map_err(|error| MidiTemplateError::InvalidSource(error.0))?;
        cached = Some(ramp);
    }
    Ok(Some(gains))
}

#[cfg(test)]
mod tests;
