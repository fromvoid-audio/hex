//! Bounded, non-destructive copies of failed writer custody.
use super::{DraftTerminal, DraftWorkerOutcome, EVENTS_PER_BATCH};
use hex_project::Project;
use hex_storage::automation_draft::{
    CaptureClock, DraftProvenance, DraftReadLimits, DraftReader, DraftTarget, DraftWriter,
};
use std::path::Path;

/// Original capture identity retained by the recording owner.
pub struct RecoverySource<'a> {
    pub project: &'a Project,
    pub target: DraftTarget,
    pub provenance: DraftProvenance,
    pub clock: Option<CaptureClock>,
}

/// Copy a failed writer's validated prefix and retained suffix to a new interrupted draft.
/// The original file and borrowed batches remain owned by the caller on every outcome.
/// At most `max_events` records are scanned or copied, plus one record to detect overflow.
/// This checkpoint is not an adoptable completed take and does not include later receipts.
pub fn copy_failed_on_control(
    outcome: &DraftWorkerOutcome,
    expected: RecoverySource<'_>,
    destination: &Path,
    limits: DraftReadLimits,
    max_events: u64,
) -> Result<(), String> {
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
        || acknowledged.checked_add(retained_count) != Some(outcome.admitted_events)
    {
        return Err("automation writer custody count is inconsistent or exceeds budget".into());
    }
    let mut reader = DraftReader::open_bounded(&outcome.path, limits, max_events)
        .map_err(|error| error.to_string())?;
    if reader.target() != expected.target
        || reader.provenance() != expected.provenance
        || reader.capture_clock() != expected.clock
        || !reader
            .source_matches(expected.project)
            .map_err(|error| error.to_string())?
    {
        return Err("automation draft no longer matches original capture identity".into());
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
            ordinal += 1;
        }
        writer.append(&events).map_err(|error| error.to_string())?;
    }
    loop {
        let events: Vec<_> = retained.by_ref().take(EVENTS_PER_BATCH).collect();
        if events.is_empty() {
            break;
        }
        writer.append(&events).map_err(|error| error.to_string())?;
    }
    writer
        .finish(DraftTerminal::Interrupted)
        .map_err(|error| error.to_string())?;
    Ok(())
}
