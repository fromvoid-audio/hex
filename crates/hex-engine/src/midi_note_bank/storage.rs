//! Actual constructor-paired note-bank storage, independent of device source
//! leases, native publication, provider contexts and active runtime custody.
use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageError, AudioStorageRole, audio_storage::Owner,
    owned_shared::OwnedShared,
};

pub(crate) fn visit<'a>(
    bank: &'a OwnedShared<PreparedMidiNoteBank>,
    ledger: &mut AudioRootStorageLedger<'a>,
    role: AudioStorageRole,
) -> Result<(), AudioStorageError> {
    if ledger.contains_root(bank) {
        return Ok(());
    }
    ledger.root(bank, role)?;
    visit_contents(bank, ledger)
}
pub(crate) fn visit_contents<'a>(
    bank: &'a OwnedShared<PreparedMidiNoteBank>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let visited = ledger.contains_shared_storage(bank.arc());
    ledger.shared(
        bank.arc(),
        bank.parts(),
        Owner::exclusive(bank),
        crate::audio_only_inputs::handle()?,
        None,
    )?;
    if visited {
        return Ok(());
    }
    let PreparedMidiNoteBank {
        entries,
        context_id,
        context_index,
    } = bank.as_ref();
    let _: &Option<crate::shared_analytic::SharedContextId> = context_id;
    let _: &Option<u16> = context_index;
    ledger.vec(entries)?;
    for entry in entries {
        let BankEntry { key, sampler } = entry;
        let _: &u16 = key;
        // The fixed coefficient ABI is Copy and contains no owning handles.
        // Context indices are not leases: actual retained context/source owners
        // must be inventoried independently before any graph admission.
        let _: PreparedMidiNote = *sampler;
    }
    Ok(())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Selected actual Arrangement/Session/routed note-bank fields, owner
    /// allocations and entry-buffer capacities. Includes cached banks through
    /// their original constructor receipts. This does not inventory enclosing
    /// templates, provider contexts, source leases or active callback owners,
    /// and grants no publication/retirement or complete graph authority.
    pub fn borrow_revision_note_banks_on_control(
        &mut self,
        revision: &'a crate::PreparedRevision,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            for track in revision.arrangement().tracks() {
                if let Some(instrument) = &track.instrument {
                    visit(&instrument.bank, ledger, role)?;
                }
            }
            revision
                .session()
                .visit_midi_note_banks_on_control(ledger, role)
        })
    }
}
