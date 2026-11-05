//! Worker-side event composition before the container fade. No callback work.
use crate::{Frame, Invalid};
use hex_project::{AudioClipEdit, AudioDomainClock, TempoMap};
use std::sync::atomic::{AtomicBool, Ordering};

/// The finite PCM root and the authored destination coordinate corresponding
/// to its source zero. Both clocks use the same units, but their extents and
/// tempo origins may differ.
pub(super) struct EventSource<'a> {
    pub(super) frames: &'a [Frame],
    pub(super) root: &'a AudioClipEdit,
    pub(super) zero_in_destination: u64,
}

pub(super) fn compose(
    source: EventSource<'_>,
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
    let frames_clock = matches!(edit.domain.clock, AudioDomainClock::Frames(_));
    let (length, aligned_zero) =
        if std::ptr::eq(edit, source.root) && source.zero_in_destination == 0 {
            (source.frames.len(), Some(0))
        } else {
            if frames_clock != matches!(source.root.domain.clock, AudioDomainClock::Frames(_)) {
                return Err(Invalid(
                    "audio event source and destination clock units differ",
                ));
            }
            let (_, source_length, _, _) = source.root.projected_domain(tempo)?;
            if source_length != source.frames.len() as u64 {
                return Err(Invalid("audio event PCM differs from its source clock"));
            }
            let (_, length, _, _) = edit.projected_domain(tempo)?;
            let aligned = frames_clock
                || (edit.processing_tempo(tempo) == source.root.processing_tempo(tempo)
                    && edit
                        .processing_origin()?
                        .checked_add(i128::from(source.zero_in_destination))
                        == Some(source.root.processing_origin()?));
            let zero = aligned
                .then(|| edit.root_frame_at(source.zero_in_destination, tempo))
                .transpose()?;
            (
                usize::try_from(length).map_err(|_| Invalid("audio event destination overflow"))?,
                zero,
            )
        };
    let mut output = Vec::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| Invalid("audio event composition allocation failed"))?;
    output.resize(length, [0.0, 0.0]);
    for event in &events.events {
        if event.source != 0
            || event
                .source_start
                .checked_add(event.length())
                .is_none_or(|end| end > source.root.domain.clock.length())
        {
            return Err(Invalid("audio event exceeds its retained source clock"));
        }
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
            .map(|fade| fade.reproject(|at| source.root.root_frame_at(at, tempo)))
            .transpose()?;
        let affine_start = if frames_clock {
            Some(event.source_start)
        } else if source.zero_in_destination.checked_add(event.source_start) == Some(event.start) {
            aligned_zero.and_then(|zero| (start as u64).checked_sub(zero))
        } else {
            None
        };
        if let (None, Some(offset)) = (fade.as_ref(), affine_start) {
            let offset =
                usize::try_from(offset).map_err(|_| Invalid("audio event source overflow"))?;
            let source_end = offset
                .checked_add(destination.len())
                .ok_or(Invalid("audio event source overflow"))?;
            destination.copy_from_slice(
                source
                    .frames
                    .get(offset..source_end)
                    .ok_or(Invalid("audio event source exceeds prepared root"))?,
            );
        } else {
            for (index, sample) in destination.iter_mut().enumerate() {
                if index % 4096 == 0 && cancelled.load(Ordering::Relaxed) {
                    return Err(Invalid("audio event preparation cancelled"));
                }
                let source_frame = if let Some(offset) = affine_start {
                    offset
                        .checked_add(index as u64)
                        .ok_or(Invalid("audio event source position overflow"))?
                } else {
                    let position = edit.root_position_at((start + index) as u64, tempo)?;
                    let source_position = event
                        .source_start
                        .checked_add(position.saturating_sub(event.start))
                        .ok_or(Invalid("audio event source position overflow"))?;
                    source.root.root_frame_at(source_position, tempo)?
                };
                *sample = *source
                    .frames
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

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
