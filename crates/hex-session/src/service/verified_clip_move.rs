//! Source-bound metadata projection for a held clip move in verified streaming.
//! The canonical snapshot remains the sole authority for source facts and context;
//! this value owns no native graph, reader, page or source lease.
use super::*;

/// A transient clip placement derived from one genuine canonical source.
pub struct VerifiedClipMoveSource {
    canonical: Arc<CanonicalAudioSourceSnapshot>,
    project: Arc<Project>,
    identity: ClipMovePreviewIdentity,
}

impl VerifiedClipMoveSource {
    /// Project an admitted clip target without changing canonical project or history.
    /// A target can never mint a new source identity or bypass project validation.
    pub fn capture(
        canonical: Arc<CanonicalAudioSourceSnapshot>,
        target: ArrangementMoveAudition,
    ) -> Result<Self, SessionError> {
        let project = canonical.project_metadata().clone();
        Self::project(canonical, target, project, false)
    }

    // The worker supplies its actual project and sealed current view, not a
    // project supplied with the gesture.
    pub(super) fn capture_current(
        canonical: Arc<CanonicalAudioSourceSnapshot>,
        target: ArrangementMoveAudition,
        project: &Project,
        state: &ViewState,
    ) -> Result<Self, SessionError> {
        if state.project.as_ref() != project
            || state
                .canonical_audio_source
                .as_ref()
                .is_none_or(|source| !Arc::ptr_eq(source, &canonical))
            || target.context.editor_revision != state.revision
            || state.audio_authoring_revision(target.context) != Some(state.revision)
        {
            return Err(SessionError::Invalid(
                "clip audition authoring source changed",
            ));
        }
        Self::project(canonical, target, project.clone(), true)
    }

    fn project(
        canonical: Arc<CanonicalAudioSourceSnapshot>,
        target: ArrangementMoveAudition,
        project: Project,
        current_authoring: bool,
    ) -> Result<Self, SessionError> {
        if canonical.audio_mode() != DocumentAudioMode::VerifiedStreaming
            || (!current_authoring && target.context != canonical.context())
            || target.gesture == 0
            || target.sequence == 0
            || !target.valid()
        {
            return Err(SessionError::Invalid(
                "clip audition source or target changed",
            ));
        }
        let identity = target.identity();
        let mut editor = hex_project::Editor::new(project, 1)?;
        editor.execute(clip_audition::Target::Move(target).command())?;
        let source = Self {
            canonical,
            project: Arc::new(editor.project().clone()),
            identity,
        };
        if hex_engine::native_device_resources::requires_private_sampler_sources(&source.project) {
            return Err(SessionError::Invalid(
                "clip audition is unavailable for this streaming project",
            ));
        }
        Ok(source)
    }

    /// The real canonical source and its unchanged catalog bindings.
    pub fn canonical(&self) -> &Arc<CanonicalAudioSourceSnapshot> {
        &self.canonical
    }

    /// Correlation metadata, not proof that a callback applied the candidate.
    pub fn identity(&self) -> ClipMovePreviewIdentity {
        self.identity
    }

    /// Use the canonical source count as a conservative finite reservation.
    pub fn candidate_reservation_quote(
        &self,
    ) -> Result<hex_engine::AudioCandidateReservationQuote, hex_engine::AudioStorageError> {
        if hex_engine::native_device_resources::requires_private_sampler_sources(&self.project) {
            return Err(hex_engine::AudioStorageError::UnsupportedGraph);
        }
        hex_engine::AudioCandidateReservationQuote::for_project(
            &self.project,
            self.canonical.required_sources().count(),
        )
    }

    /// Prepare only in the existing reserved cell, with the canonical source context.
    pub fn prepare_candidate_on_control(
        &self,
        catalog: u64,
        destination: &mut hex_engine::AudioCandidatePreparation,
    ) -> Result<(), hex_engine::AudioStorageError> {
        self.candidate_reservation_quote()?;
        destination.prepare_on_control(&self.project, catalog, self.canonical.context())
    }
}

/// Worker-side prepared transient clip move for a verified document that borrows
/// the real Session media bank, current native device resources, and host file
/// catalog without publishing a graph, transferring owners, or mutating
/// canonical project/history.
pub(in crate::service) struct PreparedVerifiedClipAudition {
    canonical: Arc<CanonicalAudioSourceSnapshot>,
    project: Arc<Project>,
    identity: ClipMovePreviewIdentity,
    snapshot: resident_audio::Snapshot,
}

