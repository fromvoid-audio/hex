//! Bounded, non-destructive copies of failed writer custody.
use super::{
    AppliedEvent, DraftPrefixIdentity, DraftTerminal, DraftWorkerOutcome, EVENTS_PER_BATCH,
};
use hex_storage::automation_draft::{
    CompletedAutomationTake, DraftOutcome, DraftReadLimits, DraftReader, DraftWriter,
};
use std::path::Path;

/// Identity of the verified copy, not permission to release the original custody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryCopyReceipt {
    prefix: DraftPrefixIdentity,
    terminal: DraftTerminal,
}

impl RecoveryCopyReceipt {
    pub fn terminal(self) -> DraftTerminal {
        self.terminal
    }

    pub fn events(self) -> u64 {
        self.prefix.events()
    }

    /// Revalidate while holding the reader lock. A later replacement remains
    /// possible after this returns; adoption must revalidate in its own custody.
    pub fn verify_on_control(
        self,
        path: &Path,
        limits: DraftReadLimits,
        max_events: u64,
    ) -> Result<(), String> {
        if self.events() > max_events {
            return Err("automation recovery receipt exceeds budget".into());
        }
        let mut reader = DraftReader::open_bounded(path, limits, max_events)
            .map_err(|error| error.to_string())?;
        self.verify_reader(&mut reader)
    }

    /// Verify and prepare a finished take from the same locked reader. This is
    /// not permission to release the original custody or commit an edit.
    pub fn open_completed_on_control(
        self,
        path: &Path,
        limits: DraftReadLimits,
        max_events: u64,
    ) -> Result<CompletedAutomationTake, String> {
        if self.events() > max_events {
            return Err("automation recovery receipt exceeds budget".into());
        }
        let mut reader = DraftReader::open_bounded(path, limits, max_events)
            .map_err(|error| error.to_string())?;
        self.verify_reader(&mut reader)?;
        let completed = reader.into_completed().map_err(|error| error.to_string())?;
        CompletedAutomationTake::from_completed_on_control(completed)
    }

    fn verify_reader(self, reader: &mut DraftReader) -> Result<(), String> {
        let expected = match self.terminal {
            DraftTerminal::Finished(clock) => DraftOutcome::Finished(clock),
            DraftTerminal::Aborted(reason) => DraftOutcome::Aborted(reason),
            DraftTerminal::Interrupted => DraftOutcome::Interrupted,
        };
        if reader.outcome() != expected
            || reader.progress().durable_terminal != Some(self.terminal)
            || reader.progress().durable_events != self.events()
            || !reader
                .matches_prefix(self.prefix)
                .map_err(|error| error.to_string())?
        {
            return Err("automation recovery copy no longer matches its receipt".into());
        }
        Ok(())
    }
}

/// Copy a failed writer's validated prefix and retained suffix to a new interrupted draft.
/// The original file and borrowed batches remain owned by the caller on every outcome.
/// Each scan and the copy is capped at `max_events`; opening reads one extra record
/// to detect overflow. Revalidation and output verification make multiple passes.
/// This checkpoint is not an adoptable completed take and does not include later receipts.
/// File operations and sync can block; the caller must keep them off the audio callback
/// and arrange worker custody if its control loop must remain responsive. A failure after
/// destination publication leaves that incomplete file in place for inspection.
pub fn copy_failed_on_control(
    outcome: &DraftWorkerOutcome,
    destination: &Path,
    limits: DraftReadLimits,
    max_events: u64,
) -> Result<(), String> {
    copy_failed_with_tail_on_control(outcome, destination, limits, max_events, |_, _| {
        Ok((Vec::new(), DraftTerminal::Interrupted))
    })
    .map(|_| ())
}

