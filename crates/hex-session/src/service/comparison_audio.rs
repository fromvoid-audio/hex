//! Ordered authoring correlation retaining the ACTUAL captured source field.
//! This is neither native admission nor a replacement for physical inventory.
use super::*;

#[derive(Clone)]
pub struct ComparisonAudioBinding {
    pub(super) source: SharedCanonicalAudioSource,
    previous_revision: u64,
    revision: u64,
}
impl ComparisonAudioBinding {
    pub(super) fn advance(
        previous: &ViewState,
        generation: u64,
        revision: u64,
        session: &Session,
    ) -> Option<Self> {
        let source = previous.canonical_audio_source.as_ref()?;
        if source.context().project_generation != generation
            || previous.revision.checked_add(1) != Some(revision)
            || previous.audio_authoring_revision(source.context()) != Some(previous.revision)
            || !source.matches_session_sources(session)
        {
            return None;
        }
        Some(Self {
            source: source.clone(),
            previous_revision: previous.revision,
            revision,
        })
    }
    pub fn follows(&self, previous: &ViewState, current: &ViewState, generation: u64) -> bool {
        self.previous_revision == previous.revision
            && self.revision == current.revision
            && self.source.context().project_generation == generation
            && previous
                .canonical_audio_source
                .as_ref()
                .is_some_and(|source| source.shares_owner_with(&self.source))
            && current
                .canonical_audio_source
                .as_ref()
                .is_some_and(|source| source.shares_owner_with(&self.source))
            && previous.audio_authoring_revision(self.source.context()) == Some(previous.revision)
            && current.audio_authoring_revision(self.source.context()) == Some(current.revision)
    }
    pub(crate) fn sampler_source_fields_on_control(
        &self,
    ) -> hex_engine::sampler_host_fields::CapturedSourceFields<'_> {
        let Self {
            source,
            previous_revision: _,
            revision: _,
        } = self;
        canonical_audio_source::sampler_fields_on_control(source)
    }
}
impl ViewState {
    pub fn audio_authoring_revision(&self, context: hex_engine::AudioSourceContext) -> Option<u64> {
        let source = self.canonical_audio_source.as_ref()?;
        if source.context() != context {
            return None;
        }
        if context.editor_revision == self.revision {
            return Some(self.revision);
        }
        self.comparison_audio
            .as_ref()
            .filter(|binding| {
                binding.revision == self.revision && binding.source.shares_owner_with(source)
            })
            .map(|_| self.revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_exports::{
        ExportRecordError, SessionExportFamily, SnapshotExportReservation,
    };
    use hex_project::{Command, Created};

    fn reservation() -> SnapshotExportReservation {
        SnapshotExportReservation {
            snapshot_records: 2,
            read_records: 4,
            max_bookkeeping_bytes: 16_384,
        }
    }

    #[test]
    fn ordered_source_reads_retain_actual_capture_after_history_pruning_and_refuse_foreign_reads() {
        let mut session = Session::new(48_000, 1, 1024).unwrap();
        let Created::Track(track) = session
            .editor
            .execute(Command::AddTrack { name: "old".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let old = view_state_owned(
            &session,
            hex_engine::empty_midi_preview_on_control(),
            7,
            1,
            false,
            None,
        );
        session
            .editor
            .execute(Command::RenameTrack {
                track,
                name: "metadata".into(),
            })
            .unwrap();
        // This tests the exact private bound-constructor branch. Production
        // edit classification and admission remain separate required work.
        let current = view_state_owned_bound(
            &session,
            hex_engine::empty_midi_preview_on_control(),
            8,
            1,
            true,
            None,
            Some(&old),
        );
        assert!(
            current
                .comparison_audio
                .as_ref()
                .unwrap()
                .follows(&old, &current, 1)
        );
        assert!(
            !current
                .comparison_audio
                .as_ref()
                .unwrap()
                .follows(&current, &old, 1)
        );
        assert!(
            !current
                .comparison_audio
                .as_ref()
                .unwrap()
                .follows(&old, &current, 2)
        );
        let unrelated = view_state_owned(
            &session,
            hex_engine::empty_midi_preview_on_control(),
            7,
            1,
            false,
            None,
        );
        assert!(
            !current
                .comparison_audio
                .as_ref()
                .unwrap()
                .follows(&unrelated, &current, 1)
        );
        drop(unrelated);
        session.editor.execute(Command::RemoveTrack(track)).unwrap();
        session
            .editor
            .execute(Command::AddTrack { name: "new".into() })
            .unwrap();
        assert!(
            session
                .editor
                .retained_projects_on_control()
                .all(|p| p.track(track).is_none())
        );
        let captured = current.comparison_audio.as_ref().unwrap();
        assert!(
            captured
                .source
                .project_owner_on_control()
                .track(track)
                .is_some()
        );
        assert!(
            captured
                .source
                .shares_owner_with(old.canonical_audio_source.as_ref().unwrap())
        );
        drop(session);

        let ledger_budget = hex_engine::AudioStorageLedgerReservation {
            data_rows: 1024,
            handle_rows: 1024,
            root_rows: 16,
            max_retained_bytes: 4_000_000,
            max_construction_peak_bytes: 4_000_000,
        };
        // Each of old canonical/new canonical/new comparison is an actual
        // owning handle. Shared allocation bodies deduplicate; handles do not.
        let omitted = current.comparison_audio.as_ref().unwrap().clone();
        {
            let mut ledger =
                hex_engine::AudioRootStorageLedger::try_new_on_control(ledger_budget).unwrap();
            old.visit_children_selected_storage_on_control(&mut ledger)
                .unwrap();
            current
                .visit_children_selected_storage_on_control(&mut ledger)
                .unwrap();
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
        }
        drop(omitted);
        {
            let mut ledger =
                hex_engine::AudioRootStorageLedger::try_new_on_control(ledger_budget).unwrap();
            old.visit_children_selected_storage_on_control(&mut ledger)
                .unwrap();
            current
                .visit_children_selected_storage_on_control(&mut ledger)
                .unwrap();
            ledger
                .validate_selected_shared_handles_on_control()
                .unwrap();
            assert!(ledger.summary().unwrap().require_complete().is_err());
        }

        let (mut family, key) = SessionExportFamily::try_new_on_control(reservation()).unwrap();
        let old = family
            .insert_event_read_on_control(&key, &mut Some(old))
            .unwrap();
        let current = family
            .insert_event_read_on_control(&key, &mut Some(current))
            .unwrap();
        assert_eq!(family.follows_source(&current, &old, 1), Ok(true));
        assert_eq!(family.follows_source(&old, &current, 1), Ok(false));
        assert_eq!(family.follows_source(&current, &old, 2), Ok(false));
        let (foreign, foreign_key) =
            SessionExportFamily::try_new_on_control(reservation()).unwrap();
        let foreign_view = view_state_owned(
            &Session::new(48_000, 1, 1024).unwrap(),
            hex_engine::empty_midi_preview_on_control(),
            7,
            1,
            false,
            None,
        );
        let foreign_read = foreign
            .insert_event_read_on_control(&foreign_key, &mut Some(foreign_view))
            .unwrap();
        assert_eq!(
            family.follows_source(&current, &foreign_read, 1),
            Err(ExportRecordError::Stale)
        );
        foreign.release_read(foreign_read).unwrap();
        family.release_read(old).unwrap();
        family.release_read(current).unwrap();
        assert_eq!(
            family.cleanup_one_on_control(),
            Ok(crate::snapshot_exports::SnapshotFamilyCleanup::Pending)
        );
    }
}
