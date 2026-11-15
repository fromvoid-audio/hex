//! Grouped VST3 delivery shares the existing device writer and its single terminal.
//! The caller must retain the exact correlated Main rows until admission succeeds.
use super::{
    Capture, CaptureClock, DeviceDraftInput, DraftProvenance, DraftSubmitError, DraftTerminal,
    DraftWorker, MAX_APPEND_EVENTS, Path, PathBuf, Session,
};
use crate::service::automation_draft::{GroupApplication, GroupDraftEvent, Vst3GroupTarget};
use hex_lattice_contract::ForeignProcessorBinding;

/// Exact live group. The durable target keeps the class and parameter IDs;
/// the original worker binding is checked for admission, never serialized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vst3GroupCaptureTarget {
    pub draft: Vst3GroupTarget,
    pub binding: ForeignProcessorBinding,
}

impl DeviceDraftInput {
    pub(in crate::service) fn start_vst3_group_on_control(
        session: &Session,
        document_base: Option<&Path>,
        path: PathBuf,
        target: Vst3GroupCaptureTarget,
        stamp: DraftProvenance,
        clock: CaptureClock,
    ) -> Result<(Self, DraftWorker), String> {
        if !target.binding.is_valid() || target.binding.root != stamp.output_identity {
            return Err("vst3 group capture binding differs from its output".into());
        }
        let (input, worker) = crate::service::automation_draft::start_group_on_control(
            session,
            document_base,
            path,
            target.draft,
            stamp,
            clock,
        )?;
        Ok((
            Self {
                input,
                capture: Capture::Vst3Group {
                    target,
                    latest: None,
                },
                stamp,
                last_sequence: 0,
            },
            worker,
        ))
    }

    pub(in crate::service) fn append_vst3_group_delivered(
        &mut self,
        target: Vst3GroupCaptureTarget,
        stamp: DraftProvenance,
        rows: &[GroupDraftEvent],
    ) -> Result<Option<u64>, &'static str> {
        let Capture::Vst3Group {
            target: current,
            latest,
        } = &mut self.capture
        else {
            return Err("vst3 group delivery has a different draft kind");
        };
        if *current != target
            || self.stamp != stamp
            || !(1..=MAX_APPEND_EVENTS).contains(&rows.len())
        {
            return Err("vst3 group delivery differs from its scope or batch bound");
        }
        let Some(mut batch) = self.input.reserve_batch()? else {
            return Ok(None);
        };
        let mut last = *latest;
        for &row in rows {
            if last.map_or(Some(1), |row| row.ordinal.checked_add(1)) != Some(row.ordinal)
                || target.draft.ids().binary_search(&row.parameter).is_err()
            {
                return Err("vst3 group row differs from its order or member set");
            }
            batch
                .push_group(row)
                .map_err(|_| "vst3 group delivery exceeds its batch bound")?;
            last = Some(row);
        }
        let ticket = batch.submit().map_err(|error| match error {
            DraftSubmitError::Refused(batch) => batch.reason(),
            DraftSubmitError::InvalidReservation => "vst3 group capture reservation is unavailable",
        })?;
        *latest = last;
        Ok(Some(ticket))
    }

    pub(in crate::service) fn finish_vst3_group_delivered(
        &mut self,
        target: Vst3GroupCaptureTarget,
        stamp: DraftProvenance,
        terminal: DraftTerminal,
    ) -> Result<(), &'static str> {
        let Capture::Vst3Group {
            target: current,
            latest,
        } = &self.capture
        else {
            return Err("vst3 group terminal has a different draft kind");
        };
        if *current != target || self.stamp != stamp {
            return Err("vst3 group terminal differs from its scope");
        }
        if let DraftTerminal::Finished(clock) = terminal
            && latest.is_none_or(|row| {
                row.clock != clock || row.application != GroupApplication::Restored
            })
        {
            return Err("vst3 group terminal has no matching final restoration");
        }
        self.input.finish(terminal)
    }
}
