//! Sole owning Session producer for original-Sampler construction writers.
use super::*;
use hex_project::{
    Command, OriginalSamplerEditorTransition, OriginalSamplerTransitionRequest, Project,
};
use std::cell::RefMut;

/// Purpose is independent of the cleanup phase. An initial source never owns
/// an Editor transition and cannot use that transition's commit operation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingSourcePurpose {
    Edit,
    Initial,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum PendingSourcePhase {
    #[default]
    Vacant,
    Requested,
    Building,
    Built,
    NoChange,
    Refused,
    Committed,
    NoChangeCommitted,
    Cleaning,
    Failed,
}
/// Actual owning metadata in the existing Session. This is not a source pool,
/// admission permit or an owning getter. Failed cleanup is permanently rooted.
#[derive(Default)]
pub(crate) struct PendingSource {
    phase: PendingSourcePhase,
    purpose: Option<PendingSourcePurpose>,
    context: Option<hex_engine::AudioSourceContext>,
    project: Option<crate::SharedProject>,
    source: Option<crate::service::SharedCanonicalAudioSource>,
    bindings: Option<SortedEntries<AssetId, crate::AudioSourceBinding>>,
    first_panic: Option<Box<dyn std::any::Any + Send>>,
    #[cfg(test)]
    cleanup_probe: Option<PendingCleanupProbe>,
}
impl PendingSource {
    pub(super) fn vacant(&self) -> bool {
        self.phase == PendingSourcePhase::Vacant
            && self.purpose.is_none()
            && self.context.is_none()
            && self.project.is_none()
            && self.source.is_none()
            && self.bindings.is_none()
            && self.first_panic.is_none()
            && {
                #[cfg(test)]
                {
                    self.cleanup_probe.is_none()
                }
                #[cfg(not(test))]
                {
                    true
                }
            }
    }
    pub(super) fn inspect(
        &self,
    ) -> Result<
        hex_engine::sampler_host_fields::OriginalSamplerPendingSourceFields<'_>,
        AudioStorageError,
    > {
        use hex_engine::sampler_host_fields::{BodyRef, OriginalSamplerPendingSourceFields};
        let Self {
            phase,
            purpose,
            context,
            project,
            source,
            bindings,
            first_panic,
            #[cfg(test)]
            cleanup_probe,
        } = self;
        #[cfg(test)]
        if cleanup_probe.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        if first_panic.is_some() || bindings.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        if *phase != PendingSourcePhase::Vacant {
            match purpose {
                Some(PendingSourcePurpose::Edit) => {}
                Some(PendingSourcePurpose::Initial)
                    if context.is_some_and(|context| {
                        context.project_generation == 0 && context.editor_revision == 0
                    }) => {}
                _ => return Err(AudioStorageError::SourceMismatch),
            }
        }
        match phase {
            PendingSourcePhase::Vacant if self.vacant() => {}
            PendingSourcePhase::Requested
            | PendingSourcePhase::NoChange
            | PendingSourcePhase::NoChangeCommitted
                if context.is_some() && project.is_none() && source.is_none() => {}
            PendingSourcePhase::Built | PendingSourcePhase::Committed => {
                let (Some(project), Some(source), Some(context)) = (project, source, context)
                else {
                    return Err(AudioStorageError::UnknownStorage);
                };
                if !project.shares_owner_with(source.project_owner_on_control())
                    || source.context() != *context
                {
                    return Err(AudioStorageError::SourceMismatch);
                }
            }
            _ => return Err(AudioStorageError::UnknownStorage),
        }
        Ok(OriginalSamplerPendingSourceFields {
            body: BodyRef::of(self),
            project: project.as_ref(),
            source: source
                .as_ref()
                .map(crate::service::sampler_captured_source_fields_on_control),
        })
    }
    fn clear_on_control(&mut self) -> Result<(), AudioStorageError> {
        if matches!(
            self.phase,
            PendingSourcePhase::Failed | PendingSourcePhase::Cleaning
        ) || self.first_panic.is_some()
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        if self.phase == PendingSourcePhase::Building {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.phase = PendingSourcePhase::Cleaning;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(test)]
            drop(self.cleanup_probe.take());
            drop(self.source.take());
            drop(self.project.take());
            drop(self.bindings.take());
        })) {
            Ok(()) => {
                self.context = None;
                self.purpose = None;
                self.phase = PendingSourcePhase::Vacant;
                Ok(())
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = PendingSourcePhase::Failed;
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
}

#[cfg(test)]
struct PendingCleanupProbe(std::sync::Arc<std::sync::atomic::AtomicUsize>);
#[cfg(test)]
impl Drop for PendingCleanupProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        panic!("actual pending source metadata cleanup fault");
    }
}
#[cfg(test)]
mod tests;

