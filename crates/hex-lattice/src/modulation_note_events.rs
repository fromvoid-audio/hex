//! Borrow the ordered instrument event input without retaining caller storage.
use crate::NativeInstrumentEvent;
use hex_lattice_contract::DeviceContractError as Error;

/// The ordinary instrument host can deliver up to 259 events in one chunk.
/// This ceiling leaves room for other callers while bounding callback scans.
const MAX_EVENTS: usize = 512;

pub(crate) fn validate_events(
    events: &[NativeInstrumentEvent],
    frames: usize,
) -> Result<(), Error> {
    if events.len() > MAX_EVENTS {
        return Err(Error::RuntimeBudgetExceeded);
    }
    if events
        .last()
        .is_some_and(|event| event.frame_offset() >= frames)
        || events
            .windows(2)
            .any(|pair| pair[0].frame_offset() > pair[1].frame_offset())
    {
        return Err(Error::InvalidParameterValue);
    }
    Ok(())
}

pub(crate) fn note_on_offsets(
    events: &[NativeInstrumentEvent],
) -> impl Iterator<Item = usize> + '_ {
    events.iter().filter_map(|event| match event {
        NativeInstrumentEvent::NoteOn { frame_offset, .. } => Some(*frame_offset),
        _ => None,
    })
}
