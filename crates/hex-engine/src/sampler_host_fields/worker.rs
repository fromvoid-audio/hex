//! Same-dispatch borrowed worker descendants. Legacy outer roots remain unknown.
use super::SnapshotViewPayload;
use crate::{AudioRootStorageLedger, AudioStorageError, SharedMidiArrangementPreview};
use std::path::PathBuf;

/// Finite references to the actual dispatch locals, built only by its audited
/// owning module. No list allocation, owner clone, ledger callback or saved
/// completeness flag. These child borrows do not qualify legacy outer Arcs,
/// channel backing, gesture/runtime/journal owners or the enclosing worker.
/// ```compile_fail
/// fn escape<'a>(fields: hex_engine::sampler_host_fields::SamplerWorkerFields<'a>)
///     -> hex_engine::sampler_host_fields::SamplerWorkerFields<'static> { fields }
/// ```
pub struct SamplerWorkerFields<'a> {
    pub current: &'a dyn SnapshotViewPayload,
    pub event: Option<&'a dyn SnapshotViewPayload>,
    pub device_preview: Option<&'a dyn SnapshotViewPayload>,
    pub tuning_preview: Option<&'a dyn SnapshotViewPayload>,
    pub midi_preview: &'a SharedMidiArrangementPreview,
    pub media_base: Option<&'a PathBuf>,
    pub recovery: Option<&'a PathBuf>,
    pub export_path: Option<&'a PathBuf>,
    pub export_completion_path: Option<&'a PathBuf>,
}
pub(crate) fn visit_worker<'a>(
    fields: &SamplerWorkerFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    for view in [
        Some(fields.current),
        fields.event,
        fields.device_preview,
        fields.tuning_preview,
    ]
    .into_iter()
    .flatten()
    {
        super::snapshot_view::visit_snapshot_children(view, ledger)?;
    }
    crate::midi_preview::visit_shared_midi_preview_storage_on_control_inner(
        fields.midi_preview,
        ledger,
    )?;
    for path in [
        fields.media_base,
        fields.recovery,
        fields.export_path,
        fields.export_completion_path,
    ]
    .into_iter()
    .flatten()
    {
        ledger.path_backing(path)?;
    }
    Ok(())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Selected dispatch children only. Actual Session/cache/cancellation and
    /// all enclosing/native/family/legacy roots remain mandatory separately.
    pub fn borrow_sampler_worker_fields_on_control(
        &mut self,
        fields: &SamplerWorkerFields<'a>,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_worker(fields, ledger))
    }
}