/// Held writers cannot escape their actual Session, and inspection cannot
/// return borrowed records across a mutable construction reborrow.
/// ```compile_fail
/// let session = hex_session::Session::new(48000, 16, 1024).unwrap();
/// let loan = session.original_sampler_session_loan_on_control().unwrap();
/// let escaped = loan.with_inventory(|inventory| Ok(inventory));
/// ```
/// The pending pair is borrowed from the SAME held loan, never extracted into
/// an owning view or carried across the mutable constructor reborrow.
/// ```compile_fail
/// use hex_engine::sampler_host_fields::OriginalSamplerPendingSession;
/// let session = hex_session::Session::new(48000, 16, 1024).unwrap();
/// let mut request = Some(hex_project::OriginalSamplerTransitionRequest::undo());
/// let escaped = {
///     let mut loan = session.original_sampler_pending_session_loan_on_control(
///         &mut request,
///         hex_engine::AudioSourceContext { project_generation: 0, editor_revision: 1 },
///     ).unwrap();
///     loan.source_and_writer_on_control().unwrap().1
/// };
/// let _ = escaped.payload.project();
/// ```
/// ```compile_fail
/// fn requires_send<T: Send>(_: T) {}
/// let session = hex_session::Session::new(48000, 16, 1024).unwrap();
/// let mut request = Some(hex_project::OriginalSamplerTransitionRequest::undo());
/// let loan = session.original_sampler_pending_session_loan_on_control(
///     &mut request,
///     hex_engine::AudioSourceContext { project_generation: 0, editor_revision: 1 },
/// ).unwrap();
/// requires_send(loan);
/// ```
pub struct OriginalSamplerSessionLoan<'a> {
    session: &'a Session,
    pending_source: RefMut<'a, PendingSource>,
    editor_pending: Option<OriginalSamplerEditorTransition<'a>>,
    waveforms: RefMut<'a, SortedEntries<ClipId, AudioClipWaveform>>,
    transaction: hex_engine::audio_transform::OriginalSamplerTransaction<'a>,
}
impl Session {
    /// Existing order: waveform then cache/pending/tempo, before native guards.
    /// No allocation, pending write or candidate mutation during acquisition.
    pub fn original_sampler_session_loan_on_control(
        &self,
    ) -> Result<OriginalSamplerSessionLoan<'_>, AudioStorageError> {
        self.editor
            .require_no_original_sampler_pending_on_control()
            .map_err(|_| AudioStorageError::ExternalReadersPending)?;
        let (pending_source, waveforms, transaction) =
            self.original_sampler_writers_on_control()?;
        Ok(OriginalSamplerSessionLoan {
            session: self,
            pending_source,
            editor_pending: None,
            waveforms,
            transaction,
        })
    }
    /// One actual pending guard, then metadata, waveform and cache writers.
    /// A busy later writer returns the SAME untouched request without a clone.
    pub fn original_sampler_pending_session_loan_on_control(
        &self,
        request: &mut Option<OriginalSamplerTransitionRequest>,
        context: hex_engine::AudioSourceContext,
    ) -> Result<OriginalSamplerSessionLoan<'_>, AudioStorageError> {
        let mut pending = self
            .editor
            .original_sampler_transition_on_control(request)
            .map_err(|_| AudioStorageError::ExternalReadersPending)?;
        let acquired = self.original_sampler_writers_on_control();
        let (mut pending_source, waveforms, transaction) = match acquired {
            Ok(writers) => writers,
            Err(error) => {
                pending
                    .return_unprepared_request_on_control(request)
                    .map_err(|_| AudioStorageError::UnknownStorage)?;
                return Err(error);
            }
        };
        pending_source.context = Some(context);
        pending_source.purpose = Some(PendingSourcePurpose::Edit);
        pending_source.phase = PendingSourcePhase::Requested;
        Ok(OriginalSamplerSessionLoan {
            session: self,
            pending_source,
            editor_pending: Some(pending),
            waveforms,
            transaction,
        })
    }
    // Only the actual pre-current worker view can call this. Acquiring writers
    // is nonallocating; the closed initial prefix is still required before
    // any Project clone, binding reserve, paired metadata or view construction.
    pub(crate) fn original_sampler_initial_writers_on_control(
        &self,
    ) -> Result<OriginalSamplerSessionLoan<'_>, AudioStorageError> {
        self.editor
            .require_no_original_sampler_pending_on_control()
            .map_err(|_| AudioStorageError::ExternalReadersPending)?;
        let (mut pending_source, waveforms, transaction) =
            self.original_sampler_writers_on_control()?;
        pending_source.purpose = Some(PendingSourcePurpose::Initial);
        pending_source.context = Some(hex_engine::AudioSourceContext {
            project_generation: 0,
            editor_revision: 0,
        });
        pending_source.phase = PendingSourcePhase::Requested;
        Ok(OriginalSamplerSessionLoan {
            session: self,
            pending_source,
            editor_pending: None,
            waveforms,
            transaction,
        })
    }
    // Retry only the SAME already-entered Initial purpose after a nonmutating
    // prefix refusal. This cannot begin a different operation or clear failure.
    pub(crate) fn resume_original_sampler_initial_writers_on_control(
        &self,
    ) -> Result<OriginalSamplerSessionLoan<'_>, AudioStorageError> {
        self.editor
            .require_no_original_sampler_pending_on_control()
            .map_err(|_| AudioStorageError::ExternalReadersPending)?;
        let pending_source = self
            .original_sampler_source
            .try_borrow_mut()
            .map_err(|_| AudioStorageError::ExternalReadersPending)?;
        pending_source.inspect()?;
        if pending_source.purpose != Some(PendingSourcePurpose::Initial)
            || !matches!(
                pending_source.phase,
                PendingSourcePhase::Requested | PendingSourcePhase::Built
            )
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let (waveforms, transaction) = self.original_sampler_cache_writers_on_control()?;
        Ok(OriginalSamplerSessionLoan {
            session: self,
            pending_source,
            editor_pending: None,
            waveforms,
            transaction,
        })
    }
    #[expect(
        clippy::type_complexity,
        reason = "three existing disjoint Session writers"
    )]
    fn original_sampler_writers_on_control(
        &self,
    ) -> Result<
        (
            RefMut<'_, PendingSource>,
            RefMut<'_, SortedEntries<ClipId, AudioClipWaveform>>,
            hex_engine::audio_transform::OriginalSamplerTransaction<'_>,
        ),
        AudioStorageError,
    > {
        let pending_source = self
            .original_sampler_source
            .try_borrow_mut()
            .map_err(|_| AudioStorageError::ExternalReadersPending)?;
        if !pending_source.vacant() {
            return Err(AudioStorageError::UnknownStorage);
        }
        let (waveforms, transaction) = self.original_sampler_cache_writers_on_control()?;
        Ok((pending_source, waveforms, transaction))
    }
    fn original_sampler_cache_writers_on_control(
        &self,
    ) -> Result<
        (
            RefMut<'_, SortedEntries<ClipId, AudioClipWaveform>>,
            hex_engine::audio_transform::OriginalSamplerTransaction<'_>,
        ),
        AudioStorageError,
    > {
        let waveforms = self
            .audio_clip_waveforms
            .try_borrow_mut()
            .map_err(|_| AudioStorageError::ExternalReadersPending)?;
        let transaction = self
            .audio_transforms
            .original_sampler_transaction_on_control()?;
        Ok((waveforms, transaction))
    }
    /// Called only after every readonly/native/writer loan has ended. No owner
    /// is returned; failed metadata remains in this actual Session permanently.
    pub fn clear_original_sampler_pending_on_control(&mut self) -> Result<(), AudioStorageError> {
        let source = self.original_sampler_source.get_mut();
        if source.phase == PendingSourcePhase::Failed || source.first_panic.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        // Keep the model's SAME nonidle candidate latch while paired metadata
        // is destructed. A metadata unwind must not expose an idle Editor.
        let purpose = source.purpose;
        source.clear_on_control()?;
        if purpose == Some(PendingSourcePurpose::Initial) {
            return Ok(());
        }
        if self
            .editor
            .clear_original_sampler_transition_on_control()
            .is_err()
        {
            source.phase = PendingSourcePhase::Failed;
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(())
    }
    /// Fixed model commit only after ALL writer/native/readonly loans end. The
    /// new source remains owned here; this is not take/publication authority.
    pub fn commit_original_sampler_pending_on_control(
        &mut self,
    ) -> Result<hex_project::OriginalSamplerTransitionResult, AudioStorageError> {
        let source = self.original_sampler_source.get_mut();
        source.inspect()?;
        if source.purpose != Some(PendingSourcePurpose::Edit) {
            return Err(AudioStorageError::SourceMismatch);
        }
        if !matches!(
            source.phase,
            PendingSourcePhase::Built | PendingSourcePhase::NoChange
        ) {
            return Err(AudioStorageError::SourceMismatch);
        }
        {
            let (candidate, prepared) = self
                .editor
                .original_sampler_prepared_candidate_on_control()
                .map_err(|_| AudioStorageError::SourceMismatch)?;
            if prepared.changed != (source.phase == PendingSourcePhase::Built)
                || source
                    .project
                    .as_ref()
                    .is_some_and(|project| project.as_ref() != candidate)
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        let result = self
            .editor
            .commit_original_sampler_transition_on_control()
            .map_err(|_| AudioStorageError::UnknownStorage)?;
        source.phase = if result.changed {
            PendingSourcePhase::Committed
        } else {
            PendingSourcePhase::NoChangeCommitted
        };
        Ok(result)
    }
}

// Both purposes use the SAME persistent partial fields. The purpose-specific
// closed Engine caller must quote/preflight before reaching either producer.
// No local Project/bindings owner can escape a failed construction step.
fn capture_pair_on_control(
    session: &Session,
    source: &mut PendingSource,
    candidate: &Project,
    context: hex_engine::AudioSourceContext,
) -> Result<(), AudioStorageError> {
    if source.phase != PendingSourcePhase::Building
        || source.project.is_some()
        || source.source.is_some()
        || source.bindings.is_some()
    {
        return Err(AudioStorageError::SourceMismatch);
    }
    source.project = Some(crate::SharedProject::new_on_control(candidate.clone()));
    let project = source
        .project
        .as_ref()
        .ok_or(AudioStorageError::SourceMismatch)?;
    let count = project.asset_entries_on_control().len();
    source.bindings = Some(SortedEntries::new());
    let bindings = source
        .bindings
        .as_mut()
        .ok_or(AudioStorageError::SourceMismatch)?;
    bindings
        .try_reserve_on_control(count, hex_project::MAX_ASSETS)
        .map_err(|_| AudioStorageError::Allocation)?;
    if bindings.capacity() != count {
        return Err(AudioStorageError::ReservationExceeded);
    }
    for asset in project.assets() {
        let audio = session
            .audio(asset.id())
            .ok_or(AudioStorageError::SourceMismatch)?;
        if bindings
            .try_insert_on_control(
                asset.id(),
                crate::AudioSourceBinding::Resident(audio.clone()),
                hex_project::MAX_ASSETS,
            )
            .map_err(|_| AudioStorageError::Allocation)?
            .is_some()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
    }
    let payload = crate::service::canonical_audio_source::CanonicalAudioSourceSnapshot::from_pending_original_on_control(
                project, context, &mut source.bindings,
            )?;
    source.source = Some(crate::service::SharedCanonicalAudioSource::new_on_control(
        payload,
    ));
    Ok(())
}

impl<'owner> hex_engine::sampler_host_fields::OriginalSamplerPendingSession<'owner>
    for OriginalSamplerSessionLoan<'owner>
{
    type Fields<'scope>
        = OriginalSamplerResidentView<'scope>
    where
        Self: 'scope;
    fn pending_context_on_control(
        &self,
    ) -> Result<hex_engine::AudioSourceContext, AudioStorageError> {
        self.pending_source
            .context
            .ok_or(AudioStorageError::SourceMismatch)
    }
    fn fields_and_writer_on_control<'scope>(
        &'scope mut self,
    ) -> (
        Self::Fields<'scope>,
        &'scope mut hex_engine::audio_transform::OriginalSamplerTransaction<'owner>,
    ) {
        self.resident_fields_and_writer_on_control()
    }

    fn clone_source_on_control(&self) -> Result<Option<&Project>, AudioStorageError> {
        let pending = self
            .editor_pending
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?;
        if self.pending_source.phase != PendingSourcePhase::Requested
            || self.pending_source.purpose != Some(PendingSourcePurpose::Edit)
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(pending.clone_source_on_control())
    }
    fn replacement_elements_on_control(&self) -> Result<(usize, usize), AudioStorageError> {
        self.editor_pending
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?
            .replacement_elements_on_control()
            .map_err(|_| AudioStorageError::SourceMismatch)
    }
    fn command_on_control(&self) -> Result<Option<&Command>, AudioStorageError> {
        let pending = self
            .editor_pending
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?;
        Ok(pending
            .storage()
            .request()
            .and_then(|request| request.command_value()))
    }
    fn stage_and_capture_on_control(&mut self) -> Result<bool, AudioStorageError> {
        use hex_engine::CancellationQuery;
        if self.pending_source.phase != PendingSourcePhase::Requested
            || self.pending_source.purpose != Some(PendingSourcePurpose::Edit)
            || self.pending_source.first_panic.is_some()
            || self.session.shutdown.is_cancelled()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let pending = self
            .editor_pending
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        let context = self
            .pending_source
            .context
            .ok_or(AudioStorageError::SourceMismatch)?;
        self.pending_source.phase = PendingSourcePhase::Building;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pending
                .stage_original_sampler_after_prefix_on_control()
                .map_err(|_| AudioStorageError::SourceMismatch)?;
            if !pending
                .prepared_result_on_control()
                .map_err(|_| AudioStorageError::SourceMismatch)?
                .changed
            {
                return Ok(false);
            }
            let candidate = pending
                .storage()
                .candidate()
                .ok_or(AudioStorageError::SourceMismatch)?;
            capture_pair_on_control(self.session, &mut self.pending_source, candidate, context)?;
            Ok(true)
        }));
        match outcome {
            Ok(Ok(changed)) => {
                self.pending_source.phase = if changed {
                    PendingSourcePhase::Built
                } else {
                    PendingSourcePhase::NoChange
                };
                Ok(changed)
            }
            Ok(Err(error)) => {
                self.pending_source.phase = PendingSourcePhase::Refused;
                Err(error)
            }
            Err(payload) => {
                self.pending_source.first_panic = Some(payload);
                self.pending_source.phase = PendingSourcePhase::Failed;
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
    fn source_and_writer_on_control<'scope>(
        &'scope mut self,
    ) -> Result<
        (
            Self::Fields<'scope>,
            hex_engine::sampler_host_fields::CapturedSourceFields<'scope>,
            &'scope mut hex_engine::audio_transform::OriginalSamplerTransaction<'owner>,
        ),
        AudioStorageError,
    > {
        self.pending_source.inspect()?;
        if self.pending_source.phase != PendingSourcePhase::Built {
            return Err(AudioStorageError::SourceMismatch);
        }
        let (view, writers) = self.resident_fields_and_writer_on_control();
        let source = view
            .pending_source
            .source
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?;
        Ok((
            view,
            crate::service::sampler_captured_source_fields_on_control(source),
            writers,
        ))
    }
}
impl<'a> OriginalSamplerSessionLoan<'a> {
    /// Actual media/history/waveform/transform fields under the SAME writers.
    /// The inspection result is never constructor authority: the selected
    /// constructor still performs mandatory normal binding and both cap checks.
    pub fn with_inventory<T>(
        &self,
        visit: impl for<'scope> FnOnce(
            &'scope ResidentPcmInventory<'scope>,
        ) -> Result<T, AudioStorageError>,
    ) -> Result<T, AudioStorageError> {
        let transforms = self.transaction.inventory();
        let inventory = ResidentPcmInventory {
            session: self.session,
            pending_source: &self.pending_source,
            pending_model: self.editor_pending.as_ref().map(
                hex_engine::sampler_host_fields::OriginalSamplerPendingModelFields::from_transition,
            ),
            waveforms: &self.waveforms,
            transforms: &transforms,
        };
        visit(&inventory)
    }
    /// Sole private Audio construction consumer; source policy restricts this
    /// internal integration accessor. No owner is extracted and no guard ends.
    pub fn original_sampler_writer_on_control(
        &mut self,
    ) -> &mut hex_engine::audio_transform::OriginalSamplerTransaction<'a> {
        &mut self.transaction
    }
}

