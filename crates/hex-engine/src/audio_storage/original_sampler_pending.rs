//! Selected model fields under the actual Editor's single held pending guard.
//! No prefix approval, mutation, root origin or retirement is established here.
use super::*;
use crate::sampler_host_fields::BodyRef;
use hex_project::{Command, Editor, OriginalSamplerEditorTransition, OriginalSamplerPending};

/// Borrowed actual pair from the privately constructed, still-held model guard.
/// This projection contains no guard, owner, approval or caller byte declaration.
pub struct OriginalSamplerPendingModelFields<'a> {
    editor: &'a Editor,
    pending: &'a OriginalSamplerPending,
}
impl<'a> OriginalSamplerPendingModelFields<'a> {
    pub fn reborrow(&self) -> OriginalSamplerPendingModelFields<'_> {
        OriginalSamplerPendingModelFields {
            editor: self.editor,
            pending: self.pending,
        }
    }
    pub fn from_transition(held: &'a OriginalSamplerEditorTransition<'_>) -> Self {
        Self {
            editor: held.editor(),
            pending: held.storage(),
        }
    }
    pub(crate) fn editor(&self) -> &'a Editor {
        self.editor
    }
}

impl<'a> AudioRootStorageLedger<'a> {
    /// The enclosing Editor/Session and guard inline bodies must be charged by
    /// their actual enclosing host. This walks their selected model descendants
    /// without a second RefCell borrow, clone or allocated projection list.
    /// The nominal guard privately pairs the actual Editor and pending field;
    /// footprint containment alone would not establish that association.
    ///
    /// ```compile_fail
    /// use hex_engine::{AudioRootStorageLedger, AudioStorageLedgerReservation};
    /// use hex_project::{Editor, OriginalSamplerTransitionRequest, Project};
    /// let editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    /// let mut request = Some(OriginalSamplerTransitionRequest::undo());
    /// let mut ledger = AudioRootStorageLedger::try_new_on_control(
    ///     AudioStorageLedgerReservation {
    ///         data_rows: 256, handle_rows: 128, root_rows: 8,
    ///         max_retained_bytes: 1_000_000,
    ///         max_construction_peak_bytes: 1_000_000,
    ///     }).unwrap();
    /// {
    ///     let held = editor.original_sampler_transition_on_control(&mut request).unwrap();
    ///     ledger.borrow_original_sampler_pending_model_fields_on_control(&held).unwrap();
    /// }
    /// let _ = ledger.summary(); // a borrowed pending record cannot outlive held
    /// ```
    pub fn borrow_original_sampler_pending_model_fields_on_control(
        &mut self,
        transition: &'a OriginalSamplerEditorTransition<'_>,
    ) -> Result<(), AudioStorageError> {
        self.pending_model_fields(OriginalSamplerPendingModelFields::from_transition(
            transition,
        ))
    }
    pub fn borrow_original_sampler_pending_model_projection_on_control(
        &mut self,
        fields: &'a OriginalSamplerPendingModelFields<'_>,
    ) -> Result<(), AudioStorageError> {
        self.pending_model_fields(fields.reborrow())
    }
    pub(crate) fn pending_model_fields(
        &mut self,
        fields: OriginalSamplerPendingModelFields<'a>,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| {
            let OriginalSamplerPendingModelFields { editor, pending } = fields;
            if !pending.inspectable() || !BodyRef::of(editor).contains(BodyRef::of(pending)) {
                return Err(AudioStorageError::UnknownStorage);
            }
            ledger.editor_history_buffers(editor)?;
            if let Some(values) = pending.undo_replacement() {
                ledger.project_deque(values)?;
            }
            if let Some(values) = pending.redo_replacement() {
                ledger.vec(values)?;
            }
            if let Some(values) = pending.retired_redo() {
                ledger.vec(values)?;
            }
            // These command classes have only scalar fields or the ONE actual
            // named String child. All other command payloads remain unknown,
            // including states, time ranges and vectors: body is not absence.
            if let Some(command) = pending.request().and_then(|r| r.command_value()) {
                match command {
                    Command::AddTrack { name } | Command::AddMidiTrack { name } => {
                        ledger.bytes(
                            Owner::exclusive(name),
                            Location("model-string-buffer", 0),
                            name.capacity(),
                        )?;
                    }
                    Command::Gain { track: _, gain: _ }
                    | Command::MasterGain { gain: _ }
                    | Command::SetLoopRegion {
                        start: _,
                        end: _,
                        enabled: _,
                    }
                    | Command::ClearLoopRegion
                    | Command::ToggleLoopRegion => {}
                    _ => return Err(AudioStorageError::UnknownStorage),
                }
            }
            for project in editor
                .retained_projects_on_control()
                .chain(pending.candidate())
                .chain(pending.retired())
                .chain(pending.retired_redo().into_iter().flat_map(|v| v.iter()))
                .chain(
                    pending
                        .undo_replacement()
                        .into_iter()
                        .flat_map(|v| v.iter()),
                )
                .chain(
                    pending
                        .redo_replacement()
                        .into_iter()
                        .flat_map(|v| v.iter()),
                )
            {
                ledger.project_exclusive_buffers(project)?;
                ledger.project_midi_sources(project)?;
                ledger.project_native_states(project)?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
