//! Source-bound play-start metadata. The native streaming driver keeps
//! construction, page custody and callback application; Session owns intent.
use super::*;

/// Temporary project metadata derived by executing one validated play-start
/// edit against the active authoring state. The underlying source catalog is sealed.
pub struct VerifiedArrangementPlayStartSource {
    canonical: Arc<CanonicalAudioSourceSnapshot>,
    project: Project,
    sequence: u64,
}

impl VerifiedArrangementPlayStartSource {
    fn capture(
        canonical: &Arc<CanonicalAudioSourceSnapshot>,
        project: &Project,
        target: &ArrangementPlayStartAudition,
        sequence: u64,
    ) -> Result<Self, SessionError> {
        let mut editor = hex_project::Editor::new(project.clone(), 1)?;
        editor.execute(target.edit.command())?;
        if matches!(
            target.edit,
            PlayStartEdit::Audio { .. }
                | PlayStartEdit::AudioSourcePhase { .. }
                | PlayStartEdit::AudioTrimStart { .. }
        ) {
            let clip = editor
                .project()
                .clip(target.edit.clip())
                .ok_or(SessionError::Invalid(
                    "play start audition clip unavailable",
                ))?;
            // The verified candidate constructor materializes transformed or
            // placed audio PCM only from resident bindings. Never decode or
            // substitute a verified file behind the active catalog.
            if clip.audio_edit().is_some()
                && clip.referenced_assets().any(|asset| {
                    !matches!(
                        canonical.sources().get(&asset),
                        Some(crate::AudioSourceBinding::Resident(_))
                    )
                })
            {
                return Err(SessionError::Invalid(
                    "play start audition source is unavailable for streaming",
                ));
            }
        }
        Ok(Self {
            canonical: Arc::clone(canonical),
            project: editor.project().clone(),
            sequence,
        })
    }

