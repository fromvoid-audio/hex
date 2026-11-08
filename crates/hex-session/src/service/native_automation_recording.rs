//! Single control-owned device touch take. The audio client supplies delivery
//! receipts and retains a batch until Session acknowledges its admission.
//! The disk worker seals the file independently of callback and UI lifetimes.
use super::Session;
mod plugin_observation;
use super::automation_draft::{
    DeviceDraftInput, DraftProvenance, DraftWorker, DraftWorkerOutcome, Vst3CaptureTarget,
    Vst3ParameterApplication,
};
use super::automation_take::{CompletedAutomationTake, prepare_device_take_adoption_on_control};
use hex_engine::{AudioSourceContext, NativeParameterApplication, NativeParameterInputTarget};
use hex_storage::automation_draft::{
    CaptureClock, DraftAbortReason, DraftReadLimits, DraftTerminal,
};
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
}
impl DeviceCaptureTarget {
    pub fn native(self) -> Option<NativeParameterInputTarget> {
        match self {
            Self::Native(target) => Some(target),
            Self::Vst3(_) => None,
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

/// Copied observation of the applied and playing resident output. The
/// callback still checks the actual source and epoch when admitting Begin.
#[derive(Clone, Copy, Debug)]
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
}

#[derive(Debug)]
pub enum DeviceAutomationTakeAction {
    Begin,
    Append(Arc<[(u64, NativeParameterApplication)]>),
    Finish(NativeParameterApplication),
    AppendVst3(Arc<[(u64, Vst3ParameterApplication)]>),
    FinishVst3(Vst3ParameterApplication),
    /// Original SDK/final-output observation. Retain it until Appended or
    /// Sealing; Backpressured does not consume its sequence or original clock.
    PluginObservation(Box<super::PluginParameterObservation>),
    Abort(DraftAbortReason),
    Interrupt,
    /// Poll without waiting for disk. A sealed matching take commits during
    /// this request through the ordinary Session edit transaction.
    Poll,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceAutomationTakeProgress {
    Started,
    Appended,
    Backpressured,
    Sealing,
    Pending,
    Committed,
    Retained(PathBuf),
    Refused,
}

pub struct DeviceAutomationTakeCompletion {
    pub request_id: u64,
    pub progress: DeviceAutomationTakeProgress,
}

#[derive(Default)]
pub(super) struct Lane {
    active: Option<Active>,
    // A failed writer can still own unsynced original batches. Keep these
    // with their original source and refuse new takes until recovery.
    failed: Option<Box<DraftWorkerOutcome>>,
}

struct Active {
    scope: DeviceAutomationTakeScope,
    source: DeviceAutomationSource,
    input: DeviceDraftInput,
    worker: DraftWorker,
}

pub(super) enum Step {
    Progress(DeviceAutomationTakeProgress),
    Commit(Box<hex_project::Command>, PathBuf),
}

impl Lane {
    pub(super) fn busy(&self) -> bool {
        self.active.is_some()
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
                &failed.path,
                "recover the previous device automation take first",
            ));
        }
        if self.active.is_some() {
            return Err("a device automation take already owns the input".into());
        }
        if !source.matches(scope) || source.sample_rate != session.project().sample_rate() {
            return Err("device automation source or transport changed".into());
        }
        let clock = CaptureClock {
            sample_rate: source.sample_rate,
            loop_region: source.section_loop.or_else(|| {
                session
                    .project()
                    .loop_region()
                    .filter(|region| region.enabled())
            }),
        };
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
        }?;
        self.active = Some(Active {
            scope,
            source,
            input,
            worker,
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
        let active = self
            .active
            .as_mut()
            .ok_or("device automation take is no longer active")?;
        if active.scope != scope {
            return Err("device automation receipt belongs to another take".into());
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
                self.active = None;
                return self.resolve(session, document_base, current, scope, outcome);
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
        outcome: DraftWorkerOutcome,
    ) -> Result<Step, String> {
        if let Some(reason) = outcome.failure.as_deref() {
            let error = retained_error(&outcome.path, reason);
            self.failed = Some(Box::new(outcome));
            return Err(error);
        }
        if !matches!(outcome.requested_terminal, DraftTerminal::Finished(_))
            || outcome.admitted_events == 0
            || current != scope.context()
        {
            return Ok(Step::Progress(DeviceAutomationTakeProgress::Retained(
                outcome.path,
            )));
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
