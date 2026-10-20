//! Same-dispatch borrowed worker descendants. Legacy outer roots remain unknown.
use super::{BodyRef, SnapshotViewPayload};
use crate::{AudioRootStorageLedger, AudioStorageError, SharedMidiArrangementPreview};
use std::path::PathBuf;
use std::sync::Arc;

/// Actual fields from the ONE audited Session DocumentHandle adapter and the
/// nominal private Storage FileToken observation. Neither BodyRef nor counts
/// qualifies a foreign Arc; the closed producer source and separate pinned
/// FileToken constructor family are mandatory.
pub struct DocumentTokenFields<'a> {
    pub body: BodyRef<'a>,
    pub contents: &'a Arc<[u8]>,
}
pub struct DocumentFields<'a> {
    pub body: BodyRef<'a>,
    pub path: &'a PathBuf,
    pub token: DocumentTokenFields<'a>,
}

/// Actual retained Journal fields; bytes are typed u8 storage, not a generic
/// vector whose owning elements could be omitted. Storage's audited borrowed
/// accessor exposes these exact private fields without an Engine dependency.
pub struct JournalFields<'a> {
    pub body: BodyRef<'a>,
    pub path: &'a PathBuf,
    pub bytes: Option<&'a Vec<u8>>,
}

/// The ACTUAL owning current-view field. Selected children cannot qualify a
/// legacy Arc. Only a newly paired registered constructor can supply Qualified.
pub enum WorkerCurrentOwnerFields<'a> {
    Unqualified(BodyRef<'a>),
    Qualified(super::SnapshotViewFields<'a>),
}

/// Actual legacy owning input fields. Presence must refuse until each real
/// constructor/retirement route qualifies it; a selected child walk cannot
/// turn these raw Arc owners into paired metadata.
pub struct WorkerLegacyInputFields<'a> {
    pub clipboard: Option<BodyRef<'a>>,
    pub keyboard: Option<BodyRef<'a>>,
    pub keyboard_hint: Option<BodyRef<'a>>,
    pub audition: Option<BodyRef<'a>>,
    pub expression: Option<BodyRef<'a>>,
}

/// Footprint of the actual two worker-local gesture machines. The owning
/// Session producers exhaustively match and Copy-check every private field;
/// BodyRef alone never certifies the absence of owning children.
pub struct WorkerInlineGestureFields<'a> {
    pub device: BodyRef<'a>,
    pub tuning: BodyRef<'a>,
}
fn visit_journal<'a>(
    journal: &JournalFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    if !journal.body.contains(BodyRef::of(journal.path))
        || journal
            .bytes
            .is_some_and(|bytes| !journal.body.contains(BodyRef::of(bytes)))
    {
        return Err(AudioStorageError::SourceMismatch);
    }
    ledger.path_backing(journal.path)?;
    if let Some(bytes) = journal.bytes {
        ledger.vec(bytes)?;
    }
    Ok(())
}

