//! Keep a validated graph edit private until the callback installs its exact graph.
use super::*;

fn device_edit_preparation_deadline() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

/// The user operation associated with an audio-confirmed graph edit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphEditOperation {
    InsertDevice,
    RemoveDevice,
    MoveDevice,
    RemoveTrack,
}

/// A one-shot decision for a staged graph edit. It grants no direct audio access.
#[derive(Clone, Debug)]
pub struct DeviceInsertConfirmation {
    submission: ResidentGraphSubmission,
    deadline: Instant,
    decision: Arc<std::sync::atomic::AtomicU8>,
    operation: GraphEditOperation,
}

impl DeviceInsertConfirmation {
    fn new(
        submission: ResidentGraphSubmission,
        deadline: Instant,
        operation: GraphEditOperation,
    ) -> Self {
        Self {
            submission,
            deadline,
            decision: Arc::default(),
            operation,
        }
    }

    pub fn submission(&self) -> ResidentGraphSubmission {
        self.submission
    }

    pub fn operation(&self) -> GraphEditOperation {
        self.operation
    }

    pub fn is_pending(&self) -> bool {
        self.progress().is_none()
    }

    pub fn proceed(&self) -> bool {
        Instant::now() < self.deadline && self.decide(1)
    }

    pub fn cancel(&self) -> bool {
        self.decide(2)
    }

