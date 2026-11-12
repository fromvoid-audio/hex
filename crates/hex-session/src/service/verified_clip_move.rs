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
}
