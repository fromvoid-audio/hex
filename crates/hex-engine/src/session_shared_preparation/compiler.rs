//! Session's actual alias of the already constructed Arrangement context.
//! This does not fabricate the legacy empty-table transfer/restore lifecycle.
use super::*;
impl SessionSharedConstruction {
    pub(crate) fn compiler_construction_bytes() -> Result<usize, AudioStorageError> {
        let (body, header) = OwnedShared::<SineTable>::declared_parts()
            .ok_or(AudioStorageError::UnsupportedConstructor)?;
        body.checked_add(header).ok_or(AudioStorageError::Overflow)
    }
    pub(crate) fn construct_compiler_session(
        &mut self,
        contexts: &OwnedShared<SharedAnalyticContextTable>,
    ) -> Result<(), AudioStorageError> {
        if self.context_route != ContextRoute::LegacyEmpty
            || self.sine.is_some()
            || self.arrangement_contexts.is_some()
            || self.session_contexts.is_some()
            || self.handles != [0; 3]
            || self.disposal != [Disposal::Unstarted; 3]
            || self.next != 0
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        contexts.parts().ok_or(AudioStorageError::UnknownStorage)?;
        contexts.refuse_opaque_control_drop()?;
        Self::compiler_construction_bytes()?;
        // Actual physical alias identities are reserved before either birth
        // or clone. No alias escapes this persistent constructor destination.
        for id in &mut self.handles {
            *id = crate::audio_only_inputs::handle()?;
        }
        self.context_route = ContextRoute::CompilerSession;
        self.sine = Some(OwnedShared::new_owned(SineTable::new()));
        self.session_contexts = Some(contexts.clone_on_control());
        self.observe()?;
        Ok(())
    }
    pub(crate) fn validate_compiler_pair(
        &self,
        arrangement: &OwnedShared<SharedAnalyticContextTable>,
        inventory: InventorySharedView<'_>,
    ) -> Result<(), AudioStorageError> {
        let session = self
            .session_contexts
            .as_ref()
            .or(inventory.contexts)
            .ok_or(AudioStorageError::SourceMismatch)?;
        if self.context_route != ContextRoute::CompilerSession
            || self.arrangement_contexts.is_some()
            || !Arc::ptr_eq(arrangement.arc(), session.arc())
            || arrangement.parts().is_none()
            || arrangement.parts() != session.parts()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        // Pointer equality is only the SAME-field pairing check; mandatory
        // paired declarations and descendant visitors still establish storage.
        arrangement.refuse_opaque_control_drop()?;
        self.observe_at(Some(arrangement), inventory)?;
        Ok(())
    }
}
