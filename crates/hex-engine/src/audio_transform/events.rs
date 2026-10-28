//! Worker-side event composition before the container fade. No callback work.
use crate::{Frame, Invalid};
use hex_project::{AudioClipEdit, AudioDomainClock, TempoMap};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn compose(
    source: &[Frame],
    edit: &AudioClipEdit,
    tempo: &TempoMap,
    cancelled: &AtomicBool,
) -> Result<Vec<Frame>, Invalid> {
    let events = edit
        .domain
        .events
        .as_ref()
        .ok_or(Invalid("audio event sequence missing"))?;
    events.validate(edit.domain.clock.length())?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(source.len())
        .map_err(|_| Invalid("audio event composition allocation failed"))?;
    output.resize(source.len(), [0.0, 0.0]);
    for event in &events.events {
        if !event.active {
            continue;
        }
        let start = usize::try_from(edit.root_frame_at(event.start, tempo)?)
            .map_err(|_| Invalid("audio event start overflow"))?;
        let end = usize::try_from(edit.root_frame_at(event.end, tempo)?)
            .map_err(|_| Invalid("audio event end overflow"))?;
        let destination = output
            .get_mut(start..end)
            .ok_or(Invalid("audio event exceeds prepared root"))?;
        // Source-window envelope geometry is converted exactly once on control.
        // Cropped/split events retain its original source domain; moved events
        // read the same faded samples rather than restarting a destination ramp.
        let fade = event
            .fade
            .map(|fade| fade.reproject(|at| edit.root_frame_at(at, tempo)))
            .transpose()?;
        if fade.is_none() && event.source_start == event.start {
            destination.copy_from_slice(
                source
                    .get(start..end)
                    .ok_or(Invalid("audio event source exceeds prepared root"))?,
            );
        } else if fade.is_none() && matches!(edit.domain.clock, AudioDomainClock::Frames(_)) {
            let offset = usize::try_from(event.source_start)
                .map_err(|_| Invalid("audio event source overflow"))?;
            let source_end = offset
                .checked_add(destination.len())
                .ok_or(Invalid("audio event source overflow"))?;
            destination.copy_from_slice(
                source
                    .get(offset..source_end)
                    .ok_or(Invalid("audio event source exceeds prepared root"))?,
            );
        } else {
            for (index, sample) in destination.iter_mut().enumerate() {
                if index % 4096 == 0 && cancelled.load(Ordering::Relaxed) {
                    return Err(Invalid("audio event preparation cancelled"));
                }
                let source_frame = if event.source_start == event.start {
                    (start + index) as u64
                } else if matches!(edit.domain.clock, AudioDomainClock::Frames(_)) {
                    event
                        .source_start
                        .checked_add(index as u64)
                        .ok_or(Invalid("audio event source position overflow"))?
                } else {
                    let position = edit.root_position_at((start + index) as u64, tempo)?;
                    let source_position = event
                        .source_start
                        .checked_add(position.saturating_sub(event.start))
                        .ok_or(Invalid("audio event source position overflow"))?;
                    edit.root_frame_at(source_position, tempo)?
                };
                *sample = *source
                    .get(
                        usize::try_from(source_frame)
                            .map_err(|_| Invalid("audio event sample overflow"))?,
                    )
                    .ok_or(Invalid("audio event sample exceeds prepared root"))?;
                if let Some(fade) = fade {
                    let position = source_frame
                        .checked_sub(fade.source_start())
                        .filter(|at| *at < fade.shape().span())
                        .ok_or(Invalid("audio event sample exceeds its fade domain"))?;
                    let gain = fade.shape().gain_at(position);
                    for channel in sample {
                        *channel *= gain;
                    }
                }
            }
        }
        let gain = event.gain.linear();
        if gain != 1.0 {
            for (index, sample) in destination.iter_mut().enumerate() {
                if index % 4096 == 0 && cancelled.load(Ordering::Relaxed) {
                    return Err(Invalid("audio event preparation cancelled"));
                }
                for channel in sample {
                    *channel *= gain;
                }
            }
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(Invalid("audio event preparation cancelled"));
        }
    }
    Ok(output)
}
