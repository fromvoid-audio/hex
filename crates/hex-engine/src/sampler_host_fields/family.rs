//! Fixed Session-family record schema, under its existing structural exclusion.
use super::{BodyRef, SnapshotViewFields, VecRef};
use crate::{AudioCompiledRevision, AudioRootStorageLedger, AudioStorageError};
use std::path::PathBuf;

pub struct FamilySnapshotFields<'a> {
    pub body: BodyRef<'a>,
    pub view: Option<SnapshotViewFields<'a>>,
}
pub struct FamilyCompiledRecordFields<'a> {
    pub body: BodyRef<'a>,
    pub reader: Option<&'a AudioCompiledRevision>,
}
/// The actual existing fixed pair, not a caller count or projection Vec.
pub struct FamilyCompiledFields<'a> {
    pub body: BodyRef<'a>,
    pub first: FamilyCompiledRecordFields<'a>,
    pub second: FamilyCompiledRecordFields<'a>,
}
pub enum FamilyRequestPayload<'a> {
    Inline {
        body: BodyRef<'a>,
    },
    Path {
        body: BodyRef<'a>,
        path: &'a PathBuf,
    },
    ClipIds {
        body: BodyRef<'a>,
        ids: &'a Vec<hex_project::ClipId>,
    },
    TimeRange {
        body: BodyRef<'a>,
        range: &'a hex_project::TimeRange,
    },
}
pub struct FamilyRequestFields<'a> {
    pub body: BodyRef<'a>,
    pub request: Option<FamilyRequestPayload<'a>>,
}
pub trait FamilyRequestRecords {
    fn body(&self) -> BodyRef<'_>;
    fn slots(&self) -> VecRef<'_>;
    /// The owner validates the actual per-record phase/cleanup/completion.
    /// A still-claimed unrepresented input must refuse rather than be None.
    fn request(&self, index: usize) -> Result<FamilyRequestFields<'_>, AudioStorageError>;
}
pub trait FamilyFields {
    fn body(&self) -> BodyRef<'_>;
    fn snapshots(&self) -> VecRef<'_>;
    fn snapshot(&self, index: usize) -> Result<FamilySnapshotFields<'_>, AudioStorageError>;
    fn reads(&self) -> VecRef<'_>;
    fn requests(&self) -> &dyn FamilyRequestRecords;
    fn compiled(&self) -> Result<Option<FamilyCompiledFields<'_>>, AudioStorageError>;
}
fn contained(parent: BodyRef<'_>, child: BodyRef<'_>) -> Result<(), AudioStorageError> {
    if parent.contains(child) {
        Ok(())
    } else {
        Err(AudioStorageError::SourceMismatch)
    }
}
pub(crate) fn visit_compiled<'a>(
    fields: &FamilyCompiledFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    for record in [&fields.first, &fields.second] {
        contained(fields.body, record.body)?;
        if let Some(reader) = record.reader {
            contained(record.body, BodyRef::of(reader))?;
            reader.visit_shared_storage_on_control(ledger)?;
        }
    }
    Ok(())
}
pub(crate) fn visit_requests<'a>(
    fields: &'a dyn FamilyRequestRecords,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let slots = fields.slots();
    contained(fields.body(), slots.body())?;
    slots.observe(ledger)?;
    for index in 0..slots.len() {
        let record = fields.request(index)?;
        if !slots.element(index)?.same(record.body) {
            return Err(AudioStorageError::SourceMismatch);
        }
        if let Some(request) = record.request {
            match request {
                FamilyRequestPayload::Inline { body } => contained(record.body, body)?,
                FamilyRequestPayload::Path { body, path } => {
                    contained(record.body, body)?;
                    contained(body, BodyRef::of(path))?;
                    ledger.path_backing(path)?;
                }
                FamilyRequestPayload::ClipIds { body, ids } => {
                    contained(record.body, body)?;
                    contained(body, BodyRef::of(ids))?;
                    ledger.vec(ids)?;
                }
                FamilyRequestPayload::TimeRange { body, range } => {
                    contained(record.body, body)?;
                    contained(body, BodyRef::of(range))?;
                    let tracks = range.track_buffer_on_control();
                    contained(BodyRef::of(range), BodyRef::of(tracks))?;
                    // Copy TrackId leaves; the request slot already owns the header.
                    ledger.vec(tracks)?;
                }
            }
        }
    }
    Ok(())
}
pub(crate) fn visit_family<'a>(
    fields: &'a dyn FamilyFields,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let body = fields.body();
    let snapshots = fields.snapshots();
    let reads = fields.reads();
    contained(body, snapshots.body())?;
    contained(body, reads.body())?;
    snapshots.observe(ledger)?;
    reads.observe(ledger)?;
    let requests = fields.requests();
    contained(body, requests.body())?;
    visit_requests(requests, ledger)?;
    if let Some(compiled) = fields.compiled()? {
        contained(body, compiled.body)?;
        visit_compiled(&compiled, ledger)?;
    }
    for index in 0..snapshots.len() {
        let record = fields.snapshot(index)?;
        if !snapshots.element(index)?.same(record.body) {
            return Err(AudioStorageError::SourceMismatch);
        }
        if let Some(view) = record.view {
            contained(record.body, view.owner.body())?;
            if !view.owner.payload().same(view.payload.body()) {
                return Err(AudioStorageError::SourceMismatch);
            }
            view.owner.observe(ledger)?;
            super::snapshot_view::visit_snapshot_children(view.payload, ledger)?;
        }
    }
    Ok(())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Actual record descendants only. The enclosing retained family Box/guard,
    /// other live roots, construction peak and retirement remain mandatory.
    pub fn borrow_sampler_family_fields_on_control(
        &mut self,
        fields: &'a dyn FamilyFields,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_family(fields, ledger))
    }
    pub fn borrow_sampler_family_requests_on_control(
        &mut self,
        fields: &'a dyn FamilyRequestRecords,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_requests(fields, ledger))
    }
    pub fn borrow_sampler_family_compiled_on_control(
        &mut self,
        fields: &FamilyCompiledFields<'a>,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_compiled(fields, ledger))
    }
}
