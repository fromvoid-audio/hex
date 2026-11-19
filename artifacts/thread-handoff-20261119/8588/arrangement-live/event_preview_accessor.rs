// Insert in audio_event_editor/mod.rs's existing impl EditorView.
    /// The existing admitted/held event fade also drives its Arrangement thumbnail.
    /// Pending custody is scoped by the same owner as the editor's paint path.
    pub(in crate::editor) fn arrangement_fade_event_preview(
        &self,
        clip_id: ClipId,
        state: &hex_session::service::ViewState,
    ) -> Option<hex_project::AudioEvents> {
        let owner = (clip_id, self.project_generation);
        if let Some(pending) = self.audio_event_editor.pending.as_ref()
            .filter(|pending| pending.owner == owner && matches!(pending.operation, AudioEventEdit::SetFade { .. }))
        {
            return Some(pending.preview.clone());
        }
        if self.audio_event_editor.owner != Some(owner) || self.audio_event_editor.revision != state.revision {
            return None;
        }
        let operation = self.audio_event_editor.gesture.as_ref()?.operation.as_ref()
            .filter(|operation| matches!(operation, AudioEventEdit::SetFade { .. }))?;
        let clip = state.project.clip(clip_id)?;
        clip.editable_audio_events().ok()?.edited(operation.clone(), clip.audio_event_root().domain.clock.length()).ok()
    }