/// Borrowed real Session fields, produced only from held Session writers.
/// It owns no media, cache, waveform, source snapshot or admission receipt.
/// ```compile_fail
/// let session = hex_session::Session::new(48000, 16, 1024).unwrap();
/// let fields = {
///     let mut loan = session.original_sampler_session_loan_on_control().unwrap();
///     let (fields, _writers) = loan.resident_fields_and_writer_on_control();
///     fields
/// };
/// let _ = hex_engine::sampler_host_fields::OriginalSamplerResidentFields::media(&fields);
/// ```
pub struct OriginalSamplerResidentView<'a> {
    session: &'a Session,
    waveforms: &'a SortedEntries<ClipId, AudioClipWaveform>,
    session_frame: hex_engine::sampler_host_fields::BodyRef<'a>,
    waveform_frame: hex_engine::sampler_host_fields::BodyRef<'a>,
    source_frame: hex_engine::sampler_host_fields::BodyRef<'a>,
    pending_frame: hex_engine::sampler_host_fields::BodyRef<'a>,
    pending_source: &'a PendingSource,
    pending_model: Option<hex_engine::sampler_host_fields::OriginalSamplerPendingModelFields<'a>>,
}
impl<'owner> OriginalSamplerSessionLoan<'owner> {
    /// Disjoint safe loans: waveform observations and actual cache writers.
    /// The readonly view never borrows cache payload through its RefCell.
    pub fn resident_fields_and_writer_on_control(
        &mut self,
    ) -> (
        OriginalSamplerResidentView<'_>,
        &mut hex_engine::audio_transform::OriginalSamplerTransaction<'owner>,
    ) {
        (
            OriginalSamplerResidentView {
                session: self.session,
                waveforms: &self.waveforms,
                session_frame: hex_engine::sampler_host_fields::BodyRef::of(&self.session),
                waveform_frame: hex_engine::sampler_host_fields::BodyRef::of(&self.waveforms),
                source_frame: hex_engine::sampler_host_fields::BodyRef::of(&self.pending_source),
                pending_frame: hex_engine::sampler_host_fields::BodyRef::of(&self.editor_pending),
                pending_source: &self.pending_source,
                pending_model: self.editor_pending.as_ref().map(
                    hex_engine::sampler_host_fields::OriginalSamplerPendingModelFields::from_transition,
                ),
            },
            &mut self.transaction,
        )
    }
}
impl hex_engine::sampler_host_fields::OriginalSamplerResidentFields
    for OriginalSamplerResidentView<'_>
{
    fn body(&self) -> hex_engine::sampler_host_fields::BodyRef<'_> {
        hex_engine::sampler_host_fields::BodyRef::of(self.session)
    }
    fn frame(&self) -> [hex_engine::sampler_host_fields::BodyRef<'_>; 5] {
        [
            hex_engine::sampler_host_fields::BodyRef::of(self),
            self.session_frame,
            self.waveform_frame,
            self.source_frame,
            self.pending_frame,
        ]
    }
    fn editor(&self) -> &hex_project::Editor {
        &self.session.editor
    }
    fn pending_editor(
        &self,
    ) -> Option<hex_engine::sampler_host_fields::OriginalSamplerPendingModelFields<'_>> {
        self.pending_model.as_ref().map(|fields| fields.reborrow())
    }
    fn pending_source(
        &self,
    ) -> Result<
        hex_engine::sampler_host_fields::OriginalSamplerPendingSourceFields<'_>,
        AudioStorageError,
    > {
        self.pending_source.inspect()
    }
    fn transforms(&self) -> &hex_engine::audio_transform::AudioTransformPreparation {
        &self.session.audio_transforms
    }
    fn cancellation(&self) -> Result<&hex_engine::RetainedControlCancellation, AudioStorageError> {
        match &self.session.shutdown {
            crate::session_cancellation::SessionCancellation::Retained(worker) => Ok(worker),
            crate::session_cancellation::SessionCancellation::Unqualified(_) => {
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
    fn sine(&self) -> &hex_engine::SharedSineTable {
        &self.session.sine_table
    }
    fn media(&self) -> hex_engine::sampler_host_fields::VecRef<'_> {
        hex_engine::sampler_host_fields::VecRef::entries(&self.session.media)
    }
    fn asset(
        &self,
        index: usize,
    ) -> Result<hex_engine::sampler_host_fields::ResidentAssetFields<'_>, AudioStorageError> {
        use hex_engine::sampler_host_fields::{
            BodyRef, ResidentAssetFields, ResidentOriginalFields,
        };
        let (asset, resident) = self
            .session
            .media
            .as_slice()
            .get(index)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let crate::ResidentAsset {
            source_audio,
            audio,
            waveform,
            frames,
        } = resident;
        let _: &u64 = frames;
        Ok(ResidentAssetFields {
            asset: *asset,
            body: BodyRef::of(resident),
            original_body: BodyRef::of(source_audio),
            original: match source_audio {
                OriginalAudio::Stereo(audio) => ResidentOriginalFields::Stereo(audio),
                OriginalAudio::Mono(source) => ResidentOriginalFields::Mono(source),
            },
            rendered: audio.as_ref(),
            waveform: hex_media::waveform::sampler_fields_on_control(waveform),
        })
    }
    fn waveforms(&self) -> hex_engine::sampler_host_fields::VecRef<'_> {
        hex_engine::sampler_host_fields::VecRef::entries(self.waveforms)
    }
    fn clip_waveform(
        &self,
        index: usize,
    ) -> Result<hex_engine::sampler_host_fields::ResidentClipWaveformFields<'_>, AudioStorageError>
    {
        let (_, clip) = self
            .waveforms
            .as_slice()
            .get(index)
            .ok_or(AudioStorageError::SourceMismatch)?;
        let AudioClipWaveform {
            waveform,
            offset,
            length,
        } = clip;
        let _: (&u64, &u64) = (offset, length);
        Ok(
            hex_engine::sampler_host_fields::ResidentClipWaveformFields {
                body: hex_engine::sampler_host_fields::BodyRef::of(clip),
                waveform: hex_media::waveform::sampler_fields_on_control(waveform),
            },
        )
    }
    fn verified_sources(&self) -> hex_engine::sampler_host_fields::VecRef<'_> {
        hex_engine::sampler_host_fields::VecRef::entries(&self.session.verified_sources)
    }
    fn validate_remaining_fields(&self) -> Result<(), AudioStorageError> {
        match (
            self.pending_source.purpose,
            self.pending_source.phase,
            self.pending_model.as_ref(),
        ) {
            (None, PendingSourcePhase::Vacant, None) if self.pending_source.vacant() => {}
            (
                Some(PendingSourcePurpose::Edit),
                PendingSourcePhase::Requested
                | PendingSourcePhase::Built
                | PendingSourcePhase::NoChange,
                Some(_),
            ) => {}
            (
                Some(PendingSourcePurpose::Initial),
                PendingSourcePhase::Requested | PendingSourcePhase::Built,
                None,
            ) => {}
            _ => return Err(AudioStorageError::SourceMismatch),
        }
        let Session {
            editor: _,
            original_sampler_source: _,
            audio_transforms: _,
            audio_clip_waveforms: _,
            shutdown: _,
            sine_table: _,
            media: _,
            verified_sources,
            semantic_source_epoch,
            audio_mode,
            source_reservation,
            resident_frames,
            max_resident_frames,
            resident_waveform_bins,
            resident_waveform_bytes,
        } = self.session;
        // Actual scalar-only current types; no ownership is inferred from a
        // counter, and every owning field is mandatory in the finite schema.
        let _: (
            u64,
            crate::DocumentAudioMode,
            Option<crate::SourcePlanReservation>,
            u64,
            u64,
            usize,
            usize,
        ) = (
            *semantic_source_epoch,
            *audio_mode,
            *source_reservation,
            *resident_frames,
            *max_resident_frames,
            *resident_waveform_bins,
            *resident_waveform_bytes,
        );
        if !verified_sources.is_empty() {
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(())
    }
}
