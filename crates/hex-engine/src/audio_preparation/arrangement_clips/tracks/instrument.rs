//! Actual Arrangement MIDI/state fields, retained through control retirement.
//! This is not a factory birth declaration or a complete graph reservation.
use super::super::Disposal;
use crate::{AudioRootStorageLedger, AudioStorageError, InstrumentTrack, Track};
use hex_lattice_contract::NativeDeviceState;

#[derive(Default)]
pub(super) struct InstrumentCustody {
    instrument: Option<InstrumentTrack>,
    state: Option<NativeDeviceState>,
    instrument_disposal: Disposal,
    state_disposal: Disposal,
    #[cfg(test)]
    pub(super) panic_after_instrument_take: bool,
}
impl InstrumentCustody {
    pub(super) fn vacant(&self) -> bool {
        self.instrument.is_none()
            && self.state.is_none()
            && self.instrument_disposal == Disposal::Unstarted
            && self.state_disposal == Disposal::Unstarted
    }
    fn checked_view<'a>(
        &'a self,
        actual: Option<&'a Track>,
    ) -> Result<(Option<&'a InstrumentTrack>, Option<&'a NativeDeviceState>), AudioStorageError>
    {
        if self.instrument_disposal == Disposal::Pending || self.state_disposal == Disposal::Pending
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        if let Some(track) = actual {
            if !self.vacant() {
                return Err(AudioStorageError::SourceMismatch);
            }
            Ok((track.instrument.as_ref(), track.instrument_state.as_ref()))
        } else {
            Ok((self.instrument.as_ref(), self.state.as_ref()))
        }
    }
    pub(super) fn preflight_take(&self, track: &Track) -> Result<(), AudioStorageError> {
        if !self.vacant() {
            return Err(AudioStorageError::Capacity);
        }
        if let Some(instrument) = &track.instrument {
            let InstrumentTrack {
                spans,
                nodes,
                root,
                timeline_end,
                bank,
                shared_contexts,
                mutable,
            } = instrument;
            fn copy_leaf<T: Copy>(_: T) {}
            copy_leaf((*root, *timeline_end, *mutable));
            let _: (
                &Vec<crate::midi_compile::CompiledNoteSpan>,
                &Vec<crate::midi_compile::SpanNode>,
                &crate::owned_shared::OwnedShared<crate::midi_note_bank::PreparedMidiNoteBank>,
            ) = (spans, nodes, bank);
            shared_contexts.refuse_opaque_control_drop()?;
        }
        Ok(())
    }
    /// Called only after the whole owning Track/destination batch preflight.
    pub(super) fn install(
        &mut self,
        instrument: Option<InstrumentTrack>,
        state: Option<NativeDeviceState>,
    ) {
        self.instrument = instrument;
        self.state = state;
    }
    /// Owned Vec buffers only; shared banks/tables/canonical state are visited
    /// separately below, and need the full union/peak reservation before entry.
    pub(super) fn backing_bytes(
        &self,
        actual: Option<&Track>,
    ) -> Result<(usize, usize), AudioStorageError> {
        let (instrument, _) = self.checked_view(actual)?;
        instrument.map_or(Ok((0, 0)), |instrument| {
            Ok((
                instrument
                    .spans
                    .capacity()
                    .checked_mul(std::mem::size_of::<crate::midi_compile::CompiledNoteSpan>())
                    .ok_or(AudioStorageError::Overflow)?,
                instrument
                    .nodes
                    .capacity()
                    .checked_mul(std::mem::size_of::<crate::midi_compile::SpanNode>())
                    .ok_or(AudioStorageError::Overflow)?,
            ))
        })
    }
    pub(super) fn visit<'a>(
        &'a self,
        actual: Option<&'a Track>,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let (instrument, state) = self.checked_view(actual)?;
        if let Some(instrument) = instrument {
            crate::midi_template::storage::visit_arrangement_instrument_contents(
                instrument, ledger,
            )?;
        }
        if let Some(state) = state {
            ledger.canonical_native_state(state)?;
        }
        Ok(())
    }
    pub(super) fn has_next(&self) -> bool {
        self.instrument.is_some()
            || self.state.is_some()
            || self.instrument_disposal == Disposal::Pending
            || self.state_disposal == Disposal::Pending
    }
    pub(super) fn dispose_next(&mut self) -> Result<(), AudioStorageError> {
        self.checked_view(None)?;
        if self.instrument.is_some() {
            self.instrument_disposal = Disposal::Pending;
            drop(self.instrument.take());
            #[cfg(test)]
            assert!(
                !self.panic_after_instrument_take,
                "persistent Arrangement instrument disposal panic"
            );
            self.instrument_disposal = Disposal::Complete;
        } else if self.state.is_some() {
            self.state_disposal = Disposal::Pending;
            drop(self.state.take());
            self.state_disposal = Disposal::Complete;
        }
        Ok(())
    }
    pub(super) fn clear_disposed(&mut self) -> Result<(), AudioStorageError> {
        if self.has_next() {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.instrument_disposal = Disposal::Unstarted;
        self.state_disposal = Disposal::Unstarted;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
