//! Actual pending/queued Event metadata children. Enclosing Event storage,
//! views, runtime/input owners and durable read obligations are separate.
use super::{BodyRef, VecRef};
use crate::{AudioRootStorageLedger, AudioStorageError};
use std::path::PathBuf;

pub struct EventReasonFields<'a> {
    pub body: BodyRef<'a>,
    pub reason: Option<&'a String>,
}
pub struct EventBatchFailureFields<'a> {
    pub body: BodyRef<'a>,
    pub path: &'a PathBuf,
    pub explanation: &'a String,
}
pub struct TuningSlotMetadataFields<'a> {
    pub body: BodyRef<'a>,
    pub name: Option<&'a String>,
    pub error: Option<&'a String>,
}
/// One audited Session implementation. No list allocation, recursive ledger
/// callback, retained projection, supplied size or child-absence certificate.
pub trait TuningLibraryMetadataPayload {
    fn body(&self) -> BodyRef<'_>;
    fn slots(&self) -> VecRef<'_>;
    fn slot(&self, index: usize) -> Result<TuningSlotMetadataFields<'_>, AudioStorageError>;
    fn loaded_name(&self) -> Option<&String>;
    fn error(&self) -> Option<&String>;
}
pub struct EventMetadataFields<'a> {
    pub body: BodyRef<'a>,
    pub status: Option<&'a String>,
    pub error: Option<&'a String>,
    pub batch_failure: Option<EventBatchFailureFields<'a>>,
    pub device_ack: Option<EventReasonFields<'a>>,
    pub tuning_ack: Option<EventReasonFields<'a>>,
    pub tuning_library: Option<&'a dyn TuningLibraryMetadataPayload>,
}
fn contained(parent: BodyRef<'_>, child: BodyRef<'_>) -> Result<(), AudioStorageError> {
    if parent.contains(child) {
        Ok(())
    } else {
        Err(AudioStorageError::SourceMismatch)
    }
}
fn string<'a>(
    parent: BodyRef<'a>,
    value: Option<&'a String>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    if let Some(value) = value {
        contained(parent, BodyRef::of(value))?;
        ledger.string(value)?;
    }
    Ok(())
}
fn path<'a>(
    parent: BodyRef<'a>,
    value: &'a PathBuf,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    contained(parent, BodyRef::of(value))?;
    ledger.path_backing(value)
}
pub(crate) fn visit_event_metadata<'a>(
    fields: &EventMetadataFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    string(fields.body, fields.status, ledger)?;
    string(fields.body, fields.error, ledger)?;
    if let Some(failure) = &fields.batch_failure {
        contained(fields.body, failure.body)?;
        path(failure.body, failure.path, ledger)?;
        string(failure.body, Some(failure.explanation), ledger)?;
    }
    for ack in [&fields.device_ack, &fields.tuning_ack]
        .into_iter()
        .flatten()
    {
        contained(fields.body, ack.body)?;
        string(ack.body, ack.reason, ledger)?;
    }
    if let Some(library) = fields.tuning_library {
        let body = library.body();
        contained(fields.body, body)?;
        let slots = library.slots();
        contained(body, slots.body())?;
        slots.observe(ledger)?; // capacity includes spare inline Slot bodies ONCE
        for index in 0..slots.len() {
            let slot = library.slot(index)?;
            if !slots.element(index)?.same(slot.body) {
                return Err(AudioStorageError::SourceMismatch);
            }
            string(slot.body, slot.name, ledger)?;
            string(slot.body, slot.error, ledger)?;
        }
        string(body, library.loaded_name(), ledger)?;
        string(body, library.error(), ledger)?;
    }
    Ok(())
}
