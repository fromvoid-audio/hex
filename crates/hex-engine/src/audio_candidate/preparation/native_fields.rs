//! Exhaustive children of an actual unpublished collecting or freshly owned field.
use super::*;
impl AudioCandidatePreparation {
    pub(crate) fn require_original_sampler_constructor_custody(
        &self,
        custody: &crate::AudioPreparationCustody,
    ) -> Result<(), AudioStorageError> {
        if self.failed_incomplete()
            || self.cleaning
            || self.complete
            || !self.original_resource_attempted
            || self.original_resource_refused
            || self.bank.is_some()
            || self.token.is_some()
            || self.control_work.is_some()
            || self.owner_runtime_done
            || !self.sources.is_empty()
            || !self.resident_draft.is_empty()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        if let Some(owner) = &self.owner {
            if self.custody.is_some()
                || !std::ptr::eq(&owner.inner.custody, custody)
                || !custody.original_sampler_resources_retained()
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            owner.require_unpublished_native_host_class()?;
        } else if !self
            .custody
            .as_ref()
            .is_some_and(|actual| std::ptr::eq(actual, custody))
            || !custody.original_sampler_resources_installed()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    /// Only non-custody fields of the exact selected original-class cell.
    /// The SAME mandatory compiler Inputs visit `custody` once; this private
    /// constituent is neither a public omission switch nor completeness proof.
    pub(crate) fn visit_original_sampler_constructor_shell<'a>(
        &'a self,
        custody: &'a crate::AudioPreparationCustody,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        self.require_original_sampler_constructor_custody(custody)?;
        ledger.vec(&self.sources)?;
        ledger.vec(&self.resident_draft)?;
        let draft = self
            .draft
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?;
        if let Some(owner) = &self.owner {
            draft.visit_transferred_host_shell(ledger, owner)
        } else {
            draft.visit_unconstructed_host_children(ledger)
        }
    }
    pub(crate) fn visit_sampler_host_children<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Self {
            sources,
            resident_draft,
            reservation: _,
            bank,
            custody,
            token,
            draft,
            owner,
            control_work,
            owner_runtime_done,
            original_resource_attempted: _,
            original_resource_refused,
            cleaning,
            complete,
            failed,
            first_panic,
        } = self;
        if *failed
            || *cleaning
            || *complete
            || *original_resource_refused
            || first_panic.is_some()
            || control_work.is_some()
            || *owner_runtime_done
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        ledger.vec(sources)?;
        for (_, source) in sources {
            ledger.embedded_native_source(source)?;
        }
        ledger.vec(resident_draft)?;
        if let Some(owner) = owner {
            // The constructor moved these ACTUAL fields into this same cell's
            // owned CandidateState, not a caller-installed unrelated parent.
            if bank.is_some()
                || custody.is_some()
                || token.is_some()
                || !sources.is_empty()
                || !resident_draft.is_empty()
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            draft
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?
                .visit_transferred_host_shell(ledger, owner)?;
            return owner.visit_unpublished_native_host_fields(ledger);
        }
        // A partial compiled/draft phase is not an empty collecting class.
        if token.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        if let Some(bank) = bank {
            ledger.inputs(bank)?;
        }
        let custody = custody.as_ref().ok_or(AudioStorageError::SourceMismatch)?;
        if custody.phase() != AudioPreparationPhase::Vacant {
            return Err(AudioStorageError::UnknownStorage);
        }
        custody.visit_audio_storage(ledger, crate::AudioStorageRole::Current)?;
        draft
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?
            .visit_unconstructed_host_children(ledger)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation};
    use hex_project::{Command, Created, Editor};
    fn prepared() -> AudioCandidatePreparation {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "actual owned".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor.execute(Command::RemoveInstrument { track }).unwrap();
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: "actual.wav".into(),
                sample_rate: 48000,
                frames: 8,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: 8,
                source_offset: 0,
                source_length: 8,
                looping: false,
            })
            .unwrap();
        let mut cell = AudioCandidatePreparation::try_new_on_control(
            1,
            crate::AudioStorageReservation {
                allocation_items: 256,
                handle_obligations: 512,
                max_prepared_data_bytes: 64 * 1024 * 1024,
                max_construction_data_bytes: 64 * 1024 * 1024,
                caller_retained_bytes: 0,
                caller_construction_bytes: 0,
            },
        )
        .unwrap();
        cell.copy_resident_source_on_control(
            asset,
            &crate::Audio::new(48000, vec![[0.5; 2]; 8]).unwrap(),
        )
        .unwrap();
        cell.prepare_on_control(
            editor.project(),
            1,
            crate::AudioSourceContext {
                project_generation: 1,
                editor_revision: 4,
            },
        )
        .unwrap();
        cell
    }
    fn inspect(
        cell: &AudioCandidatePreparation,
    ) -> Result<crate::AudioRootStorageSummary, AudioStorageError> {
        let mut ledger =
            AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
                data_rows: 4096,
                handle_rows: 4096,
                root_rows: 8,
                max_retained_bytes: 64 * 1024 * 1024,
                max_construction_peak_bytes: 64 * 1024 * 1024,
            })
            .unwrap();
        ledger.root(cell, crate::AudioStorageRole::Current)?;
        cell.visit_sampler_host_children(&mut ledger)?;
        ledger.validate_selected_shared_handles_on_control()?;
        ledger.summary()
    }
    #[test]
    fn actual_same_parent_owned_graph_refuses_missing_or_foreign_draft_and_pending_owner() {
        let mut first = prepared();
        let mut second = prepared();
        let identity = first.owner.as_ref().unwrap().identity();
        assert!(inspect(&first).unwrap().data_bytes > 0);
        let draft = first.draft.take();
        assert_eq!(inspect(&first), Err(AudioStorageError::SourceMismatch));
        first.draft = draft;
        std::mem::swap(&mut first.draft, &mut second.draft);
        assert_eq!(inspect(&first), Err(AudioStorageError::SourceMismatch));
        assert_eq!(inspect(&second), Err(AudioStorageError::SourceMismatch));
        std::mem::swap(&mut first.draft, &mut second.draft);
        first.owner.as_mut().unwrap().inner.phase = AudioCandidateDisposalPhase::RuntimePending;
        assert_eq!(inspect(&first), Err(AudioStorageError::SourceMismatch));
        first.owner.as_mut().unwrap().inner.phase = AudioCandidateDisposalPhase::Owned;
        first.failed = true;
        assert_eq!(inspect(&first), Err(AudioStorageError::UnknownStorage));
        first.failed = false;
        assert_eq!(first.owner.as_ref().unwrap().identity(), identity);
        inspect(&first).unwrap();
        inspect(&second).unwrap();
        first.cleanup_on_control().unwrap();
        second.cleanup_on_control().unwrap();
        assert!(first.complete && second.complete);
    }
}
