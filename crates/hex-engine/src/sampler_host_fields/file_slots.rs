use super::{BodyRef, VecRef};
use crate::{AudioRootStorageLedger, AudioStorageError, Frame};
use std::{
    fs::{File, Metadata},
    path::PathBuf,
};

/// Audited owner implementation, not a public admission certificate. Every
/// occupied actual Vec index must expose its exhaustive supported children.
pub trait FileSlotsFields<'a> {
    fn slots(&self) -> VecRef<'a>;
    fn file(&self, index: usize) -> Result<FileJobFields<'a>, AudioStorageError>;
}
pub struct FileJobFields<'a> {
    pub body: BodyRef<'a>,
    pub copy: &'a [Frame; 256],
    pub staged: StagedFileFields<'a>,
}
pub struct StagedFileFields<'a> {
    pub body: BodyRef<'a>,
    pub writer: WavWriterFields<'a>,
    pub destination: &'a PathBuf,
    pub temporary: &'a PathBuf,
    pub publication: PublicationFields<'a>,
}
pub struct WavWriterFields<'a> {
    pub body: BodyRef<'a>,
    pub file: &'a File,
    pub encoded: &'a [u8; 2048],
}
pub enum PublicationFields<'a> {
    CreateNew,
    Replace {
        body: BodyRef<'a>,
        path: &'a PathBuf,
        metadata: &'a Metadata,
    },
}
fn contained(parent: BodyRef<'_>, child: BodyRef<'_>) -> Result<(), AudioStorageError> {
    if parent.contains(child) {
        Ok(())
    } else {
        Err(AudioStorageError::SourceMismatch)
    }
}
pub(crate) fn visit_file_slots<'a>(
    fields: &dyn FileSlotsFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    let slots = fields.slots();
    slots.observe(ledger)?; // includes ALL spare element bodies; no embedded recount
    for index in 0..slots.len() {
        let slot = slots.element(index)?;
        let FileJobFields { body, copy, staged } = fields.file(index)?;
        if !slot.same(body) {
            return Err(AudioStorageError::SourceMismatch);
        }
        contained(body, BodyRef::of(copy))?;
        let StagedFileFields {
            body: staged_body,
            writer,
            destination,
            temporary,
            publication,
        } = staged;
        contained(body, staged_body)?;
        contained(staged_body, BodyRef::of(destination))?;
        contained(staged_body, BodyRef::of(temporary))?;
        let WavWriterFields {
            body: writer_body,
            file,
            encoded,
        } = writer;
        contained(staged_body, writer_body)?;
        contained(writer_body, BodyRef::of(file))?;
        contained(writer_body, BodyRef::of(encoded))?;
        ledger.path_backing(destination)?;
        ledger.path_backing(temporary)?;
        match publication {
            PublicationFields::CreateNew => {}
            PublicationFields::Replace {
                body: consent,
                path,
                metadata,
            } => {
                contained(staged_body, consent)?;
                contained(consent, BodyRef::of(path))?;
                contained(consent, BodyRef::of(metadata))?;
                ledger.path_backing(path)?;
            }
        }
    }
    Ok(())
}
impl<'a> AudioRootStorageLedger<'a> {
    /// Selected actual File slots only, not a complete worker/constructor scope.
    pub fn borrow_sampler_file_slots_fields_on_control(
        &mut self,
        fields: &dyn FileSlotsFields<'a>,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_file_slots(fields, ledger))
    }
}
