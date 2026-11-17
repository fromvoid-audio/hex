//! Source-bound gain metadata. The native streaming driver keeps construction,
//! page custody and callback application; the Session worker owns held intent.
use super::*;

/// Temporary project metadata derived by executing one validated gain edit
/// against the active authoring state. The underlying source catalog is sealed.
pub struct VerifiedArrangementGainSource {
    canonical: Arc<CanonicalAudioSourceSnapshot>,
    project: Project,
    sequence: u64,
}

impl VerifiedArrangementGainSource {
    fn capture(
        canonical: &Arc<CanonicalAudioSourceSnapshot>,
        project: &Project,
        target: &ArrangementGainAudition,
        sequence: u64,
    ) -> Result<Self, SessionError> {
        let mut editor = hex_project::Editor::new(project.clone(), 1)?;
        editor.execute(target.edit.command())?;
        if let GainEdit::Event { clip, .. } = target.edit {
            let clip = editor
                .project()
                .clip(clip)
                .ok_or(SessionError::Invalid("gain audition clip unavailable"))?;
            // The current candidate constructor materializes event PCM only
            // from resident bindings. Never decode or substitute a verified file
            // behind the active catalog to make an unsupported edit audible.
            if clip.referenced_assets().any(|asset| {
                !matches!(
                    canonical.sources().get(&asset),
                    Some(crate::AudioSourceBinding::Resident(_))
                )
            }) {
                return Err(SessionError::Invalid(
                    "gain audition event source is unavailable for streaming",
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

/// Exact worker-owned intent. Release keeps the last accepted value until the
/// canonical source changes or the ordinary edit receipt retires the gesture.
pub enum VerifiedArrangementGainUpdate {
    Preview(Arc<VerifiedArrangementGainSource>),
    Cancel(Arc<VerifiedArrangementGainSource>),
}

#[derive(Default)]
pub(in crate::service) struct VerifiedGainLane {
    attempted: Option<ArrangementGainAudition>,
    active: Option<(GainEdit, Arc<VerifiedArrangementGainSource>)>,
    next_attempt: Option<Instant>,
    sequence: u64,
}

impl VerifiedGainLane {
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
        if self.active.is_none() && !matches!(intent, Intent::Held(Target::Gain(_))) {
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
        if self.active.as_ref().is_some_and(|(edit, preview)| {
            canonical.is_none_or(|source| !Arc::ptr_eq(source, preview.canonical()))
                || !matches!(&intent, Intent::Held(Target::Gain(target)) | Intent::Released(Target::Gain(target))
                    if target.context == context && target.edit.same_gesture(edit))
        }) && let Some((_, preview)) = self.active.take()
        {
            sink.verified_arrangement_gain_update(VerifiedArrangementGainUpdate::Cancel(preview));
            self.attempted = None;
        }
        let (Intent::Held(Target::Gain(target)), Some(canonical)) = (intent, canonical) else {
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
        let Some(cancel) = mailbox.claim(Target::Gain(target.clone())) else {
            return true;
        };
        let Some(sequence) = self.sequence.checked_add(1) else {
            return false;
        };
        self.sequence = sequence;
        self.next_attempt = Some(started + RETRY_INTERVAL);
        self.attempted = Some(target.clone());
        let Ok(preview) =
            VerifiedArrangementGainSource::capture(canonical, project, &target, sequence)
        else {
            return false;
        };
        let preview = Arc::new(preview);
        if cancel.load(Ordering::Acquire)
            || !sink.verified_arrangement_gain_update(VerifiedArrangementGainUpdate::Preview(
                Arc::clone(&preview),
            ))
        {
            self.attempted = None;
            return true;
        }
        self.active = Some((target.edit, preview));
        false
    }
}