    pub fn canonical(&self) -> &Arc<CanonicalAudioSourceSnapshot> {
        &self.canonical
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Reserve the overlay against the canonical catalog. The native
    /// constructor still checks the exact raw and derived source identities.
    pub fn reservation_quote(
        &self,
        effects: Option<&hex_engine::AudioCandidateEffects<'_, '_>>,
    ) -> Result<hex_engine::AudioCandidateReservationQuote, hex_engine::AudioStorageError> {
        self.canonical
            .candidate_reservation_quote_for_effects(&self.project, effects)
    }

    pub fn prepare_on_control(
        &self,
        catalog: u64,
        destination: &mut hex_engine::AudioCandidatePreparation,
        effects: Option<&hex_engine::AudioCandidateEffects<'_, '_>>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        if let Some(effects) = effects {
            effects.prepare(
                destination,
                &self.project,
                catalog,
                self.canonical.context(),
                None,
            )
        } else {
            destination.prepare_on_control(&self.project, catalog, self.canonical.context())
        }
    }
}

/// Exact worker-owned intent. Release keeps the last accepted alignment until
/// the canonical source changes or the ordinary edit receipt retires the gesture.
pub enum VerifiedArrangementPlayStartUpdate {
    Preview(Arc<VerifiedArrangementPlayStartSource>),
    Cancel(Arc<VerifiedArrangementPlayStartSource>),
}

#[derive(Default)]
pub(in crate::service) struct VerifiedPlayStartLane {
    attempted: Option<ArrangementPlayStartAudition>,
    active: Option<(ClipId, Arc<VerifiedArrangementPlayStartSource>)>,
    next_attempt: Option<Instant>,
    sequence: u64,
}

impl VerifiedPlayStartLane {
    pub(in crate::service) fn drive(
        &mut self,
        mailbox: &Mailbox,
        state: &ViewState,
        project: &Project,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        available: bool,
    ) -> bool {
        let intent = mailbox.lock().intent.clone();
        if self.active.is_none() && !matches!(intent, Intent::Held(Target::PlayStart(_))) {
            self.attempted = None;
            return false;
        }
        let canonical = state.canonical_audio_source.as_ref().filter(|source| {
            available
                && source.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && context.editor_revision == state.revision
                && state.audio_authoring_revision(context) == Some(state.revision)
                && state.project.as_ref() == project
        });
        if self.active.as_ref().is_some_and(|(clip, preview)| {
            canonical.is_none_or(|source| !Arc::ptr_eq(source, preview.canonical()))
                || !matches!(
                    &intent,
                    Intent::Held(Target::PlayStart(target))
                        | Intent::Released(Target::PlayStart(target))
                        if target.context == context && target.edit.clip() == *clip
                )
        }) && let Some((_, preview)) = self.active.take()
        {
            sink.verified_arrangement_play_start_update(
                VerifiedArrangementPlayStartUpdate::Cancel(preview),
            );
            self.attempted = None;
        }
        let (Intent::Held(Target::PlayStart(target)), Some(canonical)) = (intent, canonical) else {
            self.attempted = None;
            return false;
        };
        if target.context != context || self.attempted.as_ref() == Some(&target) {
            return false;
        }
        let started = Instant::now();
        if self.next_attempt.is_some_and(|next| next > started) {
            return true;
        }
        let Some(cancel) = mailbox.claim(Target::PlayStart(target)) else {
            return true;
        };
        let Some(sequence) = self.sequence.checked_add(1) else {
            return false;
        };
        self.sequence = sequence;
        self.attempted = Some(target);
        let Ok(preview) =
            VerifiedArrangementPlayStartSource::capture(canonical, project, &target, sequence)
        else {
            self.next_attempt = Some(started + RETRY_INTERVAL);
            return false;
        };
        let preview = Arc::new(preview);
        let cancelled = cancel.load(Ordering::Acquire);
        let accepted = !cancelled
            && sink.verified_arrangement_play_start_update(
                VerifiedArrangementPlayStartUpdate::Preview(Arc::clone(&preview)),
            );
        self.next_attempt = Some(started + attempt_interval(Kind::PlayStart, !accepted, cancelled));
        if !accepted {
            self.attempted = None;
            return true;
        }
        self.active = Some((target.edit.clip(), preview));
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::verified_source_tests::{Files, next, reservation, run};

    #[test]
    fn verified_play_start_allows_plain_streamed_slips_and_refuses_streamed_transforms() {
        let files = Files::new();
        let wav = files.wav("play-start.wav", 48_000, 1_024, 8192);
        let service = Service::new(48_000, 16, 4096).unwrap();
        next(&service);
        run(
            &service,
            Request::NewWithAudioMode {
                mode: DocumentAudioMode::VerifiedStreaming,
                reservation: Some(reservation(2)),
            },
        );
        let imported = run(&service, Request::Import(wav));
        let source = imported.state.canonical_audio_source.as_ref().unwrap();
        let project = imported.state.project.as_ref();
        let clip = &project.tracks()[0].clips()[0];

        // Plain streamed TrimClip play-start shift keeps the sealed VerifiedFile binding.
        let trim = ArrangementPlayStartAudition {
            context: source.context(),
            edit: PlayStartEdit::AudioTrimStart {
                clip: clip.id(),
                start: clip.start(),
                duration: clip.duration(),
                source_offset: 256,
                source_length: 768,
            },
        };
        let preview = VerifiedArrangementPlayStartSource::capture(source, project, &trim, 1)
            .expect("plain streamed trim start must succeed");
        assert!(Arc::ptr_eq(preview.canonical(), source));
        assert_eq!(
            preview.project.clip(clip.id()).unwrap().source_offset(),
            256
        );

        // Creating a placed audio_edit on a VerifiedFile clip cannot decode behind the catalog.
        let placed = ArrangementPlayStartAudition {
            context: source.context(),
            edit: PlayStartEdit::Audio {
                clip: clip.id(),
                position: 256,
            },
        };
        assert!(matches!(
            VerifiedArrangementPlayStartSource::capture(source, project, &placed, 2),
            Err(SessionError::Invalid(
                "play start audition source is unavailable for streaming"
            ))
        ));
    }
}
