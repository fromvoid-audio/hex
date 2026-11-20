//! Desktop custody of an original editor source until Root can release it.
use super::*;
use hex_audio::{RetiringEditLease, RetiringEditProgress, RetiringEditReceipt};
use hex_ui::{EditorAudioEvent, PluginEditorRetirementIssue};

#[derive(Default)]
enum Stage {
    #[default]
    Claim,
    Read,
    Retain(RetiringEditReceipt),
    NeedsDisposition,
    Failed,
}

#[derive(Default)]
pub(super) struct Retirement {
    pub(super) lease: Option<RetiringEditLease>,
    pub(super) ever_accepted: bool,
    stage: Stage,
    issue: Option<PluginEditorRetirementIssue>,
    latency_reported: bool,
}

impl Retirement {
    pub(super) fn parked(&self) -> bool {
        matches!(self.stage, Stage::NeedsDisposition | Stage::Failed)
    }

    pub(super) fn issue_pending(&self) -> bool {
        self.issue.is_some()
    }

    fn waiting(&self) -> bool {
        self.lease.is_some() && !self.parked()
    }

    fn fail(&mut self, reason: &'static str) {
        if !matches!(
            reason,
            "native preparation is busy"
                | "native unaccepted source is not retiring"
                | "native unaccepted source is still referenced"
                | "native source is not retiring"
                | "native edit observation is busy"
                | "native controller mailbox is busy"
                | "native controller query is already pending"
                | "native stopped control operation is busy"
                | "native editor gesture remains active"
        ) {
            self.issue = Some(PluginEditorRetirementIssue::Failure(reason));
            self.stage = Stage::Failed;
        }
    }
}

impl Plugins {
    pub(in crate::native_audio) fn handoff_editor_receipts_on_shutdown(&mut self) {
        for source in &mut self.sources {
            let retirement = &mut source.editor_retirement;
            if let Some(lease) = retirement.lease.take() {
                lease.handoff_on_shutdown(
                    retirement.ever_accepted,
                    source.edit_recovery.map(|read| read.recovery),
                    None,
                );
            }
        }
    }

    /// Share the existing control poll with retirement even during sustained
    /// edits. Bound the number of SDK polls and yield after the current step
    /// if it consumed the time available. Root keeps custody between turns.
    pub(in crate::native_audio) fn poll_retiring_edits(&mut self) {
        const MAX_SOURCES_PER_TURN: usize = 4;
        const POLL_SLICE: std::time::Duration = std::time::Duration::from_millis(2);

        let start = std::time::Instant::now();
        let len = self.sources.len();
        let mut advanced = 0;
        for _ in 0..len {
            let index = self.retiring_poll % len;
            self.retiring_poll = (index + 1) % len;
            if self.sources[index].retiring && self.sources[index].editor_retirement.waiting() {
                self.poll_retiring_editor(index);
                advanced += 1;
                if advanced == MAX_SOURCES_PER_TURN || start.elapsed() >= POLL_SLICE {
                    break;
                }
            }
        }
    }

    fn poll_retiring_editor(&mut self, index: usize) {
        let source = &mut self.sources[index];
        let retirement = &mut source.editor_retirement;
        let Some(lease) = retirement.lease.as_mut() else {
            return;
        };
        if !retirement.ever_accepted {
            match lease.cancel_unaccepted() {
                Ok(()) => {
                    retirement.lease = None;
                    source.edit_recovery = None;
                }
                Err(reason) => retirement.fail(reason),
            }
            return;
        }
        match retirement.stage {
            Stage::Claim => {
                if let Err(reason) = lease.claim(source.edit_recovery.map(|read| read.recovery)) {
                    retirement.fail(reason);
                    return;
                }
                retirement.stage = Stage::Read;
            }
            Stage::Retain(receipt) => {
                if let Err(reason) = lease.retain_page(receipt) {
                    retirement.fail(reason);
                    return;
                }
                retirement.stage = Stage::Read;
            }
            Stage::Read => {}
            Stage::NeedsDisposition | Stage::Failed => return,
        }

        match lease.poll() {
            Ok(RetiringEditProgress::Pending) => {}
            Ok(RetiringEditProgress::Batch(receipt, batch)) => {
                let binding = source.binding;
                source.edit_recovery = None;
                retirement.stage = Stage::Retain(receipt);
                if batch.pending_latency_restart.is_some() && !retirement.latency_reported {
                    retirement.issue = Some(PluginEditorRetirementIssue::TailLatencyPending);
                    retirement.latency_reported = true;
                }
                self.observe_recording_order(binding, batch.edits.as_slice());
                #[cfg(all(test, feature = "allocation-probe"))]
                if let Some(progress) = &self.progress
                    && let Ok(mut progress) = progress.lock()
                {
                    for edit in batch.edits.as_slice() {
                        let count = progress.editor_receipt_count;
                        if let Some(slot) = progress.editor_receipts.get_mut(count) {
                            *slot = Some(*edit);
                            progress.editor_receipt_count += 1;
                        }
                    }
                }
            }
            Ok(RetiringEditProgress::Disposition(_)) => {
                retirement.stage = Stage::NeedsDisposition;
                retirement.issue = Some(PluginEditorRetirementIssue::NeedsDisposition);
            }
            Ok(RetiringEditProgress::Complete) => {
                source.edit_recovery = None;
                retirement.lease = None;
            }
            Err(reason) => retirement.fail(reason),
        }
    }

    pub(in crate::native_audio) fn take_retiring_editor_issue(
        &mut self,
    ) -> Option<EditorAudioEvent> {
        self.sources.iter_mut().find_map(|source| {
            let issue = source.editor_retirement.issue.take()?;
            Some(EditorAudioEvent::PluginEditorRetirementIssue {
                project_generation: source.project_generation,
                device: source.device,
                issue,
            })
        })
    }
}
