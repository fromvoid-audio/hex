use super::*;
impl AudioPreparationCustody {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Self {
            root: _,
            source_context: _,
            incarnation: _,
            phase: _,
            bank,
            current_sampler,
            slots,
            next_disposal: _,
            first_panic,
            capacity,
            reason: _,
            session,
            structure,
            tempo,
            shared,
            assembly,
            arrangement_clips,
            revision,
            #[cfg(test)]
                fail_after: _,
            #[cfg(test)]
                panic_after: _,
        } = self;
        if first_panic.is_some() {
            l.unknown()?;
        }
        if let Some(bank) = bank {
            l.inputs(bank)?;
        }
        current_sampler.visit(l)?;
        let bank = bank.as_ref().or_else(|| current_sampler.bank());
        l.vec(slots)?;
        l.growth(capacity.previous_slot_backing_bytes)?;
        l.growth(capacity.native_compiler_workspace_bytes)?;
        for SourceSlot {
            active: _,
            track: _,
            clip: _,
            source: _,
            handle: _,
            start: _,
            end: _,
            source_offset: _,
            source_length: _,
            phase: _,
            gain: _,
            looping: _,
            fade: _,
            owner,
            prepared_origin,
            sparse_frames: _,
            sparse,
            transformed: _,
            disposal: _,
            #[cfg(test)]
                probe: _,
        } in slots
        {
            if let Some(map) = sparse {
                l.sparse_map(map)?;
            }
            if let Some(origin) = prepared_origin {
                l.source(origin)?;
            }
            if let Some(owner) = owner {
                l.source(owner)?;
            }
        }
        let view = revision.visit_audio_storage(l)?;
        if view.arrangement.is_some() {
            if let Some(bank) = bank {
                arrangement_clips.validate_root_sources_at(slots, bank, view.arrangement)?;
                assembly.validate_pairs_at(bank, view.session)?;
            } else {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        session.visit_audio_storage(l)?;
        structure.visit_audio_storage(l)?;
        tempo.visit_audio_storage(l)?;
        assembly.visit_audio_storage(l, view.session)?;
        let contexts = arrangement_clips.visit_audio_storage(l, view.arrangement, view.preview)?;
        shared.visit_audio_storage(l, contexts, assembly.shared_storage_at(view.session)?)?;
        Ok(())
    }
}
