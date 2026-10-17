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
        let start = usize::try_from(edit.root_frame_at(event.start, tempo)?)
            .map_err(|_| Invalid("audio event start overflow"))?;
        let end = usize::try_from(edit.root_frame_at(event.end, tempo)?)
            .map_err(|_| Invalid("audio event end overflow"))?;
        let destination = output
            .get_mut(start..end)
            .ok_or(Invalid("audio event exceeds prepared root"))?;
        if event.source_start == event.start {
            destination.copy_from_slice(
                source
                    .get(start..end)
                    .ok_or(Invalid("audio event source exceeds prepared root"))?,
            );
        } else if matches!(edit.domain.clock, AudioDomainClock::Frames(_)) {
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
                let position = edit.root_position_at((start + index) as u64, tempo)?;
                let source_position = event
                    .source_start
                    .checked_add(position.saturating_sub(event.start))
                    .ok_or(Invalid("audio event source position overflow"))?;
                let source_frame = edit.root_frame_at(source_position, tempo)?;
                *sample = *source
                    .get(source_frame as usize)
                    .ok_or(Invalid("audio event sample exceeds prepared root"))?;
            }
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(Invalid("audio event preparation cancelled"));
        }
    }
    Ok(output)
}