/// Finite references to the actual dispatch locals, built only by its audited
/// owning module. No list allocation, owner clone, ledger callback or saved
/// completeness flag. These child borrows do not qualify legacy outer Arcs,
/// channel backing, gesture/runtime owners or the enclosing worker.
/// ```compile_fail
/// fn escape<'a>(fields: hex_engine::sampler_host_fields::SamplerWorkerFields<'a>)
///     -> hex_engine::sampler_host_fields::SamplerWorkerFields<'static> { fields }
/// ```
pub struct SamplerWorkerFields<'a> {
    pub current_owner: WorkerCurrentOwnerFields<'a>,
    pub legacy_inputs: WorkerLegacyInputFields<'a>,
    pub gestures: WorkerInlineGestureFields<'a>,
    /// Actual unsupported legacy owning fields, never an absence/retirement
    /// certificate. The audited producer must retain their real presence even
    /// for empty revisions. A qualified constructor must replace these owners
    /// at birth before the closed native path can admit them.
    pub unqualified_compiled: Option<BodyRef<'a>>,
    pub unqualified_playback: Option<BodyRef<'a>>,
    pub event_metadata: Option<super::EventMetadataFields<'a>>,
    pub current: &'a dyn SnapshotViewPayload,
    pub event: Option<&'a dyn SnapshotViewPayload>,
    pub device_preview: Option<&'a dyn SnapshotViewPayload>,
    pub tuning_preview: Option<&'a dyn SnapshotViewPayload>,
    pub midi_preview: &'a SharedMidiArrangementPreview,
    pub media_base: Option<&'a PathBuf>,
    pub recovery: Option<&'a PathBuf>,
    pub export_path: Option<&'a PathBuf>,
    pub export_completion: Option<super::ExportResultPayloadFields<'a>>,
    pub journal: Option<JournalFields<'a>>,
    pub document: Option<DocumentFields<'a>>,
}
impl SamplerWorkerFields<'_> {
    pub(crate) fn require_current_constructor_owner(&self) -> Result<(), AudioStorageError> {
        match &self.current_owner {
            WorkerCurrentOwnerFields::Unqualified(_) => Err(AudioStorageError::UnknownStorage),
            WorkerCurrentOwnerFields::Qualified(fields)
                if fields.owner.payload().same(self.current.body())
                    && fields.payload.body().same(self.current.body()) =>
            {
                Ok(())
            }
            WorkerCurrentOwnerFields::Qualified(_) => Err(AudioStorageError::SourceMismatch),
        }
    }
    pub(crate) fn visit_current_constructor_owner<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        self.require_current_constructor_owner()?;
        if let WorkerCurrentOwnerFields::Qualified(fields) = &self.current_owner {
            fields.owner.observe(ledger)?;
        }
        Ok(())
    }
}
pub(crate) fn visit_worker<'a>(
    fields: &SamplerWorkerFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    // BodyRef cannot qualify any reachable children of a legacy owner. Refuse
    // BEFORE descendant traversal, independently of incidental shared aliases.
    let WorkerLegacyInputFields {
        clipboard,
        keyboard,
        keyboard_hint,
        audition,
        expression,
    } = &fields.legacy_inputs;
    if fields.unqualified_compiled.is_some()
        || fields.unqualified_playback.is_some()
        || [clipboard, keyboard, keyboard_hint, audition, expression]
            .iter()
            .any(|owner| owner.is_some())
    {
        return Err(AudioStorageError::UnknownStorage);
    }
    for frame in [fields.gestures.device, fields.gestures.tuning] {
        ledger.bytes(
            frame.owner(),
            crate::audio_storage::Location("worker-inline-gesture", 0),
            frame.layout_bytes(),
        )?;
    }
    if let Some(metadata) = &fields.event_metadata {
        super::event::visit_event_metadata(metadata, ledger)?;
    }
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
    for path in [fields.media_base, fields.recovery, fields.export_path]
        .into_iter()
        .flatten()
    {
        ledger.path_backing(path)?;
    }
    if let Some(completion) = &fields.export_completion {
        // Enclosing dispatch/frame accounting remains separate. Visit EVERY
        // actual immutable completion child, not just its destination path.
        super::family::visit_export_result_payload(completion, ledger)?;
    }
    if let Some(journal) = &fields.journal {
        visit_journal(journal, ledger)?;
    }
    if let Some(document) = &fields.document {
        crate::audio_storage::document_token::visit_document(document, ledger)?;
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

#[cfg(test)]
mod journal_tests {
    use super::*;
    use crate::AudioStorageLedgerReservation;

    struct Owner {
        path: PathBuf,
        bytes: Vec<u8>,
    }
    fn limits() -> AudioStorageLedgerReservation {
        AudioStorageLedgerReservation {
            data_rows: 16,
            handle_rows: 8,
            root_rows: 4,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        }
    }
    fn fixture() -> Owner {
        let mut path = PathBuf::from("Case/Current.hex-journal");
        path.reserve(317);
        let mut bytes = Vec::with_capacity(1379);
        bytes.extend_from_slice(b"actual nonempty checkpoint bytes");
        Owner { path, bytes }
    }
    #[test]
    fn journal_fields_charge_actual_capacities_dedup_and_both_limits() {
        let owner = fixture();
        let fields = JournalFields {
            body: BodyRef::of(&owner),
            path: &owner.path,
            bytes: Some(&owner.bytes),
        };
        let collect = |reservation| {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(reservation)?;
            ledger.mutate(|ledger| visit_journal(&fields, ledger))?;
            let first = ledger.summary()?;
            ledger.mutate(|ledger| visit_journal(&fields, ledger))?;
            assert_eq!(ledger.summary()?, first);
            Ok::<_, AudioStorageError>(first)
        };
        let measured = collect(limits()).unwrap();
        assert_eq!(
            measured.data_bytes,
            owner.path.capacity() + owner.bytes.capacity()
        );
        assert_eq!(measured.data_allocations, 2);
        let mut exact = limits();
        exact.max_retained_bytes = measured.retained_bytes;
        exact.max_construction_peak_bytes = measured.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), measured);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
        exact.max_retained_bytes += 1;
        exact.max_construction_peak_bytes -= 1;
        assert!(collect(exact).is_err());
    }
    #[test]
    fn journal_foreign_path_or_checkpoint_refuses_and_invalidates_partial_ledger() {
        let owner = fixture();
        let foreign = fixture();
        for fields in [
            JournalFields {
                body: BodyRef::of(&owner),
                path: &foreign.path,
                bytes: Some(&owner.bytes),
            },
            JournalFields {
                body: BodyRef::of(&owner),
                path: &owner.path,
                bytes: Some(&foreign.bytes),
            },
        ] {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
            assert_eq!(
                ledger.mutate(|ledger| visit_journal(&fields, ledger)),
                Err(AudioStorageError::SourceMismatch)
            );
            assert!(ledger.summary().is_err());
        }
    }
}
