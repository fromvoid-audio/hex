//! Single control-owned native touch take. The audio client supplies delivery
//! receipts and retains a batch until Session acknowledges its admission.
//! The disk worker seals the file independently of callback and UI lifetimes.
use super::Session;
use super::automation_draft::{DeviceDraftInput, DraftProvenance, DraftWorker, DraftWorkerOutcome};
use super::automation_take::{CompletedAutomationTake, prepare_native_take_adoption_on_control};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeAutomationTakeScope {
    pub target: NativeParameterInputTarget,
    pub stamp: DraftProvenance,
}

impl NativeAutomationTakeScope {
    pub(super) fn context(self) -> AudioSourceContext {
        AudioSourceContext {
            project_generation: self.stamp.project_generation,
            editor_revision: self.stamp.editor_revision,
        }
    }
}

/// Copied observation of the applied and playing resident output. The
/// callback still checks the actual source and epoch when admitting Begin.
#[derive(Clone, Copy)]
pub struct NativeAutomationSource {
    pub target: NativeParameterInputTarget,
    pub sample_rate: u32,
    pub output_identity: u64,
    pub audio_revision: u64,
    pub transport_epoch: u64,
    pub section_loop: Option<hex_project::LoopRegion>,
}

impl NativeAutomationSource {
    fn matches(self, scope: NativeAutomationTakeScope) -> bool {
        self.target == scope.target
            && self.sample_rate == scope.stamp.sample_rate
            && self.output_identity == scope.stamp.output_identity
            && self.audio_revision == scope.stamp.audio_revision
            && self.transport_epoch == scope.stamp.transport_epoch
    }
}

#[derive(Debug)]
pub enum NativeAutomationTakeAction {
    Begin,
    Append(Arc<[(u64, NativeParameterApplication)]>),
    Finish(NativeParameterApplication),
    Abort(DraftAbortReason),
    Interrupt,
    /// Poll without waiting for disk. A sealed matching take commits during
    /// this request through the ordinary Session edit transaction.
    Poll,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeAutomationTakeProgress {
    Started,
    Appended,
    Backpressured,
    Sealing,
    Pending,
    Committed,
    Retained(PathBuf),
    Refused,
}

pub struct NativeAutomationTakeCompletion {
    pub request_id: u64,
    pub progress: NativeAutomationTakeProgress,
}

#[derive(Default)]
pub(super) struct Lane {
    active: Option<Active>,
    // A failed writer can still own unsynced original batches. Keep these
    // with their original source and refuse new takes until recovery.
    failed: Option<Box<DraftWorkerOutcome>>,
}

struct Active {
    scope: NativeAutomationTakeScope,
    input: DeviceDraftInput,
    worker: DraftWorker,
}

pub(super) enum Step {
    Progress(NativeAutomationTakeProgress),
    Commit(Box<hex_project::Command>, PathBuf),
}

impl Lane {
    pub(super) fn begin(
        &mut self,
        session: &Session,
        document_base: Option<&Path>,
        scope: NativeAutomationTakeScope,
        source: NativeAutomationSource,
        directory: &Path,
    ) -> Result<Step, String> {
        if let Some(failed) = self.failed.as_ref() {
            return Err(retained_error(
                &failed.path,
                "recover the previous native automation take first",
            ));
        }
        if self.active.is_some() {
            return Err("a native automation take already owns the input".into());
        }
        if !source.matches(scope) || source.sample_rate != session.project().sample_rate() {
            return Err("native automation source or transport changed".into());
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
        let (input, worker) = DeviceDraftInput::start_on_control(
            session,
            document_base,
            path,
            scope.target,
            scope.stamp,
            clock,
        )?;
        self.active = Some(Active {
            scope,
            input,
            worker,
        });
        Ok(Step::Progress(NativeAutomationTakeProgress::Started))
    }

    pub(super) fn receive(
        &mut self,
        session: &Session,
        document_base: Option<&Path>,
        current: AudioSourceContext,
        scope: NativeAutomationTakeScope,
        action: &NativeAutomationTakeAction,
    ) -> Result<Step, String> {
        use NativeAutomationTakeProgress as Progress;
        let active = self
            .active
            .as_mut()
            .ok_or("native automation take is no longer active")?;
        if active.scope != scope {
            return Err("native automation receipt belongs to another take".into());
        }
        while active.input.poll().is_some() {}
        let progress = match action {
            NativeAutomationTakeAction::Begin => {
                return Err("native automation take is already active".into());
            }
            NativeAutomationTakeAction::Append(batch) => {
                if active
                    .input
                    .append_delivered(scope.target, scope.stamp, batch)?
                    .is_some()
                {
                    Progress::Appended
                } else {
                    Progress::Backpressured
                }
            }
            NativeAutomationTakeAction::Finish(application) => {
                active
                    .input
                    .finish_delivered(scope.target, scope.stamp, *application)?;
                Progress::Sealing
            }
            NativeAutomationTakeAction::Abort(reason) => {
                active
                    .input
                    .abort_delivered(scope.target, scope.stamp, *reason)?;
                Progress::Sealing
            }
            NativeAutomationTakeAction::Interrupt => {
                active.worker.request_interruption_on_control()?;
                Progress::Sealing
            }
            NativeAutomationTakeAction::Poll => {
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
        scope: NativeAutomationTakeScope,
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
            return Ok(Step::Progress(NativeAutomationTakeProgress::Retained(
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
            prepare_native_take_adoption_on_control(session, document_base, &mut take, scope.stamp)
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
        "native-{}-{time}-{serial}.hex-automation-draft",
        std::process::id()
    )))
}
