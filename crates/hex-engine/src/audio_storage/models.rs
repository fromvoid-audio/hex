//! Selected canonical model-domain data and actual owning fields. No raw Arc
//! promotion, enclosing model/container bytes or construction/custody proof.
use super::*;
use hex_project::{MidiClip, MidiSourceIdentity, MidiSourceStorageObservation, SessionMidiClip};
use std::{alloc::Layout, sync::atomic::AtomicUsize};

fn observation(value: &dyn Any) -> Option<MidiSourceStorageObservation<'_>> {
    if let Some(clip) = value.downcast_ref::<MidiClip>() {
        return Some(clip.source_storage_on_control());
    }
    if let Some(clip) = value.downcast_ref::<SessionMidiClip>() {
        return Some(clip.source_storage_on_control());
    }
    value
        .downcast_ref::<MidiSourceIdentity>()
        .map(|id| id.source_storage_on_control())
}
fn source(value: &dyn Any) -> Owner<'_> {
    Owner {
        domain: OwnerDomain::CanonicalMidiSource,
        value,
        compare: |a, b| {
            observation(a)
                .zip(observation(b))
                .is_some_and(|(a, b)| a.shares_source_with(&b))
        },
        counts: Some(|value| observation(value).map(|view| view.owning_counts())),
    }
}
fn field(value: &dyn Any) -> Owner<'_> {
    Owner {
        domain: OwnerDomain::CanonicalMidiField,
        value,
        compare: |a, b| {
            observation(a)
                .zip(observation(b))
                .is_some_and(|(a, b)| a.is_same_owning_field_as(&b))
        },
        counts: None,
    }
}
pub(crate) fn native_layout_known() -> bool {
    let target = env!("HEX_MIDI_ARC_LAYOUT_TARGET");
    env!("HEX_MIDI_ARC_LAYOUT_KNOWN") == "1"
        && env!("HEX_MIDI_ARC_CONSTRUCTOR_FAMILY") == "canonical-midi-sized-arc-new-make-mut-v1"
        && ((cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )) && target == "x86_64-unknown-linux-gnu")
            || (cfg!(all(target_arch = "aarch64", target_os = "macos"))
                && target == "aarch64-apple-darwin"))
}
/// Declared native layout support for the audited canonical MIDI new/COW
/// family only. This is not whole-model, custody or retirement qualification.
pub fn canonical_midi_storage_layout_supported() -> bool {
    native_layout_known()
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Actual Undo/Redo container backing only, including empty reserved slots.
    /// Inline Project values are in these entries; their owned children require
    /// separate actual traversal. Editor/current body, custody and construction
    /// peak are not established by these selected observations.
    pub fn borrow_editor_history_buffers_on_control(
        &mut self,
        editor: &'a hex_project::Editor,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            let undo = editor.undo_storage_on_control();
            ledger.allocation(
                Owner::exclusive(undo),
                Location("vec-deque-buffer", 0),
                undo.capacity(),
                std::mem::size_of::<hex_project::Project>(),
            )?;
            ledger.vec(editor.redo_storage_on_control())
        })
    }
    /// Actual asset-container backing and each live path backing only. The
    /// inline container/path headers belong to their enclosing Project/entry.
    /// This is not complete Project storage, custody or construction-peak proof.
    pub fn borrow_project_asset_storage_on_control(
        &mut self,
        project: &'a hex_project::Project,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| ledger.project_asset_storage(project))
    }
    pub(super) fn project_asset_storage(
        &mut self,
        project: &'a hex_project::Project,
    ) -> Result<(), AudioStorageError> {
        let entries = project.asset_entries_on_control();
        self.entries(entries)?;
        for asset in entries.values() {
            let path = asset.path_buffer_on_control();
            self.bytes(
                Owner::exclusive(path),
                Location("project-asset-path", 0),
                path.capacity(),
            )?;
        }
        Ok(())
    }
    /// Borrow the actual current/Undo/Redo asset containers and paths, without
    /// cloning snapshots or inventing owners. Candidate Projects join through
    /// borrow_project_asset_storage_on_control while held by the caller.
    pub fn borrow_editor_asset_storage_on_control(
        &mut self,
        editor: &'a hex_project::Editor,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            for project in editor.retained_projects_on_control() {
                ledger.project_asset_storage(project)?;
            }
            Ok(())
        })
    }
    pub(crate) fn model_source_if_unvisited(
        &mut self,
        actual_field: &'a dyn Any,
    ) -> Result<(), AudioStorageError> {
        let candidate = field(actual_field);
        if self.handles.iter().any(|handle| {
            handle.field.same(candidate) && handle.location == Location("canonical-midi-field", 0)
        }) {
            return Ok(());
        }
        self.model_source(actual_field)
    }
    pub(crate) fn model_source(
        &mut self,
        actual_field: &'a dyn Any,
    ) -> Result<(), AudioStorageError> {
        if !native_layout_known() {
            return Err(AudioStorageError::UnknownStorage);
        }
        let view = observation(actual_field).ok_or(AudioStorageError::UnknownStorage)?;
        if view.owning_counts().1 != 0 {
            return Err(AudioStorageError::UnknownStorage);
        }
        let body = view.body_layout();
        let requested = Layout::array::<AtomicUsize>(2)
            .map_err(|_| AudioStorageError::Overflow)?
            .extend(body)
            .map_err(|_| AudioStorageError::Overflow)?
            .0
            .pad_to_align();
        let owner = source(actual_field);
        self.bytes(owner, Location("canonical-midi-body", 0), requested.size())?;
        self.allocation(
            owner,
            Location("canonical-midi-notes", 0),
            view.notes_capacity(),
            view.note_element_layout().size(),
        )?;
        // Inline Arc handle belongs to its enclosing model/compiled object.
        self.handle_at(
            owner,
            field(actual_field),
            Location("canonical-midi-field", 0),
            crate::audio_only_inputs::handle()?,
        )
    }
    /// Selected actual source body/backing/owning field. Enclosing object and
    /// inline Arc handle are charged by their enclosing storage visitor only.
    pub fn borrow_midi_clip_source_on_control(
        &mut self,
        clip: &'a MidiClip,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| ledger.model_source(clip))
    }
    pub fn borrow_session_midi_source_on_control(
        &mut self,
        clip: &'a SessionMidiClip,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| ledger.model_source(clip))
    }
    pub fn borrow_midi_source_identity_on_control(
        &mut self,
        identity: &'a MidiSourceIdentity,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| ledger.model_source(identity))
    }
    /// Actual project sources only. No custody,
    /// construction peak, enclosing model storage or retirement is established.
    pub fn borrow_project_midi_sources_on_control(
        &mut self,
        project: &'a hex_project::Project,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| ledger.project_midi_sources(project))
    }
    fn project_midi_sources(
        &mut self,
        project: &'a hex_project::Project,
    ) -> Result<(), AudioStorageError> {
        for track in project.tracks() {
            for clip in track.midi_clips() {
                self.model_source(clip)?;
            }
        }
        for cell in project.session_cells() {
            if let hex_project::SessionCellContent::Midi(clip) = cell.content() {
                self.model_source(clip)?;
            }
        }
        Ok(())
    }
    /// Actual current/Undo/Redo fields, never synthesized source identities.
    /// Selected source bytes exclude enclosing history/project storage.
    pub fn borrow_editor_midi_sources_on_control(
        &mut self,
        editor: &'a hex_project::Editor,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            for project in editor.retained_projects_on_control() {
                ledger.project_midi_sources(project)?;
            }
            Ok(())
        })
    }
    /// Actual retained Session/routed identity fields in this prepared revision.
    pub fn borrow_revision_midi_sources_on_control(
        &mut self,
        revision: &'a crate::PreparedRevision,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            revision.session().visit_model_sources_on_control(ledger)?;
            revision.visit_model_sources_on_control(ledger)
        })
    }
}
#[cfg(test)]
mod tests;
