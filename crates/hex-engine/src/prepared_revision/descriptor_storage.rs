//! Actual private descriptor fields; no model snapshot or synthetic owner.
use super::*;
use crate::{AudioRootStorageLedger, AudioStorageError};

pub(super) fn backing_bytes(
    rows: &Vec<PreparedMutableTrackRevision>,
) -> Result<usize, AudioStorageError> {
    fn bytes<T>(v: &Vec<T>) -> Result<usize, AudioStorageError> {
        v.capacity()
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(AudioStorageError::Overflow)
    }
    rows.iter().try_fold(bytes(rows)?, |total, row| {
        let effects = bytes(&row.effects)?;
        let sources = bytes(&row.arrangement_midi_sources)?;
        total
            .checked_add(effects)
            .and_then(|n| n.checked_add(sources))
            .ok_or(AudioStorageError::Overflow)
    })
}

#[cfg(test)]
mod tests;

pub(super) fn visit<'a>(
    rows: &'a Vec<PreparedMutableTrackRevision>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    fn scalar<T: Copy>(_: &T) {}
    fn device<'a>(
        identity: &'a PreparedDeviceIdentity,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let PreparedDeviceIdentity {
            id,
            state,
            bypassed,
        } = identity;
        scalar(id);
        scalar(bypassed);
        ledger.borrow_canonical_native_state_on_control(state)
    }
    ledger.vec(rows)?;
    for PreparedMutableTrackRevision {
        id,
        instrument,
        instrument_position,
        effects,
        arrangement_midi_sources,
    } in rows
    {
        scalar(id);
        scalar(instrument_position);
        ledger.vec(effects)?;
        ledger.vec(arrangement_midi_sources)?;
        for identity in instrument.iter().chain(effects) {
            device(identity, ledger)?;
        }
        for PreparedArrangementMidiSource {
            id,
            start,
            length,
            source,
        } in arrangement_midi_sources
        {
            scalar(id);
            scalar(start);
            scalar(length);
            ledger.model_source_if_unvisited(source)?;
        }
    }
    Ok(())
}
