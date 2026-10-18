//! Canonical/preview model outer ownership through the existing paired facade.
use hex_engine::{AudioRootStorageLedger, AudioStorageError};

/// Fresh owned Project values enter once; handle clones retain constructor
/// pairing. No raw Arc/Weak adoption or mutation. Children/peak remain separate.
///
/// ```compile_fail
/// let raw = std::sync::Arc::new(hex_project::Project::new(48_000).unwrap());
/// let _: hex_session::SharedProject = raw.into();
/// ```
/// ```compile_fail
/// let mut owner = hex_session::SharedProject::new_on_control(hex_project::Project::new(48_000).unwrap());
/// *owner = hex_project::Project::new(44_100).unwrap();
/// ```
pub type SharedProject = hex_engine::SharedMetadata<hex_project::Project>;

/// Selected actual outer body and currently qualified nested model storage.
/// Import provenance and edited-audio shared domains are not qualified by this
/// facade; refuse them rather than marking their unseen children known. This
/// supplies neither enclosing view/preview roots nor construction/custody,
/// publication/reaper proof. Every other live model owner still needs traversal.
pub fn visit_shared_project_selected_storage_on_control<'a>(
    owner: &'a SharedProject,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    ledger.borrow_shared_metadata_body_on_control(owner)?;
    if owner.import_provenance().is_some()
        || owner
            .tracks()
            .iter()
            .any(|track| track.clips().iter().any(|clip| clip.audio_edit().is_some()))
    {
        return Err(AudioStorageError::UnknownStorage);
    }
    ledger.borrow_project_exclusive_buffers_on_control(owner)?;
    ledger.borrow_project_midi_sources_on_control(owner)?;
    ledger.borrow_project_native_states_on_control(owner)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reservation() -> hex_engine::AudioStorageLedgerReservation {
        hex_engine::AudioStorageLedgerReservation {
            data_rows: 128,
            handle_rows: 128,
            root_rows: 16,
            max_retained_bytes: 2_000_000,
            max_construction_peak_bytes: 2_000_000,
        }
    }
    #[test]
    fn paired_project_equal_contents_remain_separate_and_unknown_children_refuse() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let project = hex_project::Project::new(48_000).unwrap();
        let first = SharedProject::new_on_control(project.clone());
        let second = SharedProject::new_on_control(project);
        assert_eq!(first, second); // value equality never substitutes for allocation identity
        assert!(!first.shares_owner_with(&second));
        let alias = first.clone();
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        ledger
            .borrow_shared_metadata_body_on_control(&first)
            .unwrap();
        let body = ledger.summary().unwrap().data_bytes;
        assert!(body > std::mem::size_of::<hex_project::Project>());
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        );
        ledger
            .borrow_shared_metadata_body_on_control(&alias)
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        ledger
            .borrow_shared_metadata_body_on_control(&second)
            .unwrap();
        assert_eq!(ledger.summary().unwrap().data_bytes, body * 2);
        assert_eq!(ledger.selected_shared_counts_on_control(), (2, 3));
        let complete = ledger.summary().unwrap();
        assert!(complete.require_complete().is_err());
        drop(ledger);
        let mut snapshot = hex_project::Project::new(48_000).unwrap().snapshot();
        snapshot.import_provenance =
            Some(hex_project::ImportProvenance::new("ableton-live-set", 1, 1, vec![1]).unwrap());
        let unsupported = hex_project::Project::from_snapshot(snapshot).unwrap();
        let unsupported = SharedProject::new_on_control(unsupported);
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        ledger
            .borrow_shared_metadata_body_on_control(&unsupported)
            .unwrap();
        assert_eq!(
            visit_shared_project_selected_storage_on_control(&unsupported, &mut ledger),
            Err(AudioStorageError::UnknownStorage)
        );
    }
    #[test]
    fn two_paired_aliases_visit_one_actual_model_child_field_without_hiding_a_distinct_clone() {
        if !hex_engine::audio_storage_constructor_layout_supported()
            || !hex_engine::canonical_midi_storage_layout_supported()
            || !hex_engine::canonical_native_state_storage_layout_supported()
        {
            return;
        }
        eprintln!("executing paired Project/MIDI/state nested actual field dedup path");
        use hex_project::{BeatTime, Command, Created, Project};
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap();
        let project = editor.project().clone();
        drop(editor); // retain only the actual model that enters the paired outer owner
        let owner = SharedProject::new_on_control(project);
        let alias = owner.clone();
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        visit_shared_project_selected_storage_on_control(&owner, &mut ledger).unwrap();
        let first = ledger.summary().unwrap();
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        ); // paired outer alias omitted
        visit_shared_project_selected_storage_on_control(&alias, &mut ledger).unwrap();
        assert_eq!(ledger.summary().unwrap().data_bytes, first.data_bytes);
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert_eq!(ledger.selected_shared_counts_on_control(), (3, 4));
        drop(ledger);
        let separate = SharedProject::new_on_control(owner.as_ref().clone());
        let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation()).unwrap();
        visit_shared_project_selected_storage_on_control(&owner, &mut ledger).unwrap();
        visit_shared_project_selected_storage_on_control(&alias, &mut ledger).unwrap();
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        ); // separate model owns real child aliases
        visit_shared_project_selected_storage_on_control(&separate, &mut ledger).unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert!(ledger.summary().unwrap().data_bytes > first.data_bytes);
        assert_eq!(ledger.selected_shared_counts_on_control(), (4, 7));
        assert!(ledger.summary().unwrap().require_complete().is_err());
    }
}
