//! Immutable canonical Project/counter pairing owned by the existing worker.
//! Correlation only: constructor custody and exact page protection remain separate.
use super::*;

/// Actual fresh canonical snapshot owner. Clones preserve its constructor pair;
/// no raw Arc adoption or mutable child access is available on this facade.
/// Its child bindings, Project, construction peak and custody remain separate.
///
/// ```compile_fail
/// fn adopt(raw: std::sync::Arc<hex_session::service::CanonicalAudioSourceSnapshot>) {
///     let _: hex_session::service::SharedCanonicalAudioSource = raw.into();
/// }
/// ```
pub type SharedCanonicalAudioSource = hex_engine::SharedMetadata<CanonicalAudioSourceSnapshot>;

/// Visit the actual paired snapshot and its currently supported resident children.
/// Every other live snapshot/model/media owner must be visited separately. This
/// neither seals the graph nor supplies construction peak or publication custody.
/// Verified-file facts remain unsupported here rather than inferred from a path.
pub fn visit_canonical_source_selected_storage_on_control<'a>(
    owner: &'a SharedCanonicalAudioSource,
    ledger: &mut hex_engine::AudioRootStorageLedger<'a>,
) -> Result<(), hex_engine::AudioStorageError> {
    ledger.borrow_sampler_captured_source_fields_on_control(&sampler_fields_on_control(owner))
}
// Actual paired field and local payload only; no temporary projection owner.
pub(crate) fn sampler_fields_on_control(
    owner: &SharedCanonicalAudioSource,
) -> hex_engine::sampler_host_fields::CapturedSourceFields<'_> {
    hex_engine::sampler_host_fields::CapturedSourceFields {
        owner: hex_engine::sampler_host_fields::SharedMetadataRef::of(owner),
        payload: owner.as_ref(),
    }
}
impl hex_engine::sampler_host_fields::CapturedSourcePayload for CanonicalAudioSourceSnapshot {
    fn body(&self) -> hex_engine::sampler_host_fields::BodyRef<'_> {
        hex_engine::sampler_host_fields::BodyRef::of(self)
    }
    fn project(&self) -> &crate::SharedProject {
        let Self {
            project,
            context: _,
            mode: _,
            sources: _,
        } = self;
        project
    }
    fn bindings(&self) -> hex_engine::sampler_host_fields::VecRef<'_> {
        let Self {
            project: _,
            context: _,
            mode: _,
            sources,
        } = self;
        hex_engine::sampler_host_fields::VecRef::entries(sources)
    }
    fn resident(&self, index: usize) -> Result<&hex_engine::Audio, hex_engine::AudioStorageError> {
        let Self {
            project: _,
            context: _,
            mode: _,
            sources,
        } = self;
        match sources.as_slice().get(index).map(|(_, binding)| binding) {
            Some(crate::AudioSourceBinding::Resident(audio)) => Ok(audio),
            Some(crate::AudioSourceBinding::VerifiedFile(_)) | None => {
                Err(hex_engine::AudioStorageError::UnknownStorage)
            }
        }
    }
}