impl PreparedVerifiedClipAudition {
    /// Prepare a source-bound transient clip move on control without publishing
    /// or modifying canonical project/history.
    pub(in crate::service) fn prepare_on_control(
        session: &Session,
        canonical: Arc<CanonicalAudioSourceSnapshot>,
        target: ArrangementMoveAudition,
        current: &resident_audio::Snapshot,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Self, SessionError> {
        if session.audio_mode() != DocumentAudioMode::VerifiedStreaming
            || canonical.audio_mode() != DocumentAudioMode::VerifiedStreaming
            || canonical.project_metadata() != session.project()
            || target.context != canonical.context()
            || current.prepared_context != canonical.context()
            || target.gesture == 0
            || target.sequence == 0
            || !target.valid()
        {
            return Err(SessionError::Invalid(
                "clip audition source or target changed",
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Err(SessionError::Invalid("clip audition cancelled"));
        }
        if canonical.required_sources().next().is_some() && current.file_sources.is_none() {
            return Err(SessionError::Invalid(
                "verified clip audition requires borrowed file catalog custody",
            ));
        }
        let identity = target.identity();
        let command = clip_audition::Target::Move(target).command();
        let (project, snapshot) = session.prepare_verified_clip_move_snapshot(
            command,
            resident_audio::ResidentSinkContext {
                context: canonical.context(),
                sink,
                admission,
            },
            current,
            cancel,
        )?;
        Ok(Self {
            canonical,
            project,
            identity,
            snapshot,
        })
    }

    /// Consume this prepared transient audition into its projected project,
    /// preview identity, and prepared ordinary snapshot for worker publication.
    pub(in crate::service) fn into_parts(
        self,
    ) -> (
        Arc<Project>,
        ClipMovePreviewIdentity,
        resident_audio::Snapshot,
    ) {
        (self.project, self.identity, self.snapshot)
    }

    /// Exact canonical source snapshot that authorized this transient preview.
    pub(in crate::service) fn canonical(&self) -> &Arc<CanonicalAudioSourceSnapshot> {
        &self.canonical
    }

    /// Projected transient project (canonical Session project remains unchanged).
    #[cfg(test)]
    pub(in crate::service) fn project(&self) -> &Arc<Project> {
        &self.project
    }

    /// Exact correlation identity for this transient preview.
    #[cfg(test)]
    pub(in crate::service) fn identity(&self) -> ClipMovePreviewIdentity {
        self.identity
    }

    /// Borrowed compiled revision for control-side inspection or playback build.
    #[cfg(test)]
    pub(in crate::service) fn prepared(&self) -> &Arc<PreparedRevision> {
        &self.snapshot.prepared
    }

    /// Borrowed host file catalog binding for streamed audio clips, if any.
    #[cfg(test)]
    pub(in crate::service) fn file_sources(&self) -> Option<&crate::service::ResidentFileSet> {
        self.snapshot.file_sources.as_ref()
    }

    /// Construct an isolated control-side `Playback` using the borrowed
    /// ordinary resources without publishing to any native callback.
    #[cfg(test)]
    pub(in crate::service) fn build_playback_on_control(
        &self,
    ) -> Result<hex_engine::playback::Playback, SessionError> {
        hex_engine::playback::Playback::try_from_prepared_with_device_resources(
            Arc::clone(&self.snapshot.prepared),
            &self.snapshot.resources,
        )
        .map_err(|error| SessionError::NativeAudio(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::verified_source_tests::{Files, next, reservation, run};
    use hex_project::{BeatTime, ClipGroupPosition, Command, Created};

    fn verified_fixture(include_midi: bool) -> (Files, Service, Event) {
        let files = Files::new();
        let wav = files.wav("clip-move.wav", 48_000, 128, 8192);
        let service = Service::new(48_000, 16, 0).unwrap();
        next(&service);
        run(
            &service,
            Request::NewWithAudioMode {
                mode: DocumentAudioMode::VerifiedStreaming,
                reservation: Some(reservation(2)),
            },
        );
        run(&service, Request::Import(wav));
        if include_midi {
            let midi_track = run(
                &service,
                Request::Edit(Command::AddTrack {
                    name: "midi".into(),
                }),
            );
            let track = midi_track.state.project.tracks()[1].id();
            run(
                &service,
                Request::Edit(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                }),
            );
        }
        let event = run(
            &service,
            Request::Edit(Command::AddTrack {
                name: "destination".into(),
            }),
        );
        (files, service, event)
    }

    #[test]
    fn verified_targets_match_canonical_commands_without_mutating_sources() {
        let (_files, _service, event) = verified_fixture(true);
        let canonical = Arc::clone(event.state.canonical_audio_source.as_ref().unwrap());
        let before = Arc::clone(&event.state.project);
        let source = &before.tracks()[0];
        let destination = before.tracks()[2].id();
        assert_eq!(canonical.required_sources().count(), 1);
        for (clip, position) in [
            (source.clips()[0].id(), ClipGroupPosition::Audio(32)),
            (
                before.tracks()[1].midi_clips()[0].id(),
                ClipGroupPosition::Midi(BeatTime::ONE),
            ),
        ] {
            for mode in [
                ArrangementMoveMode::Move,
                ArrangementMoveMode::Copy,
                ArrangementMoveMode::AliasCopy,
            ] {
                for new_track in [false, true] {
                    let dest = if new_track {
                        ArrangementMoveDestination::NewTrack(
                            NewTrackMoveDestination::new(
                                Arc::from("new track"),
                                Some(destination),
                                0.5,
                            )
                            .unwrap(),
                        )
                    } else {
                        ArrangementMoveDestination::Existing(destination)
                    };
                    let target = ArrangementMoveAudition {
                        context: canonical.context(),
                        gesture: 7,
                        sequence: 1,
                        clips: vec![clip].into(),
                        anchor: clip,
                        destination: dest,
                        position,
                        mode,
                    };
                    let command = match (new_track, mode, position) {
                        (false, ArrangementMoveMode::Move, ClipGroupPosition::Audio(start)) => {
                            Command::MoveClipWithOverwrite {
                                clip,
                                track: destination,
                                start,
                            }
                        }
                        (false, ArrangementMoveMode::Move, ClipGroupPosition::Midi(start)) => {
                            Command::MoveMidiClipWithOverwrite {
                                clip,
                                track: destination,
                                start,
                            }
                        }
                        (false, ArrangementMoveMode::Copy, ClipGroupPosition::Audio(start)) => {
                            Command::CopyClipTo {
                                clip,
                                track: destination,
                                start,
                            }
                        }
                        (false, ArrangementMoveMode::Copy, ClipGroupPosition::Midi(start)) => {
                            Command::CopyMidiClipTo {
                                clip,
                                track: destination,
                                start,
                            }
                        }
                        (false, ArrangementMoveMode::AliasCopy, position) => {
                            Command::DuplicateAliasClipsAt {
                                clips: vec![clip],
                                anchor: clip,
                                track: destination,
                                position,
                            }
                        }
                        (true, ArrangementMoveMode::Move, ClipGroupPosition::Audio(start)) => {
                            Command::MoveClipToNewTrack {
                                clip,
                                name: "new track".into(),
                                before_track: Some(destination),
                                start,
                                initial_gain: Some(0.5),
                            }
                        }
                        (true, ArrangementMoveMode::Move, ClipGroupPosition::Midi(start)) => {
                            Command::MoveMidiClipToNewTrack {
                                clip,
                                name: "new track".into(),
                                before_track: Some(destination),
                                start,
                                initial_gain: Some(0.5),
                            }
                        }
                        (true, ArrangementMoveMode::Copy, ClipGroupPosition::Audio(start)) => {
                            Command::CopyClipToNewTrack {
                                clip,
                                name: "new track".into(),
                                before_track: Some(destination),
                                start,
                                initial_gain: Some(0.5),
                            }
                        }
                        (true, ArrangementMoveMode::Copy, ClipGroupPosition::Midi(start)) => {
                            Command::CopyMidiClipToNewTrack {
                                clip,
                                name: "new track".into(),
                                before_track: Some(destination),
                                start,
                                initial_gain: Some(0.5),
                            }
                        }
                        (true, ArrangementMoveMode::AliasCopy, position) => {
                            Command::DuplicateAliasClipToNewTrack {
                                clip,
                                name: "new track".into(),
                                before_track: Some(destination),
                                position,
                                initial_gain: Some(0.5),
                            }
                        }
                    };
                    let mut expected =
                        hex_project::Editor::new(before.as_ref().clone(), 1).unwrap();
                    expected.execute(command).unwrap();
                    let identity = target.identity();
                    let projection =
                        VerifiedClipMoveSource::capture(Arc::clone(&canonical), target).unwrap();
                    let projected = projection.project.snapshot();
                    let mut authored = expected.project().snapshot();
                    if new_track {
                        // New-track color is independently randomized by each command;
                        // it is not an audio or source identity.
                        authored.tracks[2].color = projected.tracks[2].color;
                    }
                    assert_eq!(projected, authored);
                    assert_eq!(projection.identity(), identity);
                    assert!(Arc::ptr_eq(projection.canonical(), &canonical));
                    assert_eq!(projection.canonical().required_sources().count(), 1);
                    assert_eq!(canonical.project_metadata(), before.as_ref());
                }
            }
        }
    }

    #[test]
    fn verified_projection_refuses_stale_targets_and_unsupported_midi_construction() {
        let service = Service::new(48_000, 16, 0).unwrap();
        next(&service);
        run(
            &service,
            Request::NewWithAudioMode {
                mode: DocumentAudioMode::VerifiedStreaming,
                reservation: Some(reservation(1)),
            },
        );
        let added = run(
            &service,
            Request::Edit(Command::AddTrack {
                name: "midi".into(),
            }),
        );
        let track = added.state.project.tracks()[0].id();
        let event = run(
            &service,
            Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            }),
        );
        let Created::Clip(clip) = event.created.unwrap() else {
            panic!("clip")
        };
        let canonical = Arc::clone(event.state.canonical_audio_source.as_ref().unwrap());
        let target = ArrangementMoveAudition {
            context: canonical.context(),
            gesture: 1,
            sequence: 1,
            clips: vec![clip].into(),
            anchor: clip,
            destination: ArrangementMoveDestination::Existing(track),
            position: ClipGroupPosition::Midi(BeatTime::ONE),
            mode: ArrangementMoveMode::Move,
        };
        let mut stale = target.clone();
        stale.context.editor_revision += 1;
        assert!(VerifiedClipMoveSource::capture(Arc::clone(&canonical), stale).is_err());
        let mut stale = target.clone();
        stale.context.project_generation += 1;
        assert!(VerifiedClipMoveSource::capture(Arc::clone(&canonical), stale).is_err());
        let mut invalid = target.clone();
        invalid.clips = Arc::from([]);
        assert!(VerifiedClipMoveSource::capture(Arc::clone(&canonical), invalid).is_err());
        let mut invalid = target.clone();
        invalid.gesture = 0;
        assert!(VerifiedClipMoveSource::capture(Arc::clone(&canonical), invalid).is_err());
        let mut invalid = target.clone();
        invalid.sequence = 0;
        assert!(VerifiedClipMoveSource::capture(Arc::clone(&canonical), invalid).is_err());
        let mut invalid = target.clone();
        invalid.destination = ArrangementMoveDestination::NewTrack(
            NewTrackMoveDestination::new(Arc::from("track"), None, 1.0).unwrap(),
        );
        invalid.clips = vec![clip, clip].into();
        assert!(VerifiedClipMoveSource::capture(Arc::clone(&canonical), invalid).is_err());
        let projection = VerifiedClipMoveSource::capture(Arc::clone(&canonical), target).unwrap();
        assert!(matches!(
            projection.candidate_reservation_quote(),
            Err(hex_engine::AudioStorageError::UnsupportedGraph)
        ));
        let reservation = hex_engine::AudioStorageReservation {
            allocation_items: 256,
            handle_obligations: 4096,
            max_prepared_data_bytes: 64 * 1024 * 1024,
            max_construction_data_bytes: 64 * 1024 * 1024,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        };
        let mut cell =
            hex_engine::AudioCandidatePreparation::try_new_on_control(0, reservation).unwrap();
        assert!(matches!(
            projection.prepare_candidate_on_control(1, &mut cell),
            Err(hex_engine::AudioStorageError::UnsupportedGraph)
        ));
        assert_eq!(canonical.project_metadata(), event.state.project.as_ref());
    }

    #[test]
    fn verified_audio_projection_prepares_only_with_real_stream_metadata() {
        let (_files, _service, event) = verified_fixture(false);
        let canonical = Arc::clone(event.state.canonical_audio_source.as_ref().unwrap());
        let clip = event.state.project.tracks()[0].clips()[0].id();
        let track = event.state.project.tracks()[1].id();
        let target = ArrangementMoveAudition {
            context: canonical.context(),
            gesture: 1,
            sequence: 1,
            clips: vec![clip].into(),
            anchor: clip,
            destination: ArrangementMoveDestination::Existing(track),
            position: ClipGroupPosition::Audio(32),
            mode: ArrangementMoveMode::Move,
        };
        let mut projection =
            VerifiedClipMoveSource::capture(Arc::clone(&canonical), target).unwrap();
        let quote = projection.candidate_reservation_quote().unwrap();
        let mut cell = hex_engine::AudioCandidatePreparation::try_new_on_control(
            quote.source_capacity,
            quote.storage_reservation(0).unwrap(),
        )
        .unwrap();
        for (asset, binding) in canonical.required_sources() {
            let crate::AudioSourceBinding::VerifiedFile(source) = binding else {
                panic!("verified file")
            };
            cell.add_streamed_source_on_control(source.source_metadata(asset, 1).unwrap())
                .unwrap();
        }
        projection
            .prepare_candidate_on_control(1, &mut cell)
            .unwrap();
        cell.cleanup_on_control().unwrap();
        assert_eq!(canonical.project_metadata(), event.state.project.as_ref());
        // Even a malformed resourceful sampler state must enter the private-source
        // refusal path, rather than being admitted as an empty instrument.
        let state = hex_lattice_contract::NativeDeviceState::new(
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sampler"),
            1,
            vec![1],
        )
        .unwrap();
        let mut editor = hex_project::Editor::new(projection.project.as_ref().clone(), 1).unwrap();
        editor
            .execute(Command::InsertInstrument { track, state })
            .unwrap();
        projection.project = Arc::new(editor.project().clone());
        assert!(
            hex_engine::native_device_resources::requires_private_sampler_sources(
                &projection.project
            )
        );
        assert!(matches!(
            projection.candidate_reservation_quote(),
            Err(hex_engine::AudioStorageError::UnsupportedGraph)
        ));
    }

    struct BorrowedCatalogSink {
        reject_catalog: bool,
        publish_calls: usize,
        effect_cleanups: usize,
    }

    impl WorkerAudioSink for BorrowedCatalogSink {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(16_000)
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
        fn prepare_resident_file_metadata(
            &mut self,
            input: ResidentFileInput<'_>,
        ) -> Result<Option<ResidentFileMetadata>, String> {
            if self.reject_catalog {
                return Err("host file catalog unavailable".into());
            }
            let mut sources = BTreeMap::new();
            for (id, file) in input.files {
                sources.insert(
                    *id,
                    Arc::new(file.source_metadata(*id, 7).map_err(|e| e.to_string())?),
                );
            }
            ResidentFileMetadata::new_with_derived(7, sources, BTreeMap::new()).map(Some)
        }
        fn finish_resident_effect_edit(&mut self, _: hex_engine::AudioSourceContext, _: bool) {
            self.effect_cleanups += 1;
        }
        fn publish_resident_transition(
            &mut self,
            playback: hex_engine::playback::Playback,
            _: Option<&hex_engine::native_device_resources::PreparedDeviceResources>,
            _: Arc<hex_engine::PreparedPlaybackObservation>,
            _: ResidentPublicationMetadata,
            _: ResidentPublicationPolicy,
            _: usize,
            _: &mut dyn FnMut(
                &[hex_engine::native_device_resources::DeviceResourceAdmission<'_>],
            ) -> Result<(), String>,
            _: crate::service::ResidentPublicationInputs<'_>,
        ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
            self.publish_calls += 1;
            Err(ResidentPublicationRefusal {
                kind: crate::service::ResidentPublicationFailure::Other,
                playback,
                error: "preparation boundary must never publish a graph".into(),
            })
        }
        fn poll(&mut self) {}
        fn close(&mut self) {}
    }

    struct TestStreamPages {
        key: Option<hex_engine::StreamSourceKey>,
        frames: Vec<[f32; 2]>,
    }

    impl hex_engine::MediaPages for TestStreamPages {
        fn page(
            &mut self,
            key: hex_engine::StreamSourceKey,
            page_index: u64,
        ) -> Option<hex_engine::BorrowedMediaPage<'_>> {
            if self.key == Some(key) && page_index == 0 {
                Some(hex_engine::BorrowedMediaPage {
                    page_index: 0,
                    frames: &self.frames,
                })
            } else {
                None
            }
        }
        fn request_page(&mut self, _: hex_engine::StreamSourceKey, _: u64) {}
    }

    fn render_peak_in_window(
        playback: &mut hex_engine::playback::Playback,
        pages: &mut TestStreamPages,
        start_frame: usize,
        frames: usize,
    ) -> f32 {
        playback.apply(hex_engine::playback::PlaybackCommand::PlayFrom(
            start_frame as u64,
        ));
        let mut block = vec![[0.0f32; 2]; frames];
        playback.render_with_pages(&mut block, pages).unwrap();
        block
            .iter()
            .flat_map(|frame| frame.iter())
            .map(|sample| sample.abs())
            .fold(0.0f32, f32::max)
    }

    #[test]
    fn verified_borrowed_ordinary_clip_audition_prepares_file_and_sampler_moves_before_canonical_edit()
     {
        let original = crate::service::sampler_export::tests::Files::new(false);
        let clip_files = Files::new();
        let clip_wav = clip_files.wav("verified-clip.wav", 16_000, 16, 1024);
        let saved = original.source().with_file_name("verified-audition.hex");
        let mut limits = crate::service::sampler_export::tests::reservation();
        limits.storage.max_retained_bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
        limits.storage.max_construction_peak_bytes =
            hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 3;

        // Build and save a genuine project combining a WAV clip and a native original Sampler with MIDI.
        let builder = Service::with_native_reservation(16_000, 16, 1024, limits).unwrap();
        crate::service::sampler_export::tests::event(&builder);
        builder.submit(Request::Import(clip_wav)).unwrap();
        let imported = crate::service::sampler_export::tests::event(&builder);
        assert!(imported.error.is_none());
        let audio_track = imported.state.project.tracks()[0].id();
        let audio_clip = imported.state.project.tracks()[0].clips()[0].id();
        crate::service::sampler_export::tests::assign(&builder, original.source().to_owned());
        builder
            .submit(Request::Edit(Command::AddTrack {
                name: "dest".into(),
            }))
            .unwrap();
        let with_dest = crate::service::sampler_export::tests::event(&builder);
        assert!(with_dest.error.is_none());
        let midi_track = with_dest
            .state
            .project
            .tracks()
            .iter()
            .find(|track| track.instrument().is_some())
            .unwrap()
            .id();
        builder
            .submit(Request::Edit(Command::PlaceMidiClip {
                track: midi_track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(1).unwrap(),
            }))
            .unwrap();
        let placed = crate::service::sampler_export::tests::event(&builder);
        assert!(placed.error.is_none());
        let Created::Clip(midi_clip) = placed.created.unwrap() else {
            panic!("expected midi clip")
        };
        builder
            .submit(Request::Edit(Command::AddMidiNote {
                clip: midi_clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(1).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 127,
            }))
            .unwrap();
        assert!(
            crate::service::sampler_export::tests::event(&builder)
                .error
                .is_none()
        );
        builder.submit(Request::SaveNew(saved.clone())).unwrap();
        assert!(
            crate::service::sampler_export::tests::event(&builder)
                .error
                .is_none()
        );
        let saved_project = hex_storage::load(&saved).unwrap();

        // Open as a genuine VerifiedStreaming Session retaining verified file + native original custody.
        let old = Session::new(16_000, 16, 1024)
            .unwrap()
            .with_native_reservation(limits)
            .unwrap();
        let session = crate::service::recovery::prepare_project(
            &old,
            16,
            saved_project,
            saved.parent().unwrap(),
            DocumentAudioMode::VerifiedStreaming,
            Some(reservation(2)),
            Some(limits),
            1024,
            crate::service::sampler_export::Admission::default(),
        )
        .unwrap();
        assert_eq!(session.audio_mode(), DocumentAudioMode::VerifiedStreaming);
        assert!(session.uses_ordinary_worker_publication());
        let before_snapshot = session.project().snapshot();
        let before_undo = session.can_undo();
        let context = hex_engine::AudioSourceContext {
            project_generation: 5,
            editor_revision: 12,
        };
        let canonical = Arc::new(CanonicalAudioSourceSnapshot::capture(
            &session.project_arc(),
            context.project_generation,
            context.editor_revision,
            &session,
        ));
        assert!(canonical.uses_ordinary_worker_publication());
        assert!(
            canonical
                .sources()
                .values()
                .any(|binding| matches!(binding, crate::AudioSourceBinding::VerifiedFile(_)))
        );

        let mut sink = BorrowedCatalogSink {
            reject_catalog: false,
            publish_calls: 0,
            effect_cleanups: 0,
        };
        let inputs = session
            .document_replacement_inputs(
                &old,
                crate::service::sampler_export::Admission::default(),
                None,
            )
            .unwrap();
        let current = inputs
            .prepare_for_session(session.project(), &session, &mut sink, context)
            .unwrap();

        // 1. Existing closed verified constructor refuses private sampler projects.
        let audio_target = ArrangementMoveAudition {
            context,
            gesture: 1,
            sequence: 1,
            clips: vec![audio_clip].into(),
            anchor: audio_clip,
            destination: ArrangementMoveDestination::Existing(audio_track),
            position: ClipGroupPosition::Audio(2048),
            mode: ArrangementMoveMode::Move,
        };
        assert!(
            VerifiedClipMoveSource::capture(Arc::clone(&canonical), audio_target.clone()).is_err()
        );

        // 2. Borrowed ordinary preparation builds prospective audio clip placement without publishing.
        let cancel = Arc::new(AtomicBool::new(false));
        let audio_audition = PreparedVerifiedClipAudition::prepare_on_control(
            &session,
            Arc::clone(&canonical),
            audio_target.clone(),
            &current,
            &mut sink,
            crate::service::sampler_export::Admission::default(),
            &cancel,
        )
        .unwrap();
        assert!(Arc::ptr_eq(audio_audition.canonical(), &canonical));
        assert_eq!(audio_audition.identity(), audio_target.identity());
        assert_eq!(
            audio_audition
                .file_sources()
                .unwrap()
                .metadata()
                .catalog_generation(),
            7
        );
        assert_eq!(
            audio_audition.project().track(audio_track).unwrap().clips()[0].start(),
            2048
        );
        assert!(
            audio_audition
                .prepared()
                .arrangement_stream_sources()
                .next()
                .is_some()
        );
        let mut audio_playback = audio_audition.build_playback_on_control().unwrap();
        assert!(audio_playback.contains_streamed_sources());
        let stream_key = audio_audition
            .prepared()
            .arrangement_stream_sources()
            .next()
            .unwrap()
            .key();
        let mut stream_pages = TestStreamPages {
            key: Some(stream_key),
            frames: vec![[0.25, 0.25]; 1024],
        };
        let mut empty_pages = TestStreamPages {
            key: None,
            frames: Vec::new(),
        };
        // Sampler note still renders at beat 0 (frames 0..1024) and moved verified WAV clip renders at 2048..3072.
        assert!(render_peak_in_window(&mut audio_playback, &mut empty_pages, 0, 1024) > 0.01);
        assert!(render_peak_in_window(&mut audio_playback, &mut stream_pages, 2048, 1024) > 0.2);
        drop(audio_playback);
        drop(audio_audition);
        assert_eq!(session.project().snapshot(), before_snapshot);
        assert_eq!(session.can_undo(), before_undo);
        assert_eq!(sink.publish_calls, 0);

        // 3. Borrowed ordinary preparation builds prospective MIDI clip placement (beat 0 -> beat 1 = 8000 frames).
        let midi_target = ArrangementMoveAudition {
            context,
            gesture: 2,
            sequence: 1,
            clips: vec![midi_clip].into(),
            anchor: midi_clip,
            destination: ArrangementMoveDestination::Existing(midi_track),
            position: ClipGroupPosition::Midi(BeatTime::ONE),
            mode: ArrangementMoveMode::Move,
        };
        let midi_audition = PreparedVerifiedClipAudition::prepare_on_control(
            &session,
            Arc::clone(&canonical),
            midi_target.clone(),
            &current,
            &mut sink,
            crate::service::sampler_export::Admission::default(),
            &cancel,
        )
        .unwrap();
        assert_eq!(midi_audition.identity(), midi_target.identity());
        let mut midi_playback = midi_audition.build_playback_on_control().unwrap();
        // Beat 0 (frames 0..2048) has no sampler note (empty streamed pages isolate MIDI instrument render).
        assert_eq!(
            render_peak_in_window(&mut midi_playback, &mut empty_pages, 0, 2048),
            0.0
        );
        // Beat 1 (frame 8000 at 16 kHz / 120 BPM) renders the audible Sampler note before any canonical edit!
        assert!(render_peak_in_window(&mut midi_playback, &mut empty_pages, 8000, 2048) > 0.01);
        drop(midi_playback);
        drop(midi_audition);
        assert_eq!(session.project().snapshot(), before_snapshot);
        assert_eq!(session.can_undo(), before_undo);
        assert_eq!(sink.publish_calls, 0);

        // 4. Stale context, invalid target, cancellation, and missing host file catalog all refuse cleanly.
        let mut stale = midi_target.clone();
        stale.context.editor_revision += 1;
        assert!(
            PreparedVerifiedClipAudition::prepare_on_control(
                &session,
                Arc::clone(&canonical),
                stale,
                &current,
                &mut sink,
                crate::service::sampler_export::Admission::default(),
                &cancel,
            )
            .is_err()
        );
        let mut invalid = midi_target.clone();
        invalid.gesture = 0;
        assert!(
            PreparedVerifiedClipAudition::prepare_on_control(
                &session,
                Arc::clone(&canonical),
                invalid,
                &current,
                &mut sink,
                crate::service::sampler_export::Admission::default(),
                &cancel,
            )
            .is_err()
        );
        let cancelled = Arc::new(AtomicBool::new(true));
        assert!(
            PreparedVerifiedClipAudition::prepare_on_control(
                &session,
                Arc::clone(&canonical),
                midi_target.clone(),
                &current,
                &mut sink,
                crate::service::sampler_export::Admission::default(),
                &cancelled,
            )
            .is_err()
        );
        // Missing borrowed file catalog must refuse before touching the sink.
        let mut current_missing_files = inputs
            .prepare_for_session(session.project(), &session, &mut sink, context)
            .unwrap();
        current_missing_files.file_sources = None;
        assert!(
            PreparedVerifiedClipAudition::prepare_on_control(
                &session,
                Arc::clone(&canonical),
                midi_target.clone(),
                &current_missing_files,
                &mut sink,
                crate::service::sampler_export::Admission::default(),
                &cancel,
            )
            .is_err()
        );
        // Host catalog refusal during non-incremental move (new track) must refuse cleanly and run cleanup.
        let new_track_target = ArrangementMoveAudition {
            context,
            gesture: 3,
            sequence: 1,
            clips: vec![audio_clip].into(),
            anchor: audio_clip,
            destination: ArrangementMoveDestination::NewTrack(
                NewTrackMoveDestination::new(Arc::from("audition-dest"), None, 1.0).unwrap(),
            ),
            position: ClipGroupPosition::Audio(2048),
            mode: ArrangementMoveMode::Move,
        };
        let mut failing_sink = BorrowedCatalogSink {
            reject_catalog: true,
            publish_calls: 0,
            effect_cleanups: 0,
        };
        assert!(
            PreparedVerifiedClipAudition::prepare_on_control(
                &session,
                Arc::clone(&canonical),
                new_track_target,
                &current,
                &mut failing_sink,
                crate::service::sampler_export::Admission::default(),
                &cancel,
            )
            .is_err()
        );
        assert_eq!(failing_sink.effect_cleanups, 1);
        assert_eq!(failing_sink.publish_calls, 0);
        // Mismatched current context must refuse before touching the sink.
        let mut wrong_context_current = current_missing_files;
        wrong_context_current.prepared_context.editor_revision += 1;
        assert!(
            PreparedVerifiedClipAudition::prepare_on_control(
                &session,
                Arc::clone(&canonical),
                midi_target,
                &wrong_context_current,
                &mut sink,
                crate::service::sampler_export::Admission::default(),
                &cancel,
            )
            .is_err()
        );
        assert_eq!(sink.effect_cleanups, 2);
        assert_eq!(session.project().snapshot(), before_snapshot);
    }

    #[test]
    fn verified_midi_synth_clip_audition_refuses_closed_constructor_and_prepares_via_borrowed_ordinary_path()
     {
        let clip_files = Files::new();
        let clip_wav = clip_files.wav("synth-doc.wav", 16_000, 16, 1024);
        let mut limits = crate::service::sampler_export::tests::reservation();
        limits.storage.max_retained_bytes = hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 2;
        limits.storage.max_construction_peak_bytes =
            hex_engine::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES * 3;
        let mut session = Session::new_with_audio_mode(
            16_000,
            16,
            1024,
            DocumentAudioMode::VerifiedStreaming,
            Some(reservation(2)),
        )
        .unwrap()
        .with_native_reservation(limits)
        .unwrap();
        let old = Session::new(16_000, 16, 1024)
            .unwrap()
            .with_native_reservation(limits)
            .unwrap();
        let verified_file = session
            .verify_import_with_pending(clip_wav, &[], || false)
            .unwrap();
        session
            .import_verified_revision(
                verified_file,
                crate::service::VerifiedImportDestination::NewTrack {
                    before: None,
                    start: 0,
                },
                None,
            )
            .unwrap();
        assert!(!session.uses_ordinary_worker_publication());

        let Created::Track(midi_track) = session
            .editor
            .execute(Command::AddMidiTrack {
                name: "synth".into(),
            })
            .unwrap()
        else {
            panic!("expected midi track")
        };
        let synth_state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
        )
        .unwrap();
        session
            .editor
            .execute(Command::InsertInstrument {
                track: midi_track,
                state: synth_state,
            })
            .unwrap();
        let Created::Clip(midi_clip) = session
            .editor
            .execute(Command::PlaceMidiClip {
                track: midi_track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(1).unwrap(),
            })
            .unwrap()
        else {
            panic!("expected midi clip")
        };
        session
            .editor
            .execute(Command::AddMidiNote {
                clip: midi_clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_quarters(1).unwrap(),
                pitch: 60,
                channel: 0,
                velocity: 120,
            })
            .unwrap();
        assert!(session.uses_ordinary_worker_publication());
        let before_snapshot = session.project().snapshot();
        let context = hex_engine::AudioSourceContext {
            project_generation: 2,
            editor_revision: 6,
        };
        let canonical = Arc::new(CanonicalAudioSourceSnapshot::capture(
            &session.project_arc(),
            context.project_generation,
            context.editor_revision,
            &session,
        ));
        assert!(canonical.uses_ordinary_worker_publication());

        let target = ArrangementMoveAudition {
            context,
            gesture: 1,
            sequence: 1,
            clips: vec![midi_clip].into(),
            anchor: midi_clip,
            destination: ArrangementMoveDestination::Existing(midi_track),
            position: ClipGroupPosition::Midi(BeatTime::ONE),
            mode: ArrangementMoveMode::Move,
        };

        // Reproducer: the closed verified constructor refuses MIDI/instrument graphs.
        let closed =
            VerifiedClipMoveSource::capture(Arc::clone(&canonical), target.clone()).unwrap();
        assert!(matches!(
            closed.candidate_reservation_quote(),
            Err(hex_engine::AudioStorageError::UnsupportedGraph)
        ));

        // Source-bound borrowed ordinary preparation succeeds and renders prospective MIDI placement.
        let mut sink = BorrowedCatalogSink {
            reject_catalog: false,
            publish_calls: 0,
            effect_cleanups: 0,
        };
        let inputs = session
            .document_replacement_inputs(
                &old,
                crate::service::sampler_export::Admission::default(),
                None,
            )
            .unwrap();
        let current = inputs
            .prepare_for_session(session.project(), &session, &mut sink, context)
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let audition = PreparedVerifiedClipAudition::prepare_on_control(
            &session,
            Arc::clone(&canonical),
            target,
            &current,
            &mut sink,
            crate::service::sampler_export::Admission::default(),
            &cancel,
        )
        .unwrap();
        let mut playback = audition.build_playback_on_control().unwrap();
        assert!(playback.contains_streamed_sources());
        let mut empty_pages = TestStreamPages {
            key: None,
            frames: Vec::new(),
        };
        assert_eq!(
            render_peak_in_window(&mut playback, &mut empty_pages, 0, 2048),
            0.0
        );
        assert!(render_peak_in_window(&mut playback, &mut empty_pages, 8000, 2048) > 0.01);
        drop(playback);
        drop(audition);
        assert_eq!(sink.publish_calls, 0);
        assert_eq!(sink.effect_cleanups, 1);
        assert_eq!(session.project().snapshot(), before_snapshot);
    }
}
