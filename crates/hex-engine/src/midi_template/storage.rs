//! Actual prepared MIDI template containers/backings. Source-model identities,
//! enclosing revision metadata, native runtime custody and retirement remain
//! separate obligations; no scalar cache key supplies those owners.
use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageError, AudioStorageRole, audio_storage::Owner,
    owned_shared::OwnedShared,
};
fn visit_owned<'a, T: 'static>(
    owner: &'a OwnedShared<T>,
    ledger: &mut AudioRootStorageLedger<'a>,
    role: AudioStorageRole,
    contents: impl FnOnce(&'a T, &mut AudioRootStorageLedger<'a>) -> Result<(), AudioStorageError>,
) -> Result<(), AudioStorageError> {
    if ledger.contains_root(owner) {
        return Ok(());
    }
    ledger.root(owner, role)?;
    let visited = ledger.contains_shared_storage(owner.arc());
    ledger.shared(
        owner.arc(),
        owner.parts(),
        Owner::exclusive(owner),
        crate::audio_only_inputs::handle()?,
        None,
    )?;
    if !visited {
        contents(owner.as_ref(), ledger)?;
    }
    Ok(())
}
pub(crate) fn visit_source<'a>(
    source: &'a OwnedShared<PreparedMidiSourceTemplate>,
    ledger: &mut AudioRootStorageLedger<'a>,
    role: AudioStorageRole,
) -> Result<(), AudioStorageError> {
    visit_owned(source, ledger, role, |source, ledger| {
        let PreparedMidiSourceTemplate {
            track,
            clip,
            sample_rate,
            admitted_max_micro_bpm,
            first_pass,
            period,
            intro,
            repeats,
            intro_spans,
            repeat_spans,
            density,
        } = source;
        let _: (&TrackId, &ClipId, &u32, &u64, &u64, &Option<u64>) = (
            track,
            clip,
            sample_rate,
            admitted_max_micro_bpm,
            first_pass,
            period,
        );
        let DensityEnvelope {
            max_edges,
            max_onsets,
        } = density;
        let _: (&usize, &usize) = (max_edges, max_onsets);
        ledger.vec(intro)?;
        ledger.vec(repeats)?;
        ledger.vec(intro_spans)?;
        ledger.vec(repeat_spans)?;
        // All four vector element types are fixed Copy metadata, not owners.
        for edge in intro.iter().chain(repeats) {
            let _: SourceEdge = *edge;
        }
        for span in intro_spans.iter().chain(repeat_spans) {
            let _: SourceSpan = *span;
        }
        Ok(())
    })
}
pub(crate) fn visit_instrument<'a>(
    instrument: &'a OwnedShared<PreparedMidiInstrumentTemplate>,
    ledger: &mut AudioRootStorageLedger<'a>,
    role: AudioStorageRole,
) -> Result<(), AudioStorageError> {
    visit_owned(
        instrument,
        ledger,
        role,
        PreparedMidiInstrumentTemplate::visit_storage_contents_on_control,
    )
}
pub(crate) fn visit_arrangement_instrument<'a>(
    instrument: &'a crate::InstrumentTrack,
    ledger: &mut AudioRootStorageLedger<'a>,
    role: AudioStorageRole,
) -> Result<(), AudioStorageError> {
    if ledger.contains_root(instrument) {
        return Ok(());
    }
    ledger.root(instrument, role)?;
    let crate::InstrumentTrack {
        spans,
        nodes,
        root,
        timeline_end,
        bank,
        shared_contexts,
        mutable,
    } = instrument;
    let _: (&Option<usize>, &Option<u64>, &bool) = (root, timeline_end, mutable);
    ledger.vec(spans)?;
    ledger.vec(nodes)?;
    for span in spans {
        let _: crate::midi_compile::CompiledNoteSpan = *span;
    }
    for node in nodes {
        let crate::midi_compile::SpanNode {
            span_index,
            span_start,
            max_end,
            left,
            right,
        } = node;
        let _: (&usize, &u64, &u64, &Option<usize>, &Option<usize>) =
            (span_index, span_start, max_end, left, right);
    }
    crate::midi_note_bank::storage::visit_contents(bank, ledger)?;
    crate::shared_analytic::resource_storage::visit_table_contents(shared_contexts, ledger)
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Selected actual prepared Arrangement instrument metadata, Session/routed
    /// source/instrument template containers, all backing capacities, note banks
    /// and source context tables. Cached physical fields are counted once across
    /// revisions; distinct owning aliases remain required. Excludes canonical
    /// model-source identities, enclosing clip/revision metadata, full construction
    /// peak and active runtime custody. Use instead of overlapping selected bank/
    /// table-only inventories; this grants no native publication/retirement proof.
    pub fn borrow_revision_midi_templates_on_control(
        &mut self,
        revision: &'a crate::PreparedRevision,
        role: AudioStorageRole,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            crate::shared_analytic::resource_storage::visit_table(
                revision.arrangement().shared_contexts(),
                ledger,
                role,
            )?;
            crate::shared_analytic::resource_storage::visit_table(
                revision.session().shared_contexts(),
                ledger,
                role,
            )?;
            for track in revision.arrangement().tracks() {
                if let Some(instrument) = &track.instrument {
                    visit_arrangement_instrument(instrument, ledger, role)?;
                }
            }
            revision
                .session()
                .visit_midi_templates_on_control(ledger, role)
        })
    }
}