pub struct CanonicalAudioSourceSnapshot {
    project: crate::SharedProject,
    context: hex_engine::AudioSourceContext,
    mode: crate::DocumentAudioMode,
    sources: SortedEntries<hex_project::AssetId, crate::AudioSourceBinding>,
}
impl CanonicalAudioSourceSnapshot {
    pub(super) fn capture(
        project: &crate::SharedProject,
        project_generation: u64,
        editor_revision: u64,
        session: &Session,
    ) -> Self {
        Self {
            project: project.clone(),
            mode: session.audio_mode(),
            sources: session.source_bindings(),
            context: hex_engine::AudioSourceContext {
                project_generation,
                editor_revision,
            },
        }
    }
    /// Borrow the actual paired model field; no raw Arc or synthetic snapshot.
    pub fn project_owner_on_control(&self) -> &crate::SharedProject {
        &self.project
    }
    pub fn audio_mode(&self) -> crate::DocumentAudioMode {
        self.mode
    }
    /// Immutable resolved bindings paired with this exact Project/context.
    /// Facts only: native publication must match actual retained source leases.
    pub fn sources(&self) -> &SortedEntries<hex_project::AssetId, crate::AudioSourceBinding> {
        &self.sources
    }
    /// Current musical references only, excluding bindings retained solely for
    /// Undo/Redo or unused asset records. This is a projection of the captured
    /// Project, not a second editable model or native source authority.
    pub fn required_sources(
        &self,
    ) -> impl Iterator<Item = (hex_project::AssetId, &crate::AudioSourceBinding)> {
        self.sources.iter().filter_map(|(id, binding)| {
            let arrangement = self.project.tracks().iter()
                .any(|track| track.clips().iter().any(|clip| clip.asset() == *id));
            let session = self.project.session_cells().iter().any(|cell| {
                matches!(cell.content(), hex_project::SessionCellContent::Audio(clip) if clip.asset() == *id)
            });
            (arrangement || session).then_some((*id, binding))
        })
    }
    pub fn sample_rate(&self) -> u32 {
        self.project.sample_rate()
    }
    /// Conservative reservation only; the actual native constructor remains
    /// responsible for source/custody/closed-class validation.
    pub fn candidate_reservation_quote(
        &self,
    ) -> Result<hex_engine::AudioCandidateReservationQuote, hex_engine::AudioStorageError> {
        hex_engine::AudioCandidateReservationQuote::for_project(
            &self.project,
            self.required_sources().count(),
        )
    }
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        self.context
    }
    /// Prepare inside the native host's reserved cell without extracting its
    /// bank, draft or completed owner into the Session/Desktop adapter.
    pub fn prepare_candidate_on_control(
        &self,
        source_catalog_generation: u64,
        destination: &mut hex_engine::AudioCandidatePreparationOperations<'_>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        destination.prepare_on_control(&self.project, source_catalog_generation, self.context)
    }
    /// Keep a host's already-rooted input bank in place during all fallible
    /// preparation. Native custody still comes only from the actual constructor.
    pub fn begin_prepared_revision_in_place(
        &self,
        source_catalog_generation: u64,
        custody: &mut hex_engine::AudioPreparationCustody,
        inputs: &mut Option<hex_engine::OwnedAudioInputs>,
    ) -> Result<hex_engine::AudioPreparationToken, Box<hex_engine::AudioArrangementStartRefusal>>
    {
        custody.begin_prepared_revision_in_place_on_control(
            &self.project,
            source_catalog_generation,
            self.context,
            inputs,
        )
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
    fn paired_canonical_source_actual_producer_aliases_and_independent_bodies_conserve_limits() {
        use hex_engine::{AudioRootStorageLedger, AudioStorageLedgerReservation};
        if !hex_engine::audio_storage_constructor_layout_supported() {
            eprintln!("unsupported canonical-source outer constructor; no positive storage proof");
            return;
        }
        eprintln!("SUPPORTED actual canonical-source producer paired outer and model fields");
        let session = Session::new(48_000, 8, 1024).unwrap();
        let first = view_state(
            &session,
            hex_engine::empty_midi_preview_on_control(),
            7,
            11,
            false,
            None,
        );
        let actual = first.canonical_audio_source.as_ref().unwrap();
        let retained = actual.clone();
        let independent = SharedCanonicalAudioSource::new_on_control(
            CanonicalAudioSourceSnapshot::capture(&first.project, 11, 7, &session),
        );
        let expected_context = hex_engine::AudioSourceContext {
            project_generation: 11,
            editor_revision: 7,
        };
        assert_eq!(actual.context(), expected_context);
        assert!(actual.shares_owner_with(&retained));
        assert!(!actual.shares_owner_with(&independent));
        assert_eq!(actual.context(), independent.context());
        assert!(actual.project.shares_owner_with(&independent.project));
        let reservation = AudioStorageLedgerReservation {
            data_rows: 128,
            handle_rows: 128,
            root_rows: 16,
            max_retained_bytes: 2_000_000,
            max_construction_peak_bytes: 2_000_000,
        };
        let collect = |reservation| -> Result<_, hex_engine::AudioStorageError> {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
            ledger.borrow_shared_metadata_body_on_control(actual)?;
            let outer = ledger.summary()?.data_bytes;
            assert!(outer > std::mem::size_of::<CanonicalAudioSourceSnapshot>());
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            // An alias is a separate physical field, but not another body.
            ledger.borrow_shared_metadata_body_on_control(&retained)?;
            ledger.validate_selected_shared_handles_on_control()?;
            assert_eq!(ledger.summary()?.data_bytes, outer);
            ledger.borrow_shared_metadata_body_on_control(&independent)?;
            assert_eq!(ledger.summary()?.data_bytes, outer * 2);
            assert_eq!(ledger.selected_shared_counts_on_control(), (2, 3));
            // Visit the actual nested Project fields once despite outer aliases.
            visit_canonical_source_selected_storage_on_control(actual, &mut ledger)?;
            visit_canonical_source_selected_storage_on_control(&retained, &mut ledger)?;
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            visit_canonical_source_selected_storage_on_control(&independent, &mut ledger)?;
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            crate::visit_shared_project_selected_storage_on_control(&first.project, &mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            assert!(ledger.summary()?.require_complete().is_err()); // no whole-root seal
            ledger.summary()
        };
        let measured = collect(reservation).unwrap();
        let mut exact = reservation;
        exact.max_retained_bytes = measured.retained_bytes;
        exact.max_construction_peak_bytes = measured.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), measured);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
        drop(session);
        drop(first);
        drop(independent);
        assert_eq!(retained.context(), expected_context);
        assert_eq!(retained.sample_rate(), 48_000);
    }

    #[test]
    fn typed_captured_source_after_history_prune_rejects_foreign_project_and_binding_fields() {
        use hex_engine::sampler_host_fields::{
            BodyRef, CapturedSourceFields, CapturedSourcePayload, VecRef,
        };
        use hex_engine::{
            AudioRootStorageLedger, AudioStorageError, AudioStorageLedgerReservation,
        };
        let mut session = Session::new(48_000, 1, 1024).unwrap();
        session
            .editor
            .execute(Command::AddTrack {
                name: "captured only".into(),
            })
            .unwrap();
        let track = session.project().tracks()[0].id();
        let initial = view_state(
            &session,
            hex_engine::empty_midi_preview_on_control(),
            1,
            0,
            false,
            None,
        );
        let captured = initial.canonical_audio_source.as_ref().unwrap().clone();
        drop(initial);
        session.editor.execute(Command::RemoveTrack(track)).unwrap();
        session
            .editor
            .execute(Command::AddTrack {
                name: "later".into(),
            })
            .unwrap();
        assert!(
            session
                .editor
                .retained_projects_on_control()
                .all(|project| project.track(track).is_none())
        );
        assert!(captured.project.track(track).is_some());
        let current = view_state(
            &session,
            hex_engine::empty_midi_preview_on_control(),
            3,
            0,
            false,
            None,
        );
        let actual = sampler_fields_on_control(&captured);
        let foreign = sampler_fields_on_control(current.canonical_audio_source.as_ref().unwrap());
        let limits = AudioStorageLedgerReservation {
            data_rows: 512,
            handle_rows: 512,
            root_rows: 32,
            max_retained_bytes: 4_000_000,
            max_construction_peak_bytes: 4_000_000,
        };
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
        ledger
            .borrow_sampler_captured_source_fields_on_control(&actual)
            .unwrap();
        assert!(ledger.summary().unwrap().require_complete().is_err());
        struct Invalid<'a> {
            actual: &'a CapturedSourceFields<'a>,
            foreign: &'a CapturedSourceFields<'a>,
            bindings: bool,
        }
        impl CapturedSourcePayload for Invalid<'_> {
            fn body(&self) -> BodyRef<'_> {
                self.actual.payload.body()
            }
            fn project(&self) -> &crate::SharedProject {
                if self.bindings {
                    self.actual.payload.project()
                } else {
                    self.foreign.payload.project()
                }
            }
            fn bindings(&self) -> VecRef<'_> {
                if self.bindings {
                    self.foreign.payload.bindings()
                } else {
                    self.actual.payload.bindings()
                }
            }
            fn resident(&self, index: usize) -> Result<&hex_engine::Audio, AudioStorageError> {
                self.actual.payload.resident(index)
            }
        }
        for bindings in [false, true] {
            let bad = Invalid {
                actual: &actual,
                foreign: &foreign,
                bindings,
            };
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits).unwrap();
            assert_eq!(
                ledger.borrow_sampler_captured_source_fields_on_control(&CapturedSourceFields {
                    owner: actual.owner,
                    payload: &bad
                }),
                Err(AudioStorageError::SourceMismatch)
            );
            assert!(ledger.summary().is_err());
        }
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
        assert!(snapshot.project.shares_owner_with(&captured.state.project));
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
        let preview = preview_view_state(&captured.state, captured.state.project.clone(), prepared);
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
