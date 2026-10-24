//! Actual constructor-owned source vectors; no source owner or scheduler is added.
use super::*;
use crate::{AudioRootStorageLedger, AudioStorageError as Error, owned_shared::OwnedShared};
use std::mem::size_of;
impl PreparedMidiSourceTemplate {
    pub(crate) fn current_sampler_quote(notes: usize) -> Result<(usize, usize), Error> {
        let vectors = notes
            .checked_mul(4 * size_of::<SourceEdge>() + 2 * size_of::<SourceSpan>())
            .ok_or(Error::Overflow)?;
        let (payload, overhead) =
            OwnedShared::<Self>::declared_parts().ok_or(Error::UnknownStorage)?;
        // prepare_source reserves two edge arrays of2N and two span arrays ofN.
        // The two temporary density arrays coexist with them; unstable sort is in place.
        let scratch = notes
            .checked_mul(4 * size_of::<DensityEvent>())
            .ok_or(Error::Overflow)?;
        Ok((
            vectors
                .checked_add(payload)
                .and_then(|n| n.checked_add(overhead))
                .ok_or(Error::Overflow)?,
            scratch,
        ))
    }
    pub(crate) fn current_sampler_bytes(&self) -> Result<usize, Error> {
        let Self {
            track: _,
            clip: _,
            sample_rate: _,
            admitted_max_micro_bpm: _,
            first_pass: _,
            intro_iteration: _,
            repeat_iteration: _,
            period: _,
            source: _,
            intro,
            repeats,
            intro_spans,
            repeat_spans,
            density: _,
        } = self;
        [
            intro.capacity().checked_mul(size_of::<SourceEdge>()),
            repeats.capacity().checked_mul(size_of::<SourceEdge>()),
            intro_spans.capacity().checked_mul(size_of::<SourceSpan>()),
            repeat_spans.capacity().checked_mul(size_of::<SourceSpan>()),
        ]
        .into_iter()
        .try_fold(0usize, |n, b| {
            n.checked_add(b.ok_or(Error::Overflow)?)
                .ok_or(Error::Overflow)
        })
    }
    pub(crate) fn visit_current_sampler<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), Error> {
        self.current_sampler_bytes()?;
        ledger.vec(&self.intro)?;
        ledger.vec(&self.repeats)?;
        ledger.vec(&self.intro_spans)?;
        ledger.vec(&self.repeat_spans)
    }
}
