//! Immutable canonical Project/counter pairing owned by the existing worker.
//! Correlation only: constructor custody and exact page protection remain separate.
use super::*;
pub struct CanonicalAudioSourceSnapshot {
    project: Arc<Project>,
    context: hex_engine::AudioSourceContext,
    mode: crate::DocumentAudioMode,
    sources: BTreeMap<hex_project::AssetId, crate::AudioSourceBinding>,
}
impl CanonicalAudioSourceSnapshot {
    pub(super) fn capture(
        project: &Arc<Project>,
        project_generation: u64,
        editor_revision: u64,
        session: &Session,
    ) -> Self {
        Self {
            project: Arc::clone(project),
            mode: session.audio_mode(),
            sources: session.source_bindings(),
            context: hex_engine::AudioSourceContext {
                project_generation,
                editor_revision,
            },
        }
    }
    pub fn audio_mode(&self) -> crate::DocumentAudioMode {
        self.mode
    }
    /// Immutable resolved bindings paired with this exact Project/context.
    /// Facts only: native publication must match actual retained source leases.
    pub fn sources(&self) -> &BTreeMap<hex_project::AssetId, crate::AudioSourceBinding> {
        &self.sources
    }
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        self.context
    }
    /// Borrow the exact captured Project and copy its paired canonical context.
    /// Catalog generation validates media keys and is not document generation.
    pub fn arrangement_input(
        &self,
        source_catalog_generation: u64,
        inputs: hex_engine::OwnedAudioInputs,
    ) -> hex_engine::AudioArrangementInput<'_> {
        hex_engine::AudioArrangementInput::new(&self.project, source_catalog_generation, inputs)
            .with_source_context(self.context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn next(service: &Service) -> Event {
        service
            .events
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
    }
    fn bank() -> hex_engine::OwnedAudioInputs {
        hex_engine::OwnedAudioInputs::try_new(
            vec![],
            hex_engine::AudioStorageReservation {
                allocation_items: 64,
                handle_obligations: 256,
                max_prepared_data_bytes: 64 * 1024 * 1024,
                max_construction_data_bytes: 64 * 1024 * 1024,
                caller_retained_bytes: 0,
                caller_construction_bytes: 0,
            },
        )
        .ok()
        .unwrap()
    }
    #[test]
    fn canonical_worker_captures_project_and_counters_together_and_preview_cannot_mint_context() {
        let service = Service::new(48000, 16, 0).unwrap();
        let initial = next(&service);
        assert_eq!(
            initial
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context(),
            hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: 0
            }
        );
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "audio".into(),
            }))
            .unwrap();
        let added = next(&service);
        let track = added.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::RemoveInstrument { track }))
            .unwrap();
        let captured = next(&service);
        assert!(captured.error.is_none());
        let snapshot = captured.state.canonical_audio_source.as_ref().unwrap();
        assert!(Arc::ptr_eq(&snapshot.project, &captured.state.project));
        assert_eq!(snapshot.context().editor_revision, captured.state.revision);
        assert_eq!(snapshot.context().project_generation, 0);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "later".into(),
            }))
            .unwrap();
        let later = next(&service);
        assert_eq!(
            later
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context()
                .editor_revision,
            later.state.revision
        );
        assert!(later.state.revision > snapshot.context().editor_revision);
        assert_eq!(snapshot.project.tracks().len(), 1);
        assert_eq!(later.state.project.tracks().len(), 2);
        let prepared = Arc::clone(
            captured
                .playback
                .as_ref()
                .unwrap()
                .prepared_revision()
                .unwrap(),
        );
        let preview = preview_view_state(
            &captured.state,
            Arc::clone(&captured.state.project),
            prepared,
        );
        assert!(preview.canonical_audio_source.is_none());
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let mut custody = hex_engine::AudioPreparationCustody::try_new_on_control().unwrap();
        let token = custody
            .begin_input_on_control(
                snapshot
                    .arrangement_input(11, bank())
                    .with_audio_prepared_revision(),
            )
            .ok()
            .unwrap();
        let mut owner = hex_engine::AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        assert_eq!(owner.source_context(), Some(snapshot.context()));
        assert_eq!(
            owner
                .preparation()
                .audio_prepared_revision_metadata()
                .unwrap()
                .track_count,
            1
        );
        owner
            .seal_constructor_on_control(hex_engine::AudioStorageLedgerReservation {
                data_rows: 4096,
                handle_rows: 2048,
                root_rows: 4,
                max_retained_bytes: 64 * 1024 * 1024,
                max_construction_peak_bytes: 64 * 1024 * 1024,
            })
            .unwrap();
        let (mut runtime, anchors) = owner.split_constructor_on_control().ok().unwrap();
        assert_eq!(runtime.source_context(), Some(snapshot.context()));
        assert!(anchors.matches_current(&runtime));
        let proof = runtime.dispose_on_control().unwrap();
        let mut anchors = [Some(anchors)];
        proof.record_anchor_cells_on_control(&mut anchors).unwrap();
        anchors[0]
            .as_mut()
            .unwrap()
            .finish_disposal_on_control()
            .unwrap();
        assert!(
            anchors[0]
                .as_mut()
                .unwrap()
                .recover_inputs_on_control()
                .is_some()
        );
        let mut custody = hex_engine::AudioPreparationCustody::try_new_on_control().unwrap();
        let token = custody
            .begin_input_on_control(
                hex_engine::AudioArrangementInput::new(&snapshot.project, 11, bank())
                    .with_audio_prepared_revision(),
            )
            .ok()
            .unwrap();
        let mut legacy = hex_engine::AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        assert_eq!(legacy.source_context(), None);
        legacy.dispose_on_control().unwrap();
    }
}