/// The callback validates the source and bounded tail against the actual copied
/// predecessor. It runs on the control worker while the original reader is held.
pub(in crate::service) fn copy_failed_with_tail_on_control(
    outcome: &DraftWorkerOutcome,
    destination: &Path,
    limits: DraftReadLimits,
    max_events: u64,
    prepare: impl FnOnce(
        &DraftReader,
        Option<AppliedEvent>,
    ) -> Result<(Vec<AppliedEvent>, DraftTerminal), String>,
) -> Result<RecoveryCopyReceipt, String> {
    if outcome.failure.is_none() || outcome.retained.len() > super::BATCH_SLOTS {
        return Err("automation writer outcome is not a bounded failure".into());
    }
    let retained_count = outcome
        .retained
        .iter()
        .try_fold(0_u64, |count, batch| {
            count.checked_add(batch.events().len() as u64)
        })
        .ok_or("automation retained event count overflow")?;
    let acknowledged = outcome.progress.durable_events;
    if outcome.admitted_events > max_events
        || outcome.durable_prefix.events() != acknowledged
        || acknowledged.checked_add(retained_count) != Some(outcome.admitted_events)
    {
        return Err("automation writer custody count is inconsistent or exceeds budget".into());
    }
    let mut reader = DraftReader::open_bounded(&outcome.path, limits, max_events)
        .map_err(|error| error.to_string())?;
    if !reader
        .matches_prefix(outcome.durable_prefix)
        .map_err(|error| error.to_string())?
    {
        return Err("automation draft acknowledged prefix no longer matches writer custody".into());
    }
    let prefix = reader.progress().durable_events;
    if prefix < acknowledged || prefix > outcome.admitted_events {
        return Err("automation draft prefix does not match acknowledged custody".into());
    }
    let mut retained = outcome
        .retained
        .iter()
        .flat_map(|batch| batch.events().iter().copied());
    let mut writer = match reader.capture_clock() {
        Some(clock) => DraftWriter::create_with_clock(
            destination,
            reader.source(),
            reader.target(),
            reader.provenance(),
            clock,
        ),
        None => DraftWriter::create_new(
            destination,
            reader.source(),
            reader.target(),
            reader.provenance(),
        ),
    }
    .map_err(|error| error.to_string())?;
    let mut page = [None; EVENTS_PER_BATCH];
    let mut ordinal = 0_u64;
    let mut previous = None;
    while ordinal < prefix {
        let count = reader
            .read_events(ordinal, &mut page)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("automation draft prefix ended unexpectedly".into());
        }
        let mut events = Vec::with_capacity(count);
        for event in page.iter().take(count) {
            let event = event.ok_or("automation draft prefix is incomplete")?;
            if ordinal >= acknowledged && retained.next() != Some(event) {
                return Err("automation retained events do not match the draft prefix".into());
            }
            events.push(event);
            previous = Some(event);
            ordinal += 1;
        }
        writer.append(&events).map_err(|error| error.to_string())?;
    }
    loop {
        let events: Vec<_> = retained.by_ref().take(EVENTS_PER_BATCH).collect();
        if events.is_empty() {
            break;
        }
        previous = events.last().copied();
        writer.append(&events).map_err(|error| error.to_string())?;
    }
    let (tail, terminal) = prepare(&reader, previous)?;
    if tail.len() > 3
        || outcome
            .admitted_events
            .checked_add(tail.len() as u64)
            .is_none_or(|count| count > max_events)
    {
        return Err("automation recovery continuation exceeds budget".into());
    }
    if !tail.is_empty() {
        writer.append(&tail).map_err(|error| error.to_string())?;
    }
    writer.finish(terminal).map_err(|error| error.to_string())?;
    let copied = writer.durable_prefix_identity();
    drop(writer);
    let mut verified = DraftReader::open_bounded(destination, limits, max_events)
        .map_err(|error| error.to_string())?;
    let expected = match terminal {
        DraftTerminal::Finished(clock) => DraftOutcome::Finished(clock),
        DraftTerminal::Aborted(reason) => DraftOutcome::Aborted(reason),
        DraftTerminal::Interrupted => DraftOutcome::Interrupted,
    };
    if verified.outcome() != expected
        || verified.progress().durable_terminal != Some(terminal)
        || copied.events() != outcome.admitted_events + tail.len() as u64
        || !verified
            .matches_prefix(copied)
            .map_err(|error| error.to_string())?
    {
        return Err("automation recovery copy did not verify; preserve both drafts".into());
    }
    Ok(RecoveryCopyReceipt {
        prefix: copied,
        terminal,
    })
}
