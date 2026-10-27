//! Lean transfer of an already validated completed source. The file and its lock
//! move without reopening. One bounded captured-tempo copy is made on control;
//! the complete Project and canonical encoding token are then destroyed there.
//! The conversion peak still includes the old checkpoint/token plus that copy.
//! This layout describes retained DATA only, not allocator/RSS, standard-library
//! internals, File/OS state, Arc headers, construction peaks or native admission.
use super::*;
use crate::automation_draft::RecordReader;
use hex_project::TempoMapOwnedAllocationLayout;
use sha2::{Digest, Sha256};

/// Exact destination-map facts for a scalar source's projection metadata.
/// Control/worker preparation only: this scans the bounded tempo-point list,
/// without encoding a temporary Vec, file I/O or per-query recomputation.
/// The source-file identity separately binds the captured map. This fingerprint
/// is neither an authentication signature nor a native publication receipt.
pub fn automation_projection_fingerprint(map: &TempoMap) -> [u8; 32] {
    let mut hash = Sha256::new();
    hex_project::visit_automation_projection_identity(map, &mut |bytes| hash.update(bytes));
    hash.finalize().into()
}

pub struct PreparedAutomationTake {
    pub(super) core: TakeCore<PreparedRecordSource>,
}
// No Project/FileToken/optional full-source branch exists in this owned type.
pub(super) struct PreparedRecordSource {
    records: RecordReader,
    captured_tempo: TempoMap,
    target: DraftTarget,
    provenance: DraftProvenance,
    capture_clock: Option<CaptureClock>,
    end: DraftClock,
    identity: AutomationContentIdentity,
}

/// One retained shared DATA group/handle. Identity is for control accounting,
/// never a user-visible key or publication authority. A zero-size payload does
/// NOT imply zero Arc allocation/header/allocator overhead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutomationSharedData {
    identity: usize,
    data_bytes: usize,
    handles: usize,
}
impl AutomationSharedData {
    pub fn identity(self) -> usize {
        self.identity
    }
    pub fn data_bytes(self) -> usize {
        self.data_bytes
    }
    pub fn handles(self) -> usize {
        self.handles
    }
}
/// Inline ownership is charged once; Vec/path capacities are actual retained
/// backing. Inline arrays are disclosed separately but already included in
/// inline_bytes. Projection-map/cursor backing belongs to their separate owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedAutomationTakeDataLayout {
    inline_bytes: usize,
    path_capacity_bytes: usize,
    captured_tempo: TempoMapOwnedAllocationLayout,
    inline_array_bytes: usize,
    source_owner: AutomationSharedData,
}
impl PreparedAutomationTakeDataLayout {
    pub fn inline_bytes(self) -> usize {
        self.inline_bytes
    }
    pub fn path_capacity_bytes(self) -> usize {
        self.path_capacity_bytes
    }
    pub fn captured_tempo(self) -> TempoMapOwnedAllocationLayout {
        self.captured_tempo
    }
    pub fn inline_array_bytes(self) -> usize {
        self.inline_array_bytes
    }
    pub fn source_owner(self) -> AutomationSharedData {
        self.source_owner
    }
    pub fn checked_exclusive_data_bytes(self) -> Option<usize> {
        self.inline_bytes
            .checked_add(self.path_capacity_bytes)?
            .checked_add(self.captured_tempo.checked_backing_bytes()?)
    }
}

impl CompletedAutomationTake {
    /// Consuming control/worker transfer; never call from callback or GPUI.
    /// Existing cursors retain the same source-origin identity and can resume
    /// against the prepared owner with their original frozen projection.
    /// No file bytes are rewritten/deleted and no unlock/reopen interval occurs.
    pub fn into_prepared_on_control(self) -> Result<PreparedAutomationTake, String> {
        let CompletedAutomationTake {
            core:
                TakeCore {
                    source,
                    mapping,
                    cursor_owner,
                },
        } = self;
        let CompletedDraft {
            reader,
            end,
            identity,
        } = source;
        let super::super::DraftReader {
            records,
            source,
            token,
            target,
            provenance,
            capture_clock,
            outcome: _,
            complete_identity: _,
        } = reader;
        let captured_tempo = source.tempo_map().clone();
        // Both old owning allocations are released before the lean owner escapes.
        drop(source);
        drop(token);
        let prepared = PreparedAutomationTake {
            core: TakeCore {
                source: PreparedRecordSource {
                    records,
                    captured_tempo,
                    target,
                    provenance,
                    capture_clock,
                    end,
                    identity,
                },
                mapping,
                cursor_owner,
            },
        };
        prepared
            .data_layout()
            .checked_exclusive_data_bytes()
            .ok_or("prepared automation source data layout overflow")?;
        Ok(prepared)
    }
}
impl PreparedAutomationTake {
    /// Exhaustive ownership inventory. New retained fields must be accounted
    /// here; there is no catch-all full checkpoint/token branch to overlook.
    pub fn data_layout(&self) -> PreparedAutomationTakeDataLayout {
        let PreparedAutomationTake {
            core:
                TakeCore {
                    source:
                        PreparedRecordSource {
                            records:
                                RecordReader {
                                    file: _,
                                    path,
                                    progress: _,
                                    records_start: _,
                                    header_chain: _,
                                },
                            captured_tempo,
                            target: _,
                            provenance: _,
                            capture_clock: _,
                            end: _,
                            identity:
                                AutomationContentIdentity {
                                    version: _,
                                    file_bytes: _,
                                    terminal_chain: _,
                                },
                        },
                    mapping: _,
                    cursor_owner,
                },
        } = self;
        PreparedAutomationTakeDataLayout {
            inline_bytes: std::mem::size_of::<Self>(),
            path_capacity_bytes: path.capacity(),
            captured_tempo: captured_tempo.owned_allocation_layout(),
            inline_array_bytes: 2 * std::mem::size_of::<[u8; 32]>(),
            source_owner: AutomationSharedData {
                identity: Arc::as_ptr(cursor_owner) as usize,
                data_bytes: std::mem::size_of::<()>(),
                handles: 1,
            },
        }
    }
}
impl TakeReader for PreparedRecordSource {
    fn captured_tempo_map(&self) -> &TempoMap {
        &self.captured_tempo
    }
    fn target(&self) -> DraftTarget {
        self.target
    }
    fn provenance(&self) -> DraftProvenance {
        self.provenance
    }
    fn capture_clock(&self) -> Option<CaptureClock> {
        self.capture_clock
    }
    fn end_clock(&self) -> DraftClock {
        self.end
    }
    fn content_identity(&self) -> AutomationContentIdentity {
        self.identity
    }
    fn event_count(&self) -> u64 {
        self.records.progress.durable_events
    }
    fn path(&self) -> &Path {
        &self.records.path
    }
    fn read_events(
        &mut self,
        first: u64,
        output: &mut [Option<AppliedEvent>],
    ) -> Result<usize, DraftError> {
        self.records.read_events(first, output)
    }
    fn event_with_ordinal_at_or_before(
        &mut self,
        performance: u64,
    ) -> Result<Option<(u64, AppliedEvent)>, DraftError> {
        self.records.event_with_ordinal_at_or_before(performance)
    }
}

#[cfg(test)]
mod tests;
