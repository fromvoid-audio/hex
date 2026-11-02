//! Paired empty analytic children of a constructor-owned mutable event template.
use super::*;
use crate::{
    AudioRootStorageLedger, AudioStorageError as Error,
    midi_note_bank::PreparedMidiNoteBank,
    owned_shared::{OwnedShared, SharedDeclaration},
    shared_analytic::SharedAnalyticContextTable,
};
use std::mem::size_of;
pub(crate) struct CurrentSamplerEventCustody {
    bank: SharedDeclaration,
    contexts: SharedDeclaration,
    handles: [u64; 2],
}
impl PreparedMidiInstrumentTemplate {
    pub(crate) fn current_sampler_context_address(&self) -> usize {
        std::sync::Arc::as_ptr(&self.shared_contexts) as usize
    }
    pub(crate) fn current_sampler_quote(notes: usize) -> Result<(usize, usize), Error> {
        let (payload, overhead) =
            OwnedShared::<Self>::declared_parts().ok_or(Error::UnknownStorage)?;
        let (bank, bh) =
            OwnedShared::<PreparedMidiNoteBank>::declared_parts().ok_or(Error::UnknownStorage)?;
        let retained = notes
            .checked_mul(2 * size_of::<VoiceSpan>() + 2 * size_of::<IntervalNode>())
            .and_then(|n| n.checked_add(payload))
            .and_then(|n| n.checked_add(overhead))
            .and_then(|n| n.checked_add(bank))
            .and_then(|n| n.checked_add(bh))
            .ok_or(Error::Overflow)?;
        // Density phases are sequential. Sum their full possible allocations,
        // including the bounded first-tail occurrence sweep, conservatively.
        let scratch = density::current_sampler_workspace_quote(notes)?;
        Ok((retained, scratch))
    }
    pub(crate) fn prepare_current_sampler_events(
        source: &PreparedMidiSourceTemplate,
        limits: crate::midi_compile::MutableInstrumentLimits,
        contexts: &OwnedShared<SharedAnalyticContextTable>,
    ) -> Result<(Self, CurrentSamplerEventCustody), Error> {
        contexts.require_empty_owned_storage()?;
        let handles = [
            crate::audio_only_inputs::handle()?,
            crate::audio_only_inputs::handle()?,
        ];
        let (bank, bank_declaration) = OwnedShared::new_owned(PreparedMidiNoteBank::empty_value())
            .into_paired_parts_on_control();
        let (contexts, context_declaration) =
            contexts.clone_on_control().into_paired_parts_on_control();
        let value = Self::prepare_with_bank_mode(
            source,
            bank,
            contexts,
            Some(limits.max_release_tail_frames),
            true,
            Some(limits),
        )
        .map_err(|_| Error::UnsupportedGraph)?;
        Ok((
            value,
            CurrentSamplerEventCustody {
                bank: bank_declaration,
                contexts: context_declaration,
                handles,
            },
        ))
    }
    pub(crate) fn current_sampler_bytes(
        &self,
        custody: &CurrentSamplerEventCustody,
    ) -> Result<usize, Error> {
        let Self {
            track: _,
            clip: _,
            sample_rate: _,
            admitted_max_micro_bpm: _,
            first_pass: _,
            intro_iteration: _,
            repeat_iteration: _,
            period: _,
            release_raw: _,
            intro,
            repeats,
            intro_nodes,
            repeat_nodes,
            intro_root: _,
            repeat_root: _,
            max_active_voices: _,
            admitted_voices: _,
            max_query_onsets: _,
            max_span_hits: _,
            max_query_visits: _,
            max_period_windows: _,
            bank,
            shared_contexts,
            release_frames: _,
            event_source,
        } = self;
        if !event_source {
            return Err(Error::UnsupportedGraph);
        }
        bank.require_empty_current_sampler()?;
        shared_contexts.require_empty_owned_storage()?;
        let (payload, overhead) = custody.bank.parts(bank).ok_or(Error::UnknownStorage)?;
        custody
            .contexts
            .parts(shared_contexts)
            .ok_or(Error::UnknownStorage)?;
        [
            intro.capacity().checked_mul(size_of::<VoiceSpan>()),
            repeats.capacity().checked_mul(size_of::<VoiceSpan>()),
            intro_nodes
                .capacity()
                .checked_mul(size_of::<IntervalNode>()),
            repeat_nodes
                .capacity()
                .checked_mul(size_of::<IntervalNode>()),
            Some(payload),
            Some(overhead),
        ]
        .into_iter()
        .try_fold(0usize, |n, b| {
            n.checked_add(b.ok_or(Error::Overflow)?)
                .ok_or(Error::Overflow)
        })
    }
    pub(crate) fn visit_current_sampler<'a>(
        &'a self,
        custody: &'a CurrentSamplerEventCustody,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), Error> {
        use crate::audio_storage::Owner;
        self.current_sampler_bytes(custody)?;
        ledger.vec(&self.intro)?;
        ledger.vec(&self.repeats)?;
        ledger.vec(&self.intro_nodes)?;
        ledger.vec(&self.repeat_nodes)?;
        ledger.shared(
            &self.bank,
            custody.bank.parts(&self.bank),
            Owner::exclusive(&self.bank),
            custody.handles[0],
            None,
        )?;
        ledger.shared(
            &self.shared_contexts,
            custody.contexts.parts(&self.shared_contexts),
            Owner::exclusive(&self.shared_contexts),
            custody.handles[1],
            None,
        )
    }
}
