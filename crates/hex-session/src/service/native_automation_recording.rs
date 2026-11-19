//! Single control-owned device touch take. The audio client supplies delivery
//! receipts and retains a batch until Session acknowledges its admission.
//! The disk worker seals the file independently of callback and UI lifetimes.
use super::Session;
mod native_recovery;
pub use native_recovery::{
    MAX_NATIVE_RECOVERY_CONTINUATION_EVENTS, NativeRecoveryContinuation, NativeRecoveryTerminal,
    copy_native_recovery_on_control,
};
mod plugin_observation;
pub use plugin_observation::{
    FailedPluginContinuation, MAX_FAILED_PLUGIN_EDITOR_OBSERVATIONS, copy_failed_plugin_on_control,
    copy_failed_plugin_with_receipt_on_control, prepare_failed_plugin_continuation,
};
mod recovery;
use super::automation_draft::{
    DeviceDraftInput, DraftProvenance, DraftWorker, DraftWorkerOutcome, GroupDraftEvent,
    RecoveryCopyReceipt, RecoveryCopyWorker, Vst3CaptureTarget, Vst3GroupCaptureTarget,
    Vst3ParameterApplication,
};
use super::automation_take::{
    CompletedAutomationTake, prepare_device_take_adoption_on_control,
    prepare_group_take_adoption_on_control,
};
use hex_engine::{AudioSourceContext, NativeParameterApplication, NativeParameterInputTarget};
use hex_storage::automation_draft::{
    CaptureClock, CompletedGroupedDraft, DraftAbortReason, DraftClock, DraftReadLimits,
    DraftTerminal,
};
pub use recovery::{
    AutomationRecoveryFileIdentity, SealedAutomationRecoveryFile,
    prepare_verified_vst3_recovery_on_control,
};
pub(super) use recovery::{inspect_sealed_file, recover_sealed};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

/// Closed capture kinds share one exclusive draft/worker and adoption owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceCaptureTarget {
    Native(NativeParameterInputTarget),
    Vst3(Vst3CaptureTarget),
    Vst3Group(Vst3GroupCaptureTarget),
}
impl DeviceCaptureTarget {
    /// Durable target identity; this does not grant live worker admission.
    pub fn draft_target(self) -> super::automation_draft::DraftTarget {
        use super::automation_draft::DraftTarget;
        match self {
            Self::Native(target) => DraftTarget::Device {
                device: target.device(),
                target: target.target(),
            },
            Self::Vst3(target) => DraftTarget::Device {
                device: target.device,
                target: hex_project::DeviceAutomationTarget::Vst3 {
                    parameter: target.parameter,
                },
            },
            Self::Vst3Group(target) => DraftTarget::Vst3Group(target.draft),
        }
    }

    pub fn native(self) -> Option<NativeParameterInputTarget> {
        match self {
            Self::Native(target) => Some(target),
            Self::Vst3(_) | Self::Vst3Group(_) => None,
        }
    }
}
impl From<NativeParameterInputTarget> for DeviceCaptureTarget {
    fn from(target: NativeParameterInputTarget) -> Self {
        Self::Native(target)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceAutomationTakeScope {
    pub target: DeviceCaptureTarget,
    pub stamp: DraftProvenance,
}

impl DeviceAutomationTakeScope {
    pub(super) fn context(self) -> AudioSourceContext {
        AudioSourceContext {
            project_generation: self.stamp.project_generation,
            editor_revision: self.stamp.editor_revision,
        }
    }
}

/// Publication needs the native preparation identity or foreign confirmation,
/// not the historical live worker binding embedded in a capture receipt.
#[derive(Clone, Copy)]
pub(super) enum TakePublication {
    Native(NativeParameterInputTarget),
    Vst3,
}
impl TakePublication {
    pub(super) fn native(self) -> Option<NativeParameterInputTarget> {
        match self {
            Self::Native(target) => Some(target),
            Self::Vst3 => None,
        }
    }
}
impl From<DeviceCaptureTarget> for TakePublication {
    fn from(target: DeviceCaptureTarget) -> Self {
        match target {
            DeviceCaptureTarget::Native(target) => Self::Native(target),
            DeviceCaptureTarget::Vst3(_) | DeviceCaptureTarget::Vst3Group(_) => Self::Vst3,
        }
    }
}

/// Copied observation of the applied and playing resident output. The
/// callback still checks the actual source and epoch when admitting Begin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceAutomationSource {
    pub target: DeviceCaptureTarget,
    pub sample_rate: u32,
    pub output_identity: u64,
    pub audio_revision: u64,
    pub transport_epoch: u64,
    pub section_loop: Option<hex_project::LoopRegion>,
}

impl DeviceAutomationSource {
    fn matches(self, scope: DeviceAutomationTakeScope) -> bool {
        self.target == scope.target
            && self.sample_rate == scope.stamp.sample_rate
            && self.output_identity == scope.stamp.output_identity
            && self.audio_revision == scope.stamp.audio_revision
            && self.transport_epoch == scope.stamp.transport_epoch
    }