    fn decide(&self, decision: u8) -> bool {
        self.decision
            .compare_exchange(0, decision, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn progress(&self) -> Option<Progress> {
        match self.decision.load(Ordering::Acquire) {
            2 => Some(Progress::Terminal(DeviceInsertAudioState::Cancelled)),
            _ if Instant::now() >= self.deadline => {
                Some(Progress::Terminal(DeviceInsertAudioState::Expired))
            }
            1 => Some(Progress::Proceed),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::service) enum Operation {
    Device(GraphEditOperation),
    HostedHistory(GraphEditOperation),
    Tempo,
}

impl Operation {
    pub(in crate::service) fn installed_status(self) -> &'static str {
        match self {
            Self::Device(GraphEditOperation::InsertDevice) => "device inserted",
            Self::Device(GraphEditOperation::RemoveDevice) => "device removed",
            Self::Device(GraphEditOperation::MoveDevice) => "device moved",
            Self::Device(GraphEditOperation::RemoveTrack) => "track removed",
            Self::HostedHistory(_) => "history committed",
            Self::Tempo => "tempo applied",
        }
    }

    pub(in crate::service) fn device(self) -> Option<GraphEditOperation> {
        match self {
            Self::Device(operation) | Self::HostedHistory(operation) => Some(operation),
            Self::Tempo => None,
        }
    }
}

pub(in crate::service) struct Pending {
    edit: StagedChange,
    prepared: Prepared,
    confirmation: Option<DeviceInsertConfirmation>,
    pub(in crate::service) operation: Operation,
    // A cut publishes its captured selection only with the canonical removal.
    pub(in crate::service) cut_devices: Option<Arc<hex_project::DeviceClipboard>>,
    pub(in crate::service) cut_tracks: Option<Arc<hex_project::TrackClipboard>>,
    pub(in crate::service) document_intent: u64,
    pub(in crate::service) plugin: Option<plugin_effect::Pending>,
    pub(in crate::service) proposal_control: Option<Arc<PluginEffectControl>>,
    pub(in crate::service) publish_created: bool,
}

enum StagedChange {
    Command(hex_project::StagedProjectEdit),
    History(hex_project::StagedProjectHistory),
}

impl StagedChange {
    fn project(&self) -> &Project {
        match self {
            Self::Command(edit) => edit.project(),
            Self::History(edge) => edge.project(),
        }
    }
}

struct Resident {
    snapshot: Snapshot,
    receipt: Receipt,
    predecessor: ResidentRuntimeLoop,
}

enum Prepared {
    Resident(Box<Resident>),
    Verified {
        attempt: VerifiedGraphEditAttempt,
        editor_revision: Arc<hex_engine::PreparedRevision>,
    },
}

/// An editor-owned immutable source shared with the native preparation worker
/// until it can bind the exact callback receipt. Never used in the callback.
#[derive(Clone)]
pub struct VerifiedGraphEditAttempt(Arc<VerifiedAttempt>);

struct VerifiedAttempt {
    source: Arc<CanonicalAudioSourceSnapshot>,
    predecessor: hex_engine::AudioSourceContext,
    retiring_gain: Option<DevicePreviewIdentity>,
    confirmation: Option<ResidentGraphSubmission>,
    tempo_cut: bool,
    hosted_history: bool,
    proposal_control: Option<Arc<PluginEffectControl>>,
    prepare_until: Instant,
    state: std::sync::Mutex<VerifiedState>,
}

enum VerifiedState {
    Preparing,
    Submitted(Receipt),
    Failed(String),
    Cancelled,
}

impl VerifiedGraphEditAttempt {
    fn new(
        source: Arc<CanonicalAudioSourceSnapshot>,
        predecessor: hex_engine::AudioSourceContext,
        retiring_gain: Option<DevicePreviewIdentity>,
        confirmation: Option<ResidentGraphSubmission>,
        tempo_cut: bool,
        hosted_history: bool,
        proposal_control: Option<Arc<PluginEffectControl>>,
    ) -> Self {
        Self(Arc::new(VerifiedAttempt {
            source,
            predecessor,
            retiring_gain,
            confirmation,
            tempo_cut,
            hosted_history,
            proposal_control,
            prepare_until: device_edit_preparation_deadline(),
            state: std::sync::Mutex::new(VerifiedState::Preparing),
        }))
    }

    pub fn source(&self) -> &Arc<CanonicalAudioSourceSnapshot> {
        &self.0.source
    }
    pub fn predecessor(&self) -> hex_engine::AudioSourceContext {
        self.0.predecessor
    }
    /// The exact track gain preview invalidated by this staged removal.
    pub fn retiring_gain(&self) -> Option<DevicePreviewIdentity> {
        self.0.retiring_gain
    }
    pub fn confirmation(&self) -> Option<ResidentGraphSubmission> {
        self.0.confirmation
    }
    pub fn needs_tempo_cut(&self) -> bool {
        self.0.tempo_cut
    }

    /// Session owns the hosted membership edge; Desktop may prepare private
    /// source bindings for the exact candidate context until its terminal.
    pub fn stages_hosted_history(&self) -> bool {
        self.0.hosted_history
    }

    /// Borrow the caller's exact proposal identity without granting native custody.
    pub fn proposal_control(&self) -> Option<&Arc<PluginEffectControl>> {
        self.0.proposal_control.as_ref()
    }

    /// Latest apply decision for a verified device edit. Audio retains this
    /// deadline with the receipt once control hands the candidate to it.
    pub fn preparation_deadline(&self) -> Instant {
        self.0.prepare_until
    }

    /// Bind the caller's cancellation to the very receipt being queued.
    pub fn bind_proposal_receipt(
        &self,
        receipt: impl FnOnce() -> Box<dyn DeviceInsertAudioReceipt>,
    ) {
        if let Some(control) = &self.0.proposal_control {
            control.bind_device_insert_receipt(receipt());
        }
    }

    pub fn same_attempt(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    fn state(&self) -> std::sync::MutexGuard<'_, VerifiedState> {
        self.0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn preparing(&self) -> bool {
        matches!(*self.state(), VerifiedState::Preparing)
    }

    pub fn submitted(&self, receipt: Box<dyn DeviceInsertAudioReceipt>) {
        let receipt = Receipt(receipt);
        let mut state = self.state();
        if matches!(*state, VerifiedState::Preparing) {
            let submission = receipt.0.submission();
            *state = if submission.context == self.0.source.context()
                && submission.output_identity != 0
                && submission.audio_revision != 0
                && self.0.confirmation.is_none_or(|prior| {
                    submission.output_identity == prior.output_identity
                        && submission.audio_revision > prior.audio_revision
                }) {
                VerifiedState::Submitted(receipt)
            } else {
                VerifiedState::Failed("verified graph edit receipt differs".into())
            };
        }
    }

    pub fn refused(&self, error: String) {
        let mut state = self.state();
        if matches!(*state, VerifiedState::Preparing) {
            *state = VerifiedState::Failed(error);
        }
    }

    pub fn submission(&self) -> Option<ResidentGraphSubmission> {
        match &*self.state() {
            VerifiedState::Submitted(receipt) => Some(receipt.0.submission()),
            _ => None,
        }
    }

    fn poll(&self, cancel: bool) -> Option<Progress> {
        let mut state = self.state();
        match &*state {
            VerifiedState::Preparing if cancel => {
                *state = VerifiedState::Cancelled;
                Some(Progress::Terminal(DeviceInsertAudioState::Cancelled))
            }
            VerifiedState::Preparing if Instant::now() >= self.0.prepare_until => {
                *state = VerifiedState::Failed("verified graph edit preparation timed out".into());
                Some(Progress::PreparationFailed)
            }
            VerifiedState::Preparing => None,
            VerifiedState::Submitted(receipt) => receipt
                .poll(cancel || self.needs_tempo_cut() && Instant::now() >= self.0.prepare_until),
            VerifiedState::Failed(_) => Some(Progress::PreparationFailed),
            VerifiedState::Cancelled => Some(Progress::Terminal(DeviceInsertAudioState::Cancelled)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::service) enum Progress {
    Proceed,
    PreparationFailed,
    Terminal(DeviceInsertAudioState),
}

struct Receipt(Box<dyn DeviceInsertAudioReceipt>);

impl Receipt {
    fn poll(&self, cancel: bool) -> Option<Progress> {
        if cancel {
            self.0.cancel();
        }
        match self.0.state() {
            DeviceInsertAudioState::Pending | DeviceInsertAudioState::Applying => None,
            terminal => Some(Progress::Terminal(terminal)),
        }
    }
}

impl Drop for Receipt {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl Pending {
    fn insertion_control(&self) -> Option<&PluginEffectControl> {
        self.proposal_control.as_deref().or_else(|| {
            self.plugin
                .as_ref()
                .map(|plugin| plugin.insertion.control.as_ref())
        })
    }

    pub(in crate::service) fn submission(&self) -> Option<ResidentGraphSubmission> {
        match &self.prepared {
            Prepared::Resident(resident) => Some(resident.receipt.0.submission()),
            Prepared::Verified { attempt, .. } => attempt.submission(),
        }
    }

    pub(in crate::service) fn verified_attempt(&self) -> Option<&VerifiedGraphEditAttempt> {
        match &self.prepared {
            Prepared::Verified { attempt, .. } => Some(attempt),
            Prepared::Resident(_) => None,
        }
    }

    pub(in crate::service) fn preparation_error(&self) -> Option<String> {
        let attempt = self.verified_attempt()?;
        match &*attempt.state() {
            VerifiedState::Failed(error) => Some(error.clone()),
            _ => None,
        }
    }

    pub(in crate::service) fn awaiting_confirmation(&self) -> bool {
        self.confirmation.is_some()
    }

    pub(in crate::service) fn cancel_confirmation(&self) {
        if let Some(offer) = self.confirmation.as_ref() {
            offer.cancel();
        }
    }

    pub(in crate::service) fn poll(&self, cancel: bool) -> Option<Progress> {
        let cancel = cancel
            || self
                .insertion_control()
                .is_some_and(PluginEffectControl::is_cancelled);
        if let Some(offer) = self.confirmation.as_ref() {
            if cancel {
                offer.cancel();
                return Some(Progress::Terminal(DeviceInsertAudioState::Cancelled));
            }
            return offer.progress();
        }
        // If the callback already claimed admission, its terminal owns the edit.
        match &self.prepared {
            Prepared::Resident(resident) => resident.receipt.poll(cancel),
            Prepared::Verified { attempt, .. } => attempt.poll(cancel),
        }
    }

    pub(in crate::service) fn offer(
        &mut self,
        sink: &mut dyn WorkerAudioSink,
    ) -> Option<DeviceInsertConfirmation> {
        let operation = self.operation.device()?;
        if self.confirmation.is_some() {
            return None;
        }
        let submission = self.submission()?;
        let deadline = sink.offer_device_insert_consent(submission)?;
        if self
            .insertion_control()
            .is_some_and(|control| !control.release_refused_insert_receipt(submission))
        {
            return None;
        }
        let offer = DeviceInsertConfirmation::new(submission, deadline, operation);
        self.confirmation = Some(offer.clone());
        Some(offer)
    }

    pub(in crate::service) fn proceed(
        &mut self,
        session: &mut Session,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<(), SessionError> {
        let preparation_deadline = device_edit_preparation_deadline();
        let refused = self
            .submission()
            .ok_or(SessionError::Invalid("device insert was not submitted"))?;
        let resident = match &mut self.prepared {
            Prepared::Resident(resident) => resident,
            Prepared::Verified { attempt, .. } => {
                let retry = VerifiedGraphEditAttempt::new(
                    Arc::clone(attempt.source()),
                    attempt.predecessor(),
                    attempt.retiring_gain(),
                    Some(refused),
                    false,
                    attempt.stages_hosted_history(),
                    attempt
                        .proposal_control()
                        .cloned()
                        .or_else(|| self.proposal_control.clone()),
                );
                sink.begin_verified_graph_edit(retry.clone())
                    .map_err(SessionError::NativeAudio)?;
                *attempt = retry;
                self.confirmation = None;
                return Ok(());
            }
        };
        let inputs = Inputs {
            device_insert_control: self.proposal_control.as_deref(),
            ..session.resident_seek_inputs(admission, current, None)?
        };
        let metadata = canonical_metadata(&resident.snapshot, refused.context);
        inputs.publish_tagged_with_policy(
            &mut resident.snapshot,
            self.edit.project(),
            sink,
            metadata,
            ResidentPublicationPolicy::DeviceInsert {
                predecessor: resident.predecessor,
                hosted: self.plugin.is_some(),
                confirmation: Some(refused),
                preparation_deadline,
            },
        )?;
        resident.receipt = Receipt(insert_receipt(&resident.snapshot, sink)?);
        self.confirmation = None;
        Ok(())
    }

    pub(in crate::service) fn commit(
        self,
        session: &mut Session,
        previous: Option<&Snapshot>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>, Option<Snapshot>), SessionError> {
        let created = match self.edit {
            StagedChange::Command(edit) => session.editor.commit_staged_edit(edit)?,
            StagedChange::History(edge) => {
                session.editor.commit_staged_history(edge)?;
                Created::None
            }
        };
        match self.prepared {
            Prepared::Resident(resident) => {
                if let Some(previous) = previous {
                    resident
                        .snapshot
                        .inputs
                        .cancel_retired_from(&previous.inputs);
                }
                Ok((
                    created,
                    Arc::clone(&resident.snapshot.prepared),
                    Some(resident.snapshot),
                ))
            }
            Prepared::Verified {
                editor_revision, ..
            } => Ok((created, editor_revision, None)),
        }
    }
}

fn insert_receipt(
    snapshot: &Snapshot,
    sink: &mut dyn WorkerAudioSink,
) -> Result<Box<dyn DeviceInsertAudioReceipt>, SessionError> {
    snapshot
        .submission
        .and_then(|submission| sink.device_insert_receipt(submission))
        .filter(|receipt| Some(receipt.submission()) == snapshot.submission)
        .ok_or(SessionError::Invalid(
            "device insertion receipt unavailable",
        ))
}

fn hosted_history_operation(before: &Project, after: &Project) -> Option<GraphEditOperation> {
    fn adds_hosted_effect(before: &Project, after: &Project) -> bool {
        let vst3 = hex_lattice_contract::DeviceTypeId::from_static("hex.vst3");
        after
            .tracks()
            .iter()
            .flat_map(|track| track.devices())
            .chain(
                after
                    .return_tracks()
                    .iter()
                    .flat_map(|track| track.devices()),
            )
            .chain(after.main_devices())
            .any(|effect| effect.state().type_id() == vst3 && before.device(effect.id()).is_none())
    }
    if adds_hosted_effect(before, after) {
        Some(GraphEditOperation::InsertDevice)
    } else if adds_hosted_effect(after, before) {
        Some(GraphEditOperation::RemoveDevice)
    } else {
        None
    }
}

pub(in crate::service) fn is_tempo_edit(command: &Command) -> bool {
    matches!(
        command,
        Command::SetProjectTempo { .. } | Command::SetTempoPoint { .. }
    )
}

pub(in crate::service) fn graph_edit_operation(command: &Command) -> Option<GraphEditOperation> {
    catalog_graph_edit_operation(command, false)
}

/// An installed Resident graph can prepare fresh clipboard processors through
/// its native effect binding path. With no output, retain ordinary clipboard
/// preparation and publish the canonical source when an output opens.
pub(in crate::service) fn should_stage_resident_graph_edit(
    command: &Command,
    sink: &mut dyn WorkerAudioSink,
) -> bool {
    sink.confirms_device_inserts()
        && (graph_edit_operation(command) == Some(GraphEditOperation::InsertDevice)
            || (resident_graph_edit_operation(command, false).is_some()
                && !matches!(
                    sink.resident_publication_readiness(),
                    ResidentPublicationReadiness::NoOutput
                )))
}

fn resident_graph_edit_operation(
    command: &Command,
    controlled: bool,
) -> Option<GraphEditOperation> {
    match command {
        Command::PasteDevices { .. } => Some(GraphEditOperation::InsertDevice),
        _ => catalog_graph_edit_operation(command, controlled),
    }
}

fn catalog_graph_edit_operation(command: &Command, controlled: bool) -> Option<GraphEditOperation> {
    match command {
        Command::InitializeDevice(initialization) => {
            catalog_graph_edit_operation(initialization.command(), controlled)
        }
        // The picker keeps the instrument's identity but uses the same exact
        // install fence as an insertion. Ordinary state edits retain their
        // existing publication and parameter semantics.
        Command::ReplaceDeviceState { .. } if controlled => Some(GraphEditOperation::InsertDevice),
        Command::InsertDevice { .. }
        | Command::InsertInstrument { .. }
        | Command::LoadInstrument { .. }
        | Command::InsertMainDevice { .. }
        | Command::InsertReturnDevice { .. }
        | Command::InsertTrackWithDevice { .. }
        | Command::InsertDefaultTrackWithDevice { .. }
        | Command::InsertDefaultTrackWithDeviceAndGain { .. }
        | Command::InsertNestedDevice { .. } => Some(GraphEditOperation::InsertDevice),
        Command::RemoveDevice(_)
        | Command::RemoveDevices { .. }
        | Command::RemoveInstrument { .. } => Some(GraphEditOperation::RemoveDevice),
        Command::MoveDevice { .. }
        | Command::MoveMainDevice { .. }
        | Command::MoveReturnDevice { .. }
        | Command::MoveTrackDevice { .. }
        | Command::MoveTrackDevices { .. }
        | Command::ReorderDevices { .. } => Some(GraphEditOperation::MoveDevice),
        Command::RemoveTrack(_) | Command::RemoveTracks(_) => Some(GraphEditOperation::RemoveTrack),
        _ => None,
    }
}

pub(in crate::service) enum BrowserEdit {
    Pending(Box<Pending>),
    Applied(Created, Arc<PreparedRevision>),
    Unchanged,
}

/// Device drops that insert or move a device use the same install receipt as
/// explicit graph commands. Other browser actions and offline callers use
/// ordinary command preparation.
#[allow(clippy::too_many_arguments)] // Exact request, editor and audio owners.
pub(in crate::service) fn submit_browser_edit(
    session: &mut Session,
    command: Command,
    state: &ViewState,
    project_generation: u64,
    previous: Option<&PreparedRevision>,
    events: &mut WorkerEvents,
    resident: &mut Option<Snapshot>,
    admission: sampler_export::Admission<'_>,
    document_intent: u64,
    proposal_control: Option<&Arc<PluginEffectControl>>,
) -> Result<BrowserEdit, SessionError> {
    let context = hex_engine::AudioSourceContext {
        project_generation,
        editor_revision: state.revision,
    };
    if let Some(operation @ (GraphEditOperation::InsertDevice | GraphEditOperation::MoveDevice)) =
        catalog_graph_edit_operation(&command, proposal_control.is_some())
        && let Some(sink) = events.resident_audio()
    {
        match session.audio_mode() {
            DocumentAudioMode::Resident
                if sink.resident_publication_enabled()
                    && sink.confirms_device_inserts()
                    && (operation == GraphEditOperation::InsertDevice
                        || sink.resident_publication_readiness()
                            != ResidentPublicationReadiness::NoOutput) =>
            {
                return session
                    .stage_resident_device_insert(
                        command,
                        context,
                        sink,
                        admission,
                        resident.as_ref(),
                        None,
                        proposal_control.map(Arc::as_ref),
                        document_intent,
                    )
                    .map(BrowserEdit::from_pending);
            }
            DocumentAudioMode::VerifiedStreaming if sink.confirms_verified_device_inserts() => {
                return session
                    .stage_verified_graph_edit(
                        command,
                        state,
                        previous,
                        sink,
                        document_intent,
                        None,
                        proposal_control.cloned(),
                    )
                    .map(BrowserEdit::from_pending);
            }
            _ => {}
        }
    }
    // The synchronous fallback has no cancellable audio receipt. Its commit
    // and cancellation race at admission, before it changes the project.
    if proposal_control.is_some_and(|control| !control.begin_commit()) {
        return Err(SessionError::Invalid("device insertion was cancelled"));
    }
    let (created, prepared) = super::execute_command(
        session,
        command,
        previous,
        hex_engine::AudioSourceContext {
            editor_revision: context.editor_revision.saturating_add(1),
            ..context
        },
        events,
        resident,
        admission,
    )?;
    Ok(BrowserEdit::Applied(created, prepared))
}

impl BrowserEdit {
    fn from_pending(pending: Option<Pending>) -> Self {
        pending.map_or(Self::Unchanged, |pending| Self::Pending(Box::new(pending)))
    }
}

impl Session {
    fn verified_graph_predecessor(
        &self,
        state: &ViewState,
    ) -> Result<hex_engine::AudioSourceContext, SessionError> {
        let predecessor = state
            .canonical_audio_source
            .as_ref()
            .filter(|source| {
                source.audio_mode() == DocumentAudioMode::VerifiedStreaming
                    && source.matches_session_sources(self)
            })
            .ok_or(SessionError::Invalid("verified graph edit source changed"))?
            .context();
        if state.project.as_ref() != self.project()
            || state.audio_authoring_revision(predecessor) != Some(state.revision)
        {
            return Err(SessionError::Invalid(
                "verified graph edit authoring revision changed",
            ));
        }
        Ok(predecessor)
    }

    pub(in crate::service) fn has_verified_hosted_history(&self, redo: bool) -> bool {
        self.editor
            .inspect_history(redo, hosted_history_operation)
            .flatten()
            .is_some()
    }

    pub(in crate::service) fn stage_verified_hosted_history(
        &mut self,
        redo: bool,
        state: &ViewState,
        previous: Option<&PreparedRevision>,
        sink: &mut dyn WorkerAudioSink,
        document_intent: u64,
    ) -> Result<Pending, SessionError> {
        let predecessor = self.verified_graph_predecessor(state)?;
        let edge = self
            .editor
            .stage_history(redo)?
            .ok_or(SessionError::Invalid(
                "verified device history target changed",
            ))?;
        let operation = hosted_history_operation(self.project(), edge.project()).ok_or(
            SessionError::Invalid("verified device history target changed"),
        )?;
        self.start_verified_graph_edit(
            StagedChange::History(edge),
            Operation::HostedHistory(operation),
            predecessor,
            state.revision,
            previous,
            sink,
            document_intent,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // Exact source, worker, gesture and insertion intent.
    pub(in crate::service) fn stage_verified_graph_edit(
        &mut self,
        command: Command,
        state: &ViewState,
        previous: Option<&hex_engine::PreparedRevision>,
        sink: &mut dyn WorkerAudioSink,
        document_intent: u64,
        gesture: Option<&crate::device_gesture::GestureSnapshot>,
        proposal_control: Option<Arc<PluginEffectControl>>,
    ) -> Result<Option<Pending>, SessionError> {
        crate::validate_command_media(&command)?;
        let predecessor = self.verified_graph_predecessor(state)?;
        let operation = if is_tempo_edit(&command) {
            Operation::Tempo
        } else {
            Operation::Device(
                catalog_graph_edit_operation(&command, proposal_control.is_some())
                    .ok_or(SessionError::Invalid("unsupported staged graph edit"))?,
            )
        };
        let edit = self.editor.stage_edit(command)?;
        let tempo_cut = matches!(operation, Operation::Tempo);
        if (tempo_cut
            || operation.device() == Some(GraphEditOperation::MoveDevice)
            || proposal_control.is_some())
            && edit.project() == self.project()
        {
            return Ok(None);
        }
        let retiring_gain = gesture
            .filter(|active| {
                operation.device() == Some(GraphEditOperation::RemoveTrack)
                    && active.target.track_gain_anchor().is_some_and(|track| {
                        edit.project().track(track).is_none()
                            || active.track_gains.as_ref().is_some_and(|group| {
                                group
                                    .entries()
                                    .any(|(member, _)| edit.project().track(member).is_none())
                            })
                    })
            })
            .map(|active| DevicePreviewIdentity {
                project_generation: predecessor.project_generation,
                key: active.key,
                sequence: active.sequence,
                canonical_revision: state.revision,
            });
        self.start_verified_graph_edit(
            StagedChange::Command(edit),
            operation,
            predecessor,
            state.revision,
            previous,
            sink,
            document_intent,
            retiring_gain,
            proposal_control,
        )
        .map(Some)
    }

    #[allow(clippy::too_many_arguments)] // Exact authored target and its existing publication owners.
    fn start_verified_graph_edit(
        &self,
        edit: StagedChange,
        operation: Operation,
        predecessor: hex_engine::AudioSourceContext,
        editor_revision: u64,
        previous: Option<&PreparedRevision>,
        sink: &mut dyn WorkerAudioSink,
        document_intent: u64,
        retiring_gain: Option<DevicePreviewIdentity>,
        proposal_control: Option<Arc<PluginEffectControl>>,
    ) -> Result<Pending, SessionError> {
        let next = editor_revision
            .checked_add(1)
            .ok_or(SessionError::Invalid("editor revision exhausted"))?;
        let source = Arc::new(CanonicalAudioSourceSnapshot::capture(
            &Arc::new(edit.project().clone()),
            predecessor.project_generation,
            next,
            self,
        ));
        let tempo_cut = matches!(operation, Operation::Tempo);
        let hosted_history = matches!(operation, Operation::HostedHistory(_));
        let begin_before_projection = tempo_cut || hosted_history;
        let attempt = VerifiedGraphEditAttempt::new(
            source,
            predecessor,
            retiring_gain,
            None,
            tempo_cut,
            hosted_history,
            proposal_control,
        );
        if begin_before_projection {
            // Keep the exact predecessor before timing projection or private hosted preparation.
            sink.begin_verified_graph_edit(attempt.clone())
                .map_err(SessionError::NativeAudio)?;
        }
        let prepared = self.prepare_verified_edit_revision(
            edit.project(),
            previous,
            attempt.source().context(),
            attempt.proposal_control(),
            sink,
        );
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                if begin_before_projection {
                    sink.finish_verified_graph_edit(&attempt, false);
                }
                return Err(error);
            }
        };
        if !begin_before_projection {
            sink.begin_verified_graph_edit(attempt.clone())
                .map_err(SessionError::NativeAudio)?;
        }
        Ok(Pending {
            edit,
            prepared: Prepared::Verified {
                attempt,
                editor_revision: Arc::new(prepared),
            },
            confirmation: None,
            operation,
            cut_devices: None,
            cut_tracks: None,
            document_intent,
            plugin: None,
            proposal_control: None,
            publish_created: !hosted_history,
        })
    }

    pub(super) fn prepare_verified_edit_revision(
        &self,
        project: &Project,
        previous: Option<&PreparedRevision>,
        context: hex_engine::AudioSourceContext,
        proposal: Option<&Arc<PluginEffectControl>>,
        sink: &mut dyn WorkerAudioSink,
    ) -> Result<PreparedRevision, SessionError> {
        let media = self.media_bank();
        let previous_session = previous.map(|artifact| artifact.session().as_ref());
        let input = ResidentEffectPreparation {
            project,
            context,
            restoring_plugin_state: false,
            preparing_edit: true,
        };
        let prepared = if input.requires_plugin_profile() {
            sink.prepare_verified_effects(input, proposal, &mut |effects| {
                crate::prepare_with_waveforms_using(
                    project,
                    &media,
                    &self.audio_transforms,
                    &self.audio_clip_waveforms,
                    || {
                        if media.verified.is_empty() {
                            effects.prepare_semantic_revision(
                                project,
                                &media.resident,
                                self.sine_table(),
                                previous_session,
                                &self.audio_transforms,
                            )
                        } else {
                            media.prepare_with_effects(
                                project,
                                self.sine_table(),
                                previous_session,
                                Some(effects),
                            )
                        }
                    },
                )
            })
            .map_err(SessionError::NativeAudio)?
        } else {
            crate::prepare_with_waveforms(
                project,
                &media,
                self.sine_table(),
                previous_session,
                &self.audio_transforms,
                &self.audio_clip_waveforms,
            )?
        };
        Ok(crate::tuning_prepared_transition(
            prepared,
            self.project(),
            project,
            previous,
        )?)
    }

    /// Stage the exact replacement graph. An unchanged move leaves the
    /// existing audio submission and project history alone.
    #[allow(clippy::too_many_arguments)] // Exact editor, audio and worker request inputs.
    pub(in crate::service) fn stage_resident_device_insert(
        &mut self,
        command: Command,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
        plugin: Option<&PluginEffectControl>,
        proposal_control: Option<&PluginEffectControl>,
        document_intent: u64,
    ) -> Result<Option<Pending>, SessionError> {
        let preparation_deadline = device_edit_preparation_deadline();
        let staged = (|| {
            crate::validate_command_media(&command)?;
            let operation = if plugin.is_some() {
                // The hosted browser also stages replacement of an instrument.
                GraphEditOperation::InsertDevice
            } else {
                resident_graph_edit_operation(&command, proposal_control.is_some())
                    .ok_or(SessionError::Invalid("unsupported staged graph edit"))?
            };
            let edit = self.editor.stage_edit(command)?;
            if (operation == GraphEditOperation::MoveDevice || proposal_control.is_some())
                && edit.project() == self.project()
            {
                return Ok(None);
            }
            let installed = sink
                .resident_runtime_loop(context)
                .map_err(SessionError::NativeAudio)?
                .filter(|actual| {
                    // A delayed open may install the canonical offline graph
                    // on a fresh output without updating the Session submission.
                    // The sink supplies and revalidates the actual output/revision.
                    current.is_some_and(|snapshot| {
                        actual.audio_revision != 0
                            && actual.source_context == Some(snapshot.prepared_context)
                            && matches!(
                                actual.source_origin,
                                ResidentSourceOrigin::Unknown
                                    | ResidentSourceOrigin::CanonicalCandidate
                            )
                    })
                })
                .ok_or(SessionError::Invalid(
                    "device edit needs the current installed audio graph",
                ))?;
            let context = hex_engine::AudioSourceContext {
                editor_revision: context.editor_revision.saturating_add(1),
                ..context
            };
            let inputs = Inputs {
                preparing_edit: plugin.is_none(),
                device_insert_control: proposal_control,
                ..self.resident_seek_inputs(admission, current, None)?
            };
            let mut snapshot = inputs.prepare_transition(
                self.project(),
                edit.project(),
                false,
                sink,
                &self.media_bank(),
                self.sine_table(),
                &self.audio_transforms,
                &self.audio_clip_waveforms,
                context,
            )?;
            snapshot.source_origin = ResidentSourceOrigin::CanonicalCandidate;
            if plugin
                .or(proposal_control)
                .is_some_and(PluginEffectControl::is_cancelled)
            {
                return Err(SessionError::Invalid("device insertion was cancelled"));
            }
            let metadata = canonical_metadata(&snapshot, context);
            inputs.publish_tagged_with_policy(
                &mut snapshot,
                edit.project(),
                sink,
                metadata,
                ResidentPublicationPolicy::DeviceInsert {
                    predecessor: installed,
                    hosted: plugin.is_some(),
                    confirmation: None,
                    preparation_deadline,
                },
            )?;
            let receipt = insert_receipt(&snapshot, sink)?;
            Ok(Some(Pending {
                edit: StagedChange::Command(edit),
                prepared: Prepared::Resident(Box::new(Resident {
                    snapshot,
                    receipt: Receipt(receipt),
                    predecessor: installed,
                })),
                confirmation: None,
                operation: Operation::Device(operation),
                cut_devices: None,
                cut_tracks: None,
                document_intent,
                plugin: None,
                proposal_control: None,
                publish_created: true,
            }))
        })();
        if staged.is_err() {
            sink.finish_resident_effect_edit(
                hex_engine::AudioSourceContext {
                    editor_revision: context.editor_revision.saturating_add(1),
                    ..context
                },
                false,
            );
        }
        staged
    }
}

#[cfg(test)]
mod tests;
