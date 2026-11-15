//! Immutable canonical Project/source/context pairing owned by the Session worker.
//!
//! The exact document generation/editor revision travels with captured source facts. This is
//! correlation, not native admission: desktop workers independently retain and verify their actual
//! source/domain owners. Verified-mode events do not offer a resident Playback fallback, including
//! empty graphs; private loaded-sampler sources also stay in their worker-owned path rather than
//! escaping through an Event owner.
use super::*;
mod source_content;
pub use source_content::RawArrangementContentWitness;
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
    /// Control-side identity check for retaining this exact preparation source.
    /// File facts and PCM handles are compared, never read/copied/adopted here.
    pub(super) fn matches_session_sources(&self, session: &Session) -> bool {
        if self.mode != session.audio_mode() {
            return false;
        }
        let current = session.source_bindings();
        self.sources.len() == current.len()
            && self
                .sources
                .iter()
                .all(|(id, source)| match (source, current.get(id)) {
                    (
                        crate::AudioSourceBinding::Resident(old),
                        Some(crate::AudioSourceBinding::Resident(new)),
                    ) => old.sample_rate() == new.sample_rate() && old.shares_pcm_with(new),
                    (
                        crate::AudioSourceBinding::VerifiedFile(old),
                        Some(crate::AudioSourceBinding::VerifiedFile(new)),
                    ) => old == new,
                    _ => false,
                })
    }
    pub(super) fn project_metadata(&self) -> &Project {
        &self.project
    }
    pub fn audio_mode(&self) -> crate::DocumentAudioMode {
        self.mode
    }
    /// Immutable resolved bindings paired with this exact Project/context.
    /// Facts only: native publication must match actual retained source leases.
    pub fn sources(&self) -> &BTreeMap<hex_project::AssetId, crate::AudioSourceBinding> {
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
                .any(|track| {
                    track
                        .clips()
                        .iter()
                        .any(|clip| clip.referenced_assets().any(|asset| asset == *id))
                });
            let session = self.project.session_cells().iter().any(|cell| {
                matches!(cell.content(), hex_project::SessionCellContent::Audio(clip) if clip.asset() == *id)
            });
            (arrangement || session).then_some((*id, binding))
        })
    }
    /// Full-source plain-pitch streams required by verified Arrangement clips.
    /// Repeated clips may yield the same requirement. These are immutable facts,
    /// not worker leases or native admission; unsupported edits remain excluded.
    pub fn required_plain_pitch_sources(
        &self,
    ) -> impl Iterator<
        Item = (
            hex_project::AssetId,
            &crate::VerifiedAudioFile,
            i16,
            hex_project::WarpResampler,
        ),
    > {
        self.project
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .filter_map(move |clip| {
                let pitch = crate::audio_sources::plain_pitch_requirement(&self.project, clip)?;
                let crate::AudioSourceBinding::VerifiedFile(file) =
                    self.sources.get(&clip.asset())?
                else {
                    return None;
                };
                Some((pitch.asset, file, pitch.cents, pitch.law))
            })
    }
    /// Verified-file Re-Pitch roots in this exact snapshot. The caller must
    /// validate and project each root before treating it as a worker source.
    pub fn required_repitch_sources(
        &self,
    ) -> impl Iterator<
        Item = (
            hex_project::AssetId,
            &crate::VerifiedAudioFile,
            &hex_project::AudioClipEdit,
            &hex_project::TempoMap,
        ),
    > {
        self.project
            .tracks()
            .iter()
            .flat_map(|track| track.clips())
            .filter_map(move |clip| {
                let edit = clip.audio_edit()?;
                if !edit.domain.warp_enabled
                    || !edit
                        .domain
                        .warp
                        .as_ref()
                        .is_some_and(|warp| matches!(warp.mode, hex_project::WarpMode::RePitch))
                {
                    return None;
                }
                let crate::AudioSourceBinding::VerifiedFile(file) =
                    self.sources.get(&clip.asset())?
                else {
                    return None;
                };
                Some((clip.asset(), file, edit, self.project.tempo_map()))
            })
    }
    /// Validated derived facts for each Re-Pitch root. These are not worker leases.
    pub fn required_repitch_metadata(
        &self,
        generation: u64,
    ) -> impl Iterator<
        Item = Result<
            (
                &crate::VerifiedAudioFile,
                hex_engine::StreamedSourceMetadata,
            ),
            hex_engine::Invalid,
        >,
    > {
        self.required_repitch_sources()
            .map(move |(asset, file, edit, tempo)| {
                let raw = file.source_metadata(asset, generation)?;
                let (map, law) = hex_engine::audio_transform::streamed_repitch_map(
                    edit,
                    tempo,
                    raw.playback_frames(),
                    &std::sync::atomic::AtomicBool::new(false),
                )?;
                Ok((file, raw.with_repitch(&map, law)?))
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
        self.candidate_reservation_quote_for(&self.project)
    }
    pub(super) fn candidate_reservation_quote_for(
        &self,
        project: &Project,
    ) -> Result<hex_engine::AudioCandidateReservationQuote, hex_engine::AudioStorageError> {
        use hex_engine::AudioStorageError;
        if hex_engine::native_device_resources::requires_private_sampler_sources(project) {
            return Err(hex_engine::AudioStorageError::UnsupportedGraph);
        }
        // A sizing-only generation gives raw and derived facts a common identity;
        // the actual host catalog and its leases are checked at construction.
        let mut streams = BTreeMap::new();
        let mut derived_count = 0usize;
        for (asset, source) in self.required_sources() {
            if let crate::AudioSourceBinding::VerifiedFile(file) = source {
                let metadata = file
                    .source_metadata(asset, 1)
                    .map_err(|_| AudioStorageError::SourceMismatch)?;
                streams.insert(metadata.key(), metadata);
            }
        }
        for clip in project.tracks().iter().flat_map(|track| track.clips()) {
            let Some(pitch) = crate::audio_sources::plain_pitch_requirement(project, clip) else {
                continue;
            };
            let Some(crate::AudioSourceBinding::VerifiedFile(file)) =
                self.sources.get(&pitch.asset)
            else {
                continue;
            };
            let metadata = file
                .source_metadata(pitch.asset, 1)
                .and_then(|raw| raw.with_plain_pitch(pitch.cents, pitch.law))
                .map_err(|_| AudioStorageError::SourceMismatch)?;
            if streams.insert(metadata.key(), metadata).is_none() {
                derived_count = derived_count
                    .checked_add(1)
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        for clip in project.tracks().iter().flat_map(|track| track.clips()) {
            let Some(crate::AudioSourceBinding::VerifiedFile(file)) =
                self.sources.get(&clip.asset())
            else {
                continue;
            };
            let Some(metadata) =
                crate::audio_sources::repitch_source_metadata(project, clip, file, 1)
                    .map_err(|_| AudioStorageError::SourceMismatch)?
            else {
                continue;
            };
            if streams.insert(metadata.key(), metadata).is_none() {
                derived_count = derived_count
                    .checked_add(1)
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        let sources = self
            .required_sources()
            .count()
            .checked_add(derived_count)
            .ok_or(AudioStorageError::Overflow)?;
        let streams: Vec<_> = streams.into_values().collect();
        hex_engine::AudioCandidateReservationQuote::for_project_with_pitched_streams(
            project, sources, &streams,
        )
    }
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        self.context
    }
    /// Read-only native-effect identities/state in this exact canonical source.
    /// This borrows opaque bytes and grants no native source or admission.
    pub fn effect_preparation(&self) -> ResidentEffectPreparation<'_> {
        ResidentEffectPreparation {
            project: &self.project,
            context: self.context,
            restoring_plugin_state: false,
            preparing_edit: false,
        }
    }
    /// Prepare inside the native host's reserved cell without extracting its
    /// bank, draft or completed owner into the Session/Desktop adapter.
    pub fn prepare_candidate_on_control(
        &self,
        source_catalog_generation: u64,
        destination: &mut hex_engine::AudioCandidatePreparation,
    ) -> Result<(), hex_engine::AudioStorageError> {
        destination.prepare_on_control(&self.project, source_catalog_generation, self.context)
    }
    /// Use host-borrowed effect facts with this exact immutable Project/context.
    /// Sources and native admission remain in their original host-owned cells.
    pub fn prepare_candidate_with_effects_on_control(
        &self,
        source_catalog_generation: u64,
        destination: &mut hex_engine::AudioCandidatePreparation,
        effects: &hex_engine::AudioCandidateEffects<'_, '_>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        effects.prepare(
            destination,
            &self.project,
            source_catalog_generation,
            self.context,
            None,
        )
    }
    pub fn reservation_quote_with_effects(
        &self,
        effects: &hex_engine::AudioCandidateEffects<'_, '_>,
    ) -> Result<hex_engine::AudioCandidateReservationQuote, hex_engine::AudioStorageError> {
        if hex_engine::native_device_resources::requires_private_sampler_sources(&self.project) {
            return Err(hex_engine::AudioStorageError::UnsupportedGraph);
        }
        effects.quote(&self.project, self.required_sources().count())
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
    #[test]
    fn pitch_requirements_include_only_complete_verified_file_clips() {
        let files = super::super::verified_source_tests::Files::new();
        let path = files.wav("pitch.wav", 48_000, 32, 1000);
        let file = crate::VerifiedAudioFile::inspect(
            path.clone(),
            48_000,
            hex_media::VerifiedWavReaderLimits::new(
                hex_media::WavReaderLimits::new(1 << 20, 4096, 32, 4096),
                1 << 20,
            ),
            || false,
        )
        .unwrap();
        let mut editor = hex_project::Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let hex_project::Created::Imported { clip, asset, .. } = editor
            .execute(hex_project::Command::ImportAudioTrack {
                name: "source".into(),
                path,
                sample_rate: 48_000,
                frames: 32,
            })
            .unwrap()
        else {
            panic!("file fixture")
        };
        editor
            .execute(hex_project::Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        let hex_project::Created::Clip(first) = editor
            .execute(hex_project::Command::DuplicateClip(clip))
            .unwrap()
        else {
            panic!("duplicate")
        };
        editor
            .execute(hex_project::Command::SetAudioClipPitch {
                clip: first,
                cents: 700,
            })
            .unwrap();
        let hex_project::Created::Clip(second) = editor
            .execute(hex_project::Command::DuplicateClip(first))
            .unwrap()
        else {
            panic!("duplicate")
        };
        editor
            .execute(hex_project::Command::SetAudioClipPitch {
                clip: second,
                cents: -300,
            })
            .unwrap();
        let hex_project::Created::Clip(partial) = editor
            .execute(hex_project::Command::DuplicateClip(second))
            .unwrap()
        else {
            panic!("duplicate")
        };
        editor
            .execute(hex_project::Command::SetAudioClipPitch {
                clip: partial,
                cents: 0,
            })
            .unwrap();
        let start = editor.project().clip(partial).unwrap().start();
        editor
            .execute(hex_project::Command::TrimClipEdge {
                clip: partial,
                edge: hex_project::TrimEdge::Right,
                frame: start + 16,
            })
            .unwrap();
        editor
            .execute(hex_project::Command::SetAudioClipPitch {
                clip: partial,
                cents: 200,
            })
            .unwrap();
        let hex_project::Created::Imported {
            clip: resident_clip,
            asset: resident_asset,
            ..
        } = editor
            .execute(hex_project::Command::ImportAudioTrack {
                name: "resident".into(),
                path: "resident.wav".into(),
                sample_rate: 48_000,
                frames: 32,
            })
            .unwrap()
        else {
            panic!("resident fixture")
        };
        editor
            .execute(hex_project::Command::SetClipFades {
                clip: resident_clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        editor
            .execute(hex_project::Command::SetAudioClipPitch {
                clip: resident_clip,
                cents: 500,
            })
            .unwrap();
        let track = editor.project().tracks()[0].id();
        editor
            .execute(hex_project::Command::MoveClip {
                clip,
                track,
                start: 10_000,
            })
            .unwrap();
        let hex_project::Created::Clip(repeated) = editor
            .execute(hex_project::Command::DuplicateClip(clip))
            .unwrap()
        else {
            panic!("duplicate")
        };
        editor
            .execute(hex_project::Command::SetAudioClipPitch {
                clip: repeated,
                cents: 700,
            })
            .unwrap();
        let snapshot = CanonicalAudioSourceSnapshot {
            project: Arc::new(editor.project().clone()),
            context: hex_engine::AudioSourceContext {
                project_generation: 1,
                editor_revision: 1,
            },
            mode: crate::DocumentAudioMode::Resident,
            sources: BTreeMap::from([
                (asset, crate::AudioSourceBinding::VerifiedFile(file)),
                (
                    resident_asset,
                    crate::AudioSourceBinding::Resident(
                        hex_engine::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap(),
                    ),
                ),
            ]),
        };
        let mut requirements = snapshot
            .required_plain_pitch_sources()
            .map(|(id, _, cents, law)| (id, cents, law))
            .collect::<Vec<_>>();
        assert_eq!(requirements.len(), 3);
        requirements.sort_by_key(|(_, cents, _)| *cents);
        requirements.dedup();
        assert_eq!(
            requirements,
            vec![
                (asset, -300, hex_project::WarpResampler::V2),
                (asset, 700, hex_project::WarpResampler::V2)
            ]
        );
        let quote = snapshot.candidate_reservation_quote().unwrap();
        assert_eq!(quote.source_capacity, 4);
        assert!(
            quote.prepared_pcm_frames >= 32,
            "resident and partial transforms still reserve pcm"
        );
    }
    fn next(service: &Service) -> Event {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
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
            .submit(Request::Edit(Command::InsertTrackWithDevice {
                name: "audio".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
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