    fn capture_clock(self, project: &hex_project::Project) -> CaptureClock {
        CaptureClock {
            sample_rate: self.sample_rate,
            loop_region: self
                .section_loop
                .or_else(|| project.loop_region().filter(|region| region.enabled())),
        }
    }
}

#[derive(Debug)]
pub enum DeviceAutomationTakeAction {
    Begin,
    /// Per-take deterministic writer refusal, available only in explicit fixtures.
    #[cfg(feature = "recording-fault-test-support")]
    InjectWriterFailure,
    Append(Arc<[(u64, NativeParameterApplication)]>),
    Finish(NativeParameterApplication),
    AppendVst3(Arc<[(u64, Vst3ParameterApplication)]>),
    FinishVst3(Vst3ParameterApplication),
    /// Group rows carry actual Main clocks in delivery order, with one SDK
    /// gesture per member and an independent file ordinal.
    AppendVst3Group(Arc<[GroupDraftEvent]>),
    FinishVst3Group(DraftClock),
    /// Original SDK/final-output observation. Retain it until Appended or
    /// Sealing; Backpressured does not consume its sequence or original clock.
    PluginObservation(Box<super::PluginParameterObservation>),
    Abort(DraftAbortReason),
    Interrupt,
    /// Poll without waiting for disk. A sealed matching take commits during
    /// this request through the ordinary Session edit transaction.
    Poll,
    /// Start a non-adoptable checkpoint of the retained failed writer custody.
    CopyFailed {
        destination: PathBuf,
        max_events: u64,
    },
    /// Copy failed VST3 custody together with the quiesced SDK observations.
    /// Session obtains observations from their owner rather than the caller.
    CopyFailedPlugin {
        destination: PathBuf,
        max_events: u64,
    },
    /// Poll the one failed-writer copy without waiting for disk.
    PollFailedCopy,
    /// Retrieve the last successful VST3 copy's identity without releasing custody.
    InspectFailedPluginCopy,
    /// Adopt a verified failed VST3 copy on its unchanged live source.
    AdoptFailedPluginCopy {
        max_events: u64,
    },
    /// Explicitly keep the verified copy instead of adopting its trajectory.
    /// Release original memory custody only after revalidation; keep both files.
    KeepFailedPluginCopy {
        path: PathBuf,
        receipt: RecoveryCopyReceipt,
        max_events: u64,
    },
    /// Quiesce an exact native writer after its caller has retired the input.
    /// Repeat while Pending; this never adopts the outcome or drops custody.
    PrepareNativeRecovery,
    /// Copy the retained writer and the caller's bounded, quiescent continuation.
    CopyNativeRecovery {
        destination: PathBuf,
        max_events: u64,
        continuation: NativeRecoveryContinuation,
    },
    /// Resolve only the exact verified copy selected by the caller. Both files
    /// remain durable; no canonical edit is made.
    KeepNativeRecoveryCopy {
        path: PathBuf,
        receipt: RecoveryCopyReceipt,
        max_events: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceAutomationTakeProgress {
    Started,
    Appended,
    Backpressured,
    Sealing,
    Pending,
    /// The original editor scope ended without a confirmed point or edit.
    Unchanged,
    Committed,
    /// The edit committed but a custody recheck refused release.
    CommittedWithRetainedCustody(PathBuf),
    /// No failed VST3 writer or SDK custody is currently retained.
    FailedPluginUnavailable,
    /// Original custody has not yet become quiescent.
    FailedPluginPending,
    /// Exact retained identity; querying it never changes custody.
    FailedPluginCustody {
        scope: Box<DeviceAutomationTakeScope>,
        path: PathBuf,
    },
    /// Fully validated identity of a selected finished durable file.
    SealedFile(Box<SealedAutomationRecoveryFile>),
    /// An ordered observation found no device automation custody.
    CloseSafe,
    /// Device automation custody is active, retained or being copied.
    CloseBlocked,
    Retained(PathBuf),
    /// Poll found a failed native writer that still owns original observations.
    /// Retain the caller's exact take scope for explicit recovery.
    NativeWriterRetained {
        path: PathBuf,
        reason: String,
    },
    VerifiedCopy {
        path: PathBuf,
        receipt: RecoveryCopyReceipt,
    },
    /// Original memory custody was resolved by keeping the verified durable copy.
    CopyKept(PathBuf),
    Refused,
}

pub struct DeviceAutomationTakeCompletion {
    pub request_id: u64,
    pub progress: DeviceAutomationTakeProgress,
}

#[derive(Default)]
pub(super) struct Lane {
    active: Option<Active>,
    // Failed writers own unsynced batches; explicitly interrupted native
    // writers keep their prefix while the caller retains its continuation.
    failed: Option<Failed>,
    failed_copy: RecoveryCopyWorker,
    copy_destination: Option<PendingCopy>,
    verified_plugin_copy: Option<VerifiedPluginCopy>,
    verified_native_copy: Option<(PathBuf, RecoveryCopyReceipt)>,
}

struct Failed {
    scope: DeviceAutomationTakeScope,
    source: DeviceAutomationSource,
    outcome: Arc<DraftWorkerOutcome>,
}

enum PendingCopy {
    Checkpoint(PathBuf),
    Native(PathBuf),
    Plugin {
        path: PathBuf,
        observations: Vec<super::PluginParameterObservation>,
    },
}

struct VerifiedPluginCopy {
    path: PathBuf,
    receipt: RecoveryCopyReceipt,
    observations: Vec<super::PluginParameterObservation>,
}

fn same_plugin_observation(
    left: &super::PluginParameterObservation,
    right: &super::PluginParameterObservation,
) -> bool {
    use super::PluginParameterObservation::{Delivered, Restored};
    if left != right {
        return false;
    }
    match (left, right) {
        (Delivered(a), Delivered(b)) | (Restored(a), Restored(b)) => {
            match (a.application.value, b.application.value) {
                (
                    super::automation_draft::Vst3ApplicationValue::Value(a),
                    super::automation_draft::Vst3ApplicationValue::Value(b),
                ) => a.to_bits() == b.to_bits(),
                _ => true,
            }
        }
        _ => true,
    }
}

struct Active {
    scope: DeviceAutomationTakeScope,
    source: DeviceAutomationSource,
    input: DeviceDraftInput,
    worker: DraftWorker,
    preparing_native_recovery: bool,
}

pub(super) enum Step {
    Progress(DeviceAutomationTakeProgress),
    Commit(Box<hex_project::Command>, PathBuf),
}

impl Lane {
    pub(super) fn close_safe(&self) -> bool {
        self.active.is_none() && self.failed.is_none() && self.copy_destination.is_none()
    }
    pub(super) fn failed_plugin_identity(&self) -> Option<(DeviceAutomationTakeScope, &Path)> {
        let failed = self.failed.as_ref()?;
        matches!(
            failed.scope.target,
            DeviceCaptureTarget::Vst3(_) | DeviceCaptureTarget::Vst3Group(_)
        )
        .then_some((failed.scope, failed.outcome.path.as_path()))
    }

    pub(super) fn failed_source(
        &self,
        scope: DeviceAutomationTakeScope,
    ) -> Result<DeviceAutomationSource, String> {
        let failed = self
            .failed
            .as_ref()
            .ok_or("no failed device automation custody is available")?;
        if failed.scope != scope || !failed.source.matches(scope) {
            return Err("device automation recovery belongs to another take".into());
        }
        Ok(failed.source)
    }

    pub(super) fn failed_outcome(
        &self,
        scope: DeviceAutomationTakeScope,
    ) -> Result<&Arc<DraftWorkerOutcome>, String> {
        self.failed_source(scope)?;
        self.failed
            .as_ref()
            .map(|failed| &failed.outcome)
            .ok_or_else(|| "no failed device automation custody is available".into())
    }

    pub(super) fn copy_failed_plugin(
        &mut self,
        scope: DeviceAutomationTakeScope,
        destination: &Path,
        max_events: u64,
        observations: Vec<super::PluginParameterObservation>,
    ) -> Result<Step, String> {
        let source = self.failed_source(scope)?;
        if !matches!(
            scope.target,
            DeviceCaptureTarget::Vst3(_) | DeviceCaptureTarget::Vst3Group(_)
        ) || max_events > hex_project::MAX_DEVICE_AUTOMATION_POINTS as u64
            || observations.len() > MAX_FAILED_PLUGIN_EDITOR_OBSERVATIONS
        {
            return Err("invalid VST3 recovery target or budget".into());
        }
        let failed = self
            .failed
            .as_ref()
            .ok_or("no failed device automation custody is available")?;
        let destination = super::recovery::absolute_document_path(destination)?;
        let copy_path = destination.clone();
        let copied_observations = observations.clone();
        self.failed_copy
            .start_with_receipt(&failed.outcome, move |outcome| {
                copy_failed_plugin_with_receipt_on_control(
                    outcome,
                    &copy_path,
                    DraftReadLimits {
                        max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
                    },
                    max_events,
                    scope,
                    source,
                    &copied_observations,
                )
            })?;
        self.copy_destination = Some(PendingCopy::Plugin {
            path: destination,
            observations,
        });
        Ok(Step::Progress(DeviceAutomationTakeProgress::Pending))
    }

    pub(super) fn inspect_failed_plugin_copy(
        &self,
        scope: DeviceAutomationTakeScope,
        observations: &[super::PluginParameterObservation],
    ) -> Result<Step, String> {
        self.failed_source(scope)?;
        if self.copy_destination.is_some() {
            return Err("automation recovery copy is still pending".into());
        }
        let copy = self
            .verified_plugin_copy
            .as_ref()
            .ok_or("no verified VST3 recovery copy is available")?;
        if copy.observations.len() != observations.len()
            || !copy
                .observations
                .iter()
                .zip(observations)
                .all(|(a, b)| same_plugin_observation(a, b))
        {
            return Err("retained VST3 observations changed after recovery copy".into());
        }
        Ok(Step::Progress(DeviceAutomationTakeProgress::VerifiedCopy {
            path: copy.path.clone(),
            receipt: copy.receipt,
        }))
    }

    pub(super) fn release_failed_plugin_copy(
        &mut self,
        scope: DeviceAutomationTakeScope,
        path: &Path,
        receipt: RecoveryCopyReceipt,
        observations: &[super::PluginParameterObservation],
    ) -> Result<(), String> {
        let Step::Progress(DeviceAutomationTakeProgress::VerifiedCopy {
            path: found,
            receipt: identity,
        }) = self.inspect_failed_plugin_copy(scope, observations)?
        else {
            return Err("verified VST3 recovery copy is unavailable".into());
        };
        if found != path || identity != receipt {
            return Err("verified VST3 recovery copy changed".into());
        }
        self.verified_plugin_copy = None;
        self.failed = None;
        Ok(())
    }

    pub(super) fn path(&self) -> Option<&Path> {
        self.active.as_ref().map(|active| active.input.path())
    }

    pub(super) fn busy(&self) -> bool {
        self.active.is_some()
    }

    pub(super) fn has_failed_custody(&self) -> bool {
        self.failed.is_some()
    }

    pub(super) fn begin(
        &mut self,
        session: &Session,
        document_base: Option<&Path>,
        scope: DeviceAutomationTakeScope,
        source: DeviceAutomationSource,
        directory: &Path,
    ) -> Result<Step, String> {
        if let Some(failed) = self.failed.as_ref() {
            return Err(retained_error(
                &failed.outcome.path,
                "recover the previous device automation take first",
            ));
        }
        if self.active.is_some() {
            return Err("a device automation take already owns the input".into());
        }
        if !source.matches(scope) || source.sample_rate != session.project().sample_rate() {
            return Err("device automation source or transport changed".into());
        }
        let clock = source.capture_clock(session.project());
        let path = draft_path(directory)?;
        let (input, worker) = match scope.target {
            DeviceCaptureTarget::Native(target) => DeviceDraftInput::start_on_control(
                session,
                document_base,
                path,
                target,
                scope.stamp,
                clock,
            ),
            DeviceCaptureTarget::Vst3(target) => DeviceDraftInput::start_vst3_on_control(
                session,
                document_base,
                path,
                target,
                scope.stamp,
                clock,
            ),
            DeviceCaptureTarget::Vst3Group(target) => {
                DeviceDraftInput::start_vst3_group_on_control(
                    session,
                    document_base,
                    path,
                    target,
                    scope.stamp,
                    clock,
                )
            }
        }?;
        self.active = Some(Active {
            scope,
            source,
            input,
            worker,
            preparing_native_recovery: false,
        });
        Ok(Step::Progress(DeviceAutomationTakeProgress::Started))
    }

    pub(super) fn receive(
        &mut self,
        session: &Session,
        document_base: Option<&Path>,
        current: AudioSourceContext,
        scope: DeviceAutomationTakeScope,
        action: &DeviceAutomationTakeAction,
    ) -> Result<Step, String> {
        use DeviceAutomationTakeProgress as Progress;
        match action {
            DeviceAutomationTakeAction::PrepareNativeRecovery => {
                return self.prepare_native_recovery(scope);
            }
            DeviceAutomationTakeAction::CopyNativeRecovery {
                destination,
                max_events,
                continuation,
            } => {
                return self.start_native_recovery_copy(
                    scope,
                    destination,
                    *max_events,
                    continuation,
                );
            }
            DeviceAutomationTakeAction::KeepNativeRecoveryCopy {
                path,
                receipt,
                max_events,
            } => {
                return self.keep_native_recovery_copy(scope, path, *receipt, *max_events);
            }
            _ => {}
        }
        if matches!(
            action,
            DeviceAutomationTakeAction::CopyFailed { .. }
                | DeviceAutomationTakeAction::PollFailedCopy
        ) {
            let failed = self
                .failed
                .as_ref()
                .ok_or("no failed device automation custody is available")?;
            if failed.scope != scope || !failed.source.matches(scope) {
                return Err("device automation recovery belongs to another take".into());
            }
            return match action {
                DeviceAutomationTakeAction::CopyFailed {
                    destination,
                    max_events,
                } => {
                    if *max_events > hex_project::MAX_DEVICE_AUTOMATION_POINTS as u64 {
                        return Err("device automation recovery exceeds the event budget".into());
                    }
                    let destination = super::recovery::absolute_document_path(destination)?;
                    self.failed_copy.start(
                        &failed.outcome,
                        destination.clone(),
                        DraftReadLimits {
                            max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
                        },
                        *max_events,
                    )?;
                    self.copy_destination = Some(PendingCopy::Checkpoint(destination));
                    Ok(Step::Progress(Progress::Pending))
                }
                DeviceAutomationTakeAction::PollFailedCopy => {
                    if self.copy_destination.is_none() {
                        return Err("no device automation recovery copy is pending".into());
                    }
                    let Some(completion) = self.failed_copy.poll() else {
                        return Ok(Step::Progress(Progress::Pending));
                    };
                    let pending = self
                        .copy_destination
                        .take()
                        .ok_or("automation recovery destination is unavailable")?;
                    if !Arc::ptr_eq(&completion.outcome, &failed.outcome) {
                        return Err("automation recovery custody changed during copy".into());
                    }
                    completion.result?;
                    match (pending, completion.receipt) {
                        (PendingCopy::Checkpoint(destination), None) => {
                            Ok(Step::Progress(Progress::Retained(destination)))
                        }
                        (PendingCopy::Native(path), Some(receipt)) => {
                            self.verified_native_copy = Some((path.clone(), receipt));
                            Ok(Step::Progress(Progress::VerifiedCopy { path, receipt }))
                        }
                        (
                            PendingCopy::Plugin {
                                path: destination,
                                observations,
                            },
                            Some(receipt),
                        ) => {
                            self.verified_plugin_copy = Some(VerifiedPluginCopy {
                                path: destination.clone(),
                                receipt,
                                observations,
                            });
                            Ok(Step::Progress(Progress::VerifiedCopy {
                                path: destination,
                                receipt,
                            }))
                        }
                        _ => Err("automation recovery copy receipt is inconsistent".into()),
                    }
                }
                _ => Err("invalid automation recovery action".into()),
            };
        }
        let active = self
            .active
            .as_mut()
            .ok_or("device automation take is no longer active")?;
        if active.scope != scope {
            return Err("device automation receipt belongs to another take".into());
        }
        if active.preparing_native_recovery {
            return Err("native recovery owns the original writer".into());
        }
        let translated;
        let action = if let DeviceAutomationTakeAction::PluginObservation(observation) = action {
            translated = active.plugin_action(observation)?;
            &translated
        } else {
            action
        };
        while active.input.poll().is_some() {}
        let progress = match action {
            #[cfg(feature = "recording-fault-test-support")]
            DeviceAutomationTakeAction::InjectWriterFailure => {
                active.input.inject_next_append_failure()?;
                Progress::Appended
            }
            DeviceAutomationTakeAction::CopyFailed { .. }
            | DeviceAutomationTakeAction::CopyFailedPlugin { .. }
            | DeviceAutomationTakeAction::PollFailedCopy
            | DeviceAutomationTakeAction::InspectFailedPluginCopy
            | DeviceAutomationTakeAction::AdoptFailedPluginCopy { .. }
            | DeviceAutomationTakeAction::KeepFailedPluginCopy { .. }
            | DeviceAutomationTakeAction::PrepareNativeRecovery
            | DeviceAutomationTakeAction::CopyNativeRecovery { .. }
            | DeviceAutomationTakeAction::KeepNativeRecoveryCopy { .. } => {
                return Err("invalid active automation recovery action".into());
            }
            DeviceAutomationTakeAction::Begin => {
                return Err("device automation take is already active".into());
            }
            DeviceAutomationTakeAction::PluginObservation(_) => {
                return Err("nested plugin observation is unavailable".into());
            }
            DeviceAutomationTakeAction::Append(batch) => {
                if active
                    .input
                    .append_delivered(
                        scope
                            .target
                            .native()
                            .ok_or("native applications require a native take")?,
                        scope.stamp,
                        batch,
                    )?
                    .is_some()
                {
                    Progress::Appended
                } else {
                    Progress::Backpressured
                }
            }
            DeviceAutomationTakeAction::Finish(application) => {
                active.input.finish_delivered(
                    scope
                        .target
                        .native()
                        .ok_or("native restoration requires a native take")?,
                    scope.stamp,
                    *application,
                )?;
                Progress::Sealing
            }
            DeviceAutomationTakeAction::AppendVst3(batch) => {
                let DeviceCaptureTarget::Vst3(target) = scope.target else {
                    return Err("vst3 applications require a vst3 take".into());
                };
                if active
                    .input
                    .append_vst3_delivered(target, scope.stamp, batch)?
                    .is_some()
                {
                    Progress::Appended
                } else {
                    Progress::Backpressured
                }
            }
            DeviceAutomationTakeAction::FinishVst3(application) => {
                let DeviceCaptureTarget::Vst3(target) = scope.target else {
                    return Err("vst3 restoration requires a vst3 take".into());
                };
                active
                    .input
                    .finish_vst3_delivered(target, scope.stamp, *application)?;
                Progress::Sealing
            }
            DeviceAutomationTakeAction::AppendVst3Group(batch) => {
                let DeviceCaptureTarget::Vst3Group(target) = scope.target else {
                    return Err("grouped vst3 applications require a grouped take".into());
                };
                if active
                    .input
                    .append_vst3_group_delivered(target, scope.stamp, batch)?
                    .is_some()
                {
                    Progress::Appended
                } else {
                    Progress::Backpressured
                }
            }
            DeviceAutomationTakeAction::FinishVst3Group(clock) => {
                let DeviceCaptureTarget::Vst3Group(target) = scope.target else {
                    return Err("grouped vst3 restoration requires a grouped take".into());
                };
                active.input.finish_vst3_group_delivered(
                    target,
                    scope.stamp,
                    DraftTerminal::Finished(*clock),
                )?;
                Progress::Sealing
            }
            DeviceAutomationTakeAction::Abort(reason) => {
                match scope.target {
                    DeviceCaptureTarget::Native(target) => {
                        active.input.abort_delivered(target, scope.stamp, *reason)?
                    }
                    DeviceCaptureTarget::Vst3(target) => {
                        active
                            .input
                            .abort_vst3_delivered(target, scope.stamp, *reason)?
                    }
                    DeviceCaptureTarget::Vst3Group(target) => {
                        active.input.finish_vst3_group_delivered(
                            target,
                            scope.stamp,
                            DraftTerminal::Aborted(*reason),
                        )?
                    }
                }
                Progress::Sealing
            }
            DeviceAutomationTakeAction::Interrupt => {
                active.worker.request_interruption_on_control()?;
                Progress::Sealing
            }
            DeviceAutomationTakeAction::Poll => {
                if active.input.progress().failed {
                    // A failed append has no natural terminal until its
                    // owner closes the input. Preserve its original batches.
                    let _ = active.worker.request_interruption_on_control();
                }
                let Some(outcome) = active.input.try_take_outcome() else {
                    return Ok(Step::Progress(Progress::Pending));
                };
                let source = active.source;
                self.active = None;
                return self.resolve(session, document_base, current, scope, source, outcome);
            }
        };
        Ok(Step::Progress(progress))
    }

    pub(super) fn interrupt_if_source_changed(&self, current: AudioSourceContext) {
        if let Some(active) = self
            .active
            .as_ref()
            .filter(|active| active.scope.context() != current)
        {
            let _ = active.worker.request_interruption_on_control();
        }
    }

    fn resolve(
        &mut self,
        session: &Session,
        document_base: Option<&Path>,
        current: AudioSourceContext,
        scope: DeviceAutomationTakeScope,
        source: DeviceAutomationSource,
        outcome: DraftWorkerOutcome,
    ) -> Result<Step, String> {
        if let Some(reason) = outcome.failure.as_deref() {
            let path = outcome.path.clone();
            let reason = reason.to_owned();
            self.failed = Some(Failed {
                scope,
                source,
                outcome: Arc::new(outcome),
            });
            if scope.target.native().is_some() {
                return Ok(Step::Progress(
                    DeviceAutomationTakeProgress::NativeWriterRetained { path, reason },
                ));
            }
            return Err(retained_error(&path, &reason));
        }
        if !matches!(outcome.requested_terminal, DraftTerminal::Finished(_))
            || outcome.admitted_events == 0
            || current != scope.context()
        {
            return Ok(Step::Progress(DeviceAutomationTakeProgress::Retained(
                outcome.path,
            )));
        }
        if let DeviceCaptureTarget::Vst3Group(target) = scope.target {
            let command = (|| {
                let mut take = CompletedGroupedDraft::open(
                    &outcome.path,
                    DraftReadLimits {
                        max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
                    },
                    hex_project::MAX_DEVICE_AUTOMATION_POINTS as u64,
                )
                .map_err(|error| error.to_string())?;
                if take.target() != target.draft {
                    return Err("sealed group does not match the original capture target".into());
                }
                prepare_group_take_adoption_on_control(
                    session,
                    document_base,
                    &mut take,
                    scope.stamp,
                )
            })()
            .map_err(|error| retained_error(&outcome.path, &error))?;
            return Ok(Step::Commit(Box::new(command), outcome.path));
        }
        let mut take = CompletedAutomationTake::open_on_control(
            &outcome.path,
            DraftReadLimits {
                max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
            },
        )
        .map_err(|error| retained_error(&outcome.path, &error.to_string()))?;
        let command =
            prepare_device_take_adoption_on_control(session, document_base, &mut take, scope.stamp)
                .map_err(|error| retained_error(&outcome.path, &error))?;
        Ok(Step::Commit(Box::new(command), outcome.path))
    }
}

pub(super) fn retained_error(path: &Path, reason: &str) -> String {
    format!(
        "{reason} · automation draft retained at {}",
        hex_storage::text::verbatim(path.display())
    )
}

fn draft_path(directory: &Path) -> Result<PathBuf, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    match std::fs::symlink_metadata(directory) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err("automation recovery directory must be a real directory".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let serial = NEXT.fetch_add(1, Ordering::Relaxed);
    Ok(directory.join(format!(
        "device-{}-{time}-{serial}.hex-automation-draft",
        std::process::id()
    )))
}

#[cfg(test)]
pub(super) mod tests;
