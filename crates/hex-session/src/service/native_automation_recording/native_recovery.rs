//! Native recovery consumes original delivered observations after quiescence.
//! The caller still owns every receipt and the exact take/source; a verified
//! copy grants neither canonical adoption nor permission to release custody.
use super::*;
use crate::service::automation_draft::{
    AppliedEvent, DraftTarget, EVENTS_PER_BATCH, copy_outcome_with_tail_on_control,
    native_application_clock,
};
use hex_engine::NativeParameterCapture;

/// At most one UI writer batch plus the endpoint's outstanding normal credits.
/// Terminal observations occupy their own slot and are never counted as points.
pub const MAX_NATIVE_RECOVERY_CONTINUATION_EVENTS: usize = 2 * EVENTS_PER_BATCH;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NativeRecoveryTerminal {
    Restored(NativeParameterApplication),
    Aborted(DraftAbortReason),
    Interrupted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeRecoveryContinuation {
    pub applications: Arc<[(u64, NativeParameterApplication)]>,
    pub terminal: NativeRecoveryTerminal,
}

/// Copy a failed or explicitly interrupted writer and the original native
/// applications it did not admit. `source` is the retained source, not a
/// reconstructed live output. No original file or in-memory input is changed.
/// Call on a recovery worker or other non-realtime control owner.
pub fn copy_native_recovery_on_control(
    outcome: &DraftWorkerOutcome,
    destination: &Path,
    limits: DraftReadLimits,
    max_events: u64,
    scope: DeviceAutomationTakeScope,
    source: DeviceAutomationSource,
    continuation: &NativeRecoveryContinuation,
) -> Result<RecoveryCopyReceipt, String> {
    let target = scope
        .target
        .native()
        .ok_or("native recovery requires a native target")?;
    if !source.matches(scope) {
        return Err("native recovery source differs from the original take".into());
    }
    if continuation.applications.len() > MAX_NATIVE_RECOVERY_CONTINUATION_EVENTS
        || max_events > hex_project::MAX_DEVICE_AUTOMATION_POINTS as u64
        || outcome
            .admitted_events
            .checked_add(continuation.applications.len() as u64)
            .is_none_or(|count| count > max_events)
    {
        return Err("native recovery continuation exceeds its event budget".into());
    }
    if outcome.failure.is_none()
        && (outcome.requested_terminal != DraftTerminal::Interrupted
            || outcome.progress.durable_terminal != Some(DraftTerminal::Interrupted)
            || !outcome.retained.is_empty())
    {
        return Err("native recovery requires a failed or interrupted writer".into());
    }
    copy_outcome_with_tail_on_control(
        outcome,
        destination,
        limits,
        max_events,
        MAX_NATIVE_RECOVERY_CONTINUATION_EVENTS,
        |reader, previous| {
            if reader.provenance() != scope.stamp
                || reader.target()
                    != (DraftTarget::Device {
                        device: target.device(),
                        target: target.target(),
                    })
                || reader.capture_clock() != Some(source.capture_clock(reader.source()))
            {
                return Err("native recovery draft differs from the original scope".into());
            }
            let durable_terminal = reader.progress().durable_terminal;
            if durable_terminal.is_some_and(|terminal| terminal != outcome.requested_terminal)
                || outcome
                    .progress
                    .durable_terminal
                    .is_some_and(|terminal| durable_terminal != Some(terminal))
            {
                return Err("native recovery draft terminal differs from writer custody".into());
            }
            let capture = NativeParameterCapture::prepare(reader.source(), target)
                .map_err(|error| error.0.to_owned())?;
            let mut sequence = previous.map_or(0, |event| event.sequence);
            let mut events = Vec::with_capacity(continuation.applications.len());
            for &(next, application) in continuation.applications.iter() {
                if next <= sequence {
                    return Err("native recovery sequence overlaps the admitted writer".into());
                }
                let value = capture
                    .value(application)
                    .map_err(|error| error.0)?
                    .ok_or("native recovery application is a restoration")?;
                let clock = native_application_clock(application);
                events.push(AppliedEvent {
                    sequence: next,
                    performance_frame: clock.performance_frame,
                    content_frame: clock.content_frame,
                    loop_pass: clock.loop_pass,
                    value,
                });
                sequence = next;
            }
            let observed = match continuation.terminal {
                NativeRecoveryTerminal::Restored(application) => {
                    if capture
                        .value(application)
                        .map_err(|error| error.0)?
                        .is_some()
                    {
                        return Err("native recovery terminal is not a restoration".into());
                    }
                    DraftTerminal::Finished(native_application_clock(application))
                }
                NativeRecoveryTerminal::Aborted(reason) => DraftTerminal::Aborted(reason),
                NativeRecoveryTerminal::Interrupted => DraftTerminal::Interrupted,
            };
            let terminal = match outcome.requested_terminal {
                DraftTerminal::Interrupted => observed,
                original if observed == DraftTerminal::Interrupted || observed == original => {
                    original
                }
                _ => {
                    return Err("native recovery terminal contradicts the admitted terminal".into());
                }
            };
            Ok((events, terminal))
        },
    )
}

impl Lane {
    pub(super) fn prepare_native_recovery(
        &mut self,
        scope: DeviceAutomationTakeScope,
    ) -> Result<Step, String> {
        if scope.target.native().is_none() {
            return Err("native recovery requires a native target".into());
        }
        if self.failed.is_some() {
            let path = self.failed_outcome(scope)?.path.clone();
            return Ok(Step::Progress(DeviceAutomationTakeProgress::Retained(path)));
        }
        let active = self
            .active
            .as_mut()
            .filter(|active| active.scope == scope && active.source.matches(scope))
            .ok_or("native recovery does not own the original writer")?;
        if !active.preparing_native_recovery {
            let progress = active.input.progress();
            if progress
                .terminal_requested
                .is_some_and(|terminal| terminal != DraftTerminal::Interrupted)
                && !progress.failed
            {
                return Err("native writer already has a terminal · poll its outcome first".into());
            }
            if progress.terminal_requested.is_none() {
                active.worker.request_interruption_on_control()?;
            }
            active.preparing_native_recovery = true;
        }
        while active.input.poll().is_some() {}
        let Some(outcome) = active.input.try_take_outcome() else {
            return Ok(Step::Progress(DeviceAutomationTakeProgress::Pending));
        };
        let source = active.source;
        let path = outcome.path.clone();
        self.active = None;
        self.failed = Some(Failed {
            scope,
            source,
            outcome: Arc::new(outcome),
        });
        Ok(Step::Progress(DeviceAutomationTakeProgress::Retained(path)))
    }

    pub(super) fn start_native_recovery_copy(
        &mut self,
        scope: DeviceAutomationTakeScope,
        destination: &Path,
        max_events: u64,
        continuation: &NativeRecoveryContinuation,
    ) -> Result<Step, String> {
        if scope.target.native().is_none() {
            return Err("native recovery requires a native target".into());
        }
        let source = self.failed_source(scope)?;
        let outcome = Arc::clone(self.failed_outcome(scope)?);
        let path = super::super::recovery::absolute_document_path(destination)?;
        let copy_path = path.clone();
        let continuation = continuation.clone();
        self.failed_copy
            .start_with_receipt(&outcome, move |outcome| {
                copy_native_recovery_on_control(
                    outcome,
                    &copy_path,
                    copy_limits(),
                    max_events,
                    scope,
                    source,
                    &continuation,
                )
            })?;
        self.copy_destination = Some(PendingCopy::Native(path));
        Ok(Step::Progress(DeviceAutomationTakeProgress::Pending))
    }

    pub(super) fn keep_native_recovery_copy(
        &mut self,
        scope: DeviceAutomationTakeScope,
        selected: &Path,
        receipt: RecoveryCopyReceipt,
        max_events: u64,
    ) -> Result<Step, String> {
        if scope.target.native().is_none()
            || max_events > hex_project::MAX_DEVICE_AUTOMATION_POINTS as u64
        {
            return Err("invalid native recovery target or event budget".into());
        }
        let outcome = Arc::clone(self.failed_outcome(scope)?);
        if self.copy_destination.is_some() {
            return Err("native recovery copy is still pending".into());
        }
        let (path, _) = self
            .verified_native_copy
            .as_ref()
            .filter(|(path, current)| path.as_path() == selected && *current == receipt)
            .ok_or("verified native recovery copy changed or is unavailable")?;
        let path = path.clone();
        receipt.with_verified_on_control(&path, copy_limits(), max_events, || {
            if !Arc::ptr_eq(self.failed_outcome(scope)?, &outcome) {
                return Err("original native recovery custody changed".into());
            }
            self.verified_native_copy = None;
            self.failed = None;
            Ok(())
        })?;
        Ok(Step::Progress(DeviceAutomationTakeProgress::CopyKept(path)))
    }
}

fn copy_limits() -> DraftReadLimits {
    DraftReadLimits {
        max_source_bytes: hex_storage::MAX_PROJECT_DOCUMENT_BYTES as u64,
    }
}

#[cfg(test)]
mod tests;
