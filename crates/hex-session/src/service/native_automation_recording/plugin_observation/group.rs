//! Failed grouped input uses the same writer-prefix copy and receipt. Only
//! original, source-matched Main observations may extend a failed prefix.
use super::*;
use crate::service::{MAX_VST3_EDITOR_GROUP_MEMBERS, automation_draft};
use hex_storage::automation_draft::GroupApplication;

pub(super) fn copy_failed_group(
    outcome: &DraftWorkerOutcome,
    destination: &Path,
    limits: DraftReadLimits,
    max_events: u64,
    scope: DeviceAutomationTakeScope,
    source: DeviceAutomationSource,
    observations: &[Observation],
) -> Result<RecoveryCopyReceipt, String> {
    if outcome.failure.is_none()
        || !source.matches(scope)
        || max_events > hex_project::MAX_DEVICE_AUTOMATION_POINTS as u64
        || observations.len() > MAX_FAILED_PLUGIN_EDITOR_OBSERVATIONS
    {
        return Err("invalid grouped vst3 recovery source or capacity".into());
    }
    let max_tail = MAX_FAILED_PLUGIN_EDITOR_OBSERVATIONS * MAX_VST3_EDITOR_GROUP_MEMBERS;
    automation_draft::copy_group_outcome_with_tail_on_control(
        outcome,
        destination,
        limits,
        max_events,
        max_tail,
        |reader, mut previous| {
            if reader.provenance() != scope.stamp
                || reader.target() != scope.target.draft_target()
                || reader.capture_clock() != Some(source.capture_clock(reader.source()))
            {
                return Err("grouped vst3 recovery differs from the original draft".into());
            }
            if outcome.requested_terminal != DraftTerminal::Interrupted {
                if !observations.is_empty() {
                    return Err("grouped vst3 observation follows the admitted terminal".into());
                }
                return Ok((Vec::new(), outcome.requested_terminal));
            }
            let mut rows = Vec::new();
            rows.try_reserve_exact(observations.len() * MAX_VST3_EDITOR_GROUP_MEMBERS)
                .map_err(|_| "grouped vst3 recovery continuation storage refused")?;
            let mut terminal = None;
            for observation in observations {
                let Observation::Grouped(group) = *observation else {
                    return Err("grouped vst3 recovery received a single-target observation".into());
                };
                if terminal.is_some() || !group.matches(scope, source) {
                    return Err("grouped vst3 recovery observation belongs to another take or follows its terminal".into());
                }
                match group.receipt {
                    Vst3EditorGroupReceipt::Ready => {
                        return Err("grouped vst3 recovery is already admitted".into());
                    }
                    Vst3EditorGroupReceipt::Applied(batch) => {
                        for row in batch.rows() {
                            if previous.map_or(Some(1), |old| old.ordinal.checked_add(1))
                                != Some(row.ordinal)
                            {
                                return Err("grouped vst3 recovery ordinal does not continue its original prefix".into());
                            }
                            previous = Some(row);
                            rows.push(row);
                        }
                    }
                    Vst3EditorGroupReceipt::Finished(clock) => {
                        if previous.is_none_or(|row| {
                            row.clock != clock || row.application != GroupApplication::Restored
                        }) {
                            return Err(
                                "grouped vst3 recovery terminal lacks its last real restoration"
                                    .into(),
                            );
                        }
                        terminal = Some(DraftTerminal::Finished(clock));
                    }
                    Vst3EditorGroupReceipt::Interrupted => {
                        terminal = Some(DraftTerminal::Interrupted)
                    }
                }
            }
            Ok((rows, terminal.unwrap_or(DraftTerminal::Interrupted)))
        },
    )
}
