//! The audited private Storage FileToken family only. No raw-Arc adoption,
//! input construction peak, final-drop receipt or native admission is inferred.
use super::*;
use crate::sampler_host_fields::{BodyRef, DocumentFields};
use std::{alloc::Layout, sync::atomic::AtomicUsize};

pub fn document_file_token_storage_layout_supported() -> bool {
    let target = env!("HEX_DOCUMENT_TOKEN_ARC_LAYOUT_TARGET");
    env!("HEX_DOCUMENT_TOKEN_ARC_LAYOUT_KNOWN") == "1"
        && env!("HEX_DOCUMENT_TOKEN_ARC_CONSTRUCTOR_FAMILY")
            == "storage-file-token-vec-and-copy-slice-arc-v1"
        && ((cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )) && target == "x86_64-unknown-linux-gnu")
            || (cfg!(all(target_arch = "aarch64", target_os = "macos"))
                && target == "aarch64-apple-darwin"))
}

fn parts(length: usize, supported: bool) -> Result<(usize, usize), AudioStorageError> {
    if !supported {
        return Err(AudioStorageError::UnknownStorage);
    }
    // Pinned ArcInner header then ACTUAL u8 slice, padded by its actual layout.
    // The old Vec capacity/recovery input is not retained in the Arc; that
    // simultaneous birth overlap must be quoted at its real constructor.
    let requested = Layout::array::<AtomicUsize>(2)
        .map_err(|_| AudioStorageError::Overflow)?
        .extend(Layout::array::<u8>(length).map_err(|_| AudioStorageError::Overflow)?)
        .map_err(|_| AudioStorageError::Overflow)?
        .0
        .pad_to_align()
        .size();
    Ok((
        length,
        requested
            .checked_sub(length)
            .ok_or(AudioStorageError::Overflow)?,
    ))
}

pub(crate) fn visit_document<'a>(
    document: &DocumentFields<'a>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    // Actual enclosing FIELD and its actual private Arc FIELD, not equal bytes
    // or a copied handle/size. A foreign field invalidates the entire traversal.
    if !document.body.contains(BodyRef::of(document.path))
        || !document.body.contains(document.token.body)
        || !document
            .token
            .body
            .contains(BodyRef::of(document.token.contents))
    {
        return Err(AudioStorageError::SourceMismatch);
    }
    let parts = parts(
        document.token.contents.len(),
        document_file_token_storage_layout_supported(),
    )?;
    ledger.path_backing(document.path)?;
    ledger.sampler_shared_field(
        document.token.contents,
        Some(parts),
        BodyRef::of(document.token.contents).owner(),
    )
}

impl<'a> AudioRootStorageLedger<'a> {
    /// Internal finite DocumentHandle fields only. Normal/bootstrap use the
    /// same mandatory visitor. Enclosing worker/scratch, original inputs and
    /// every actual token alias remain separate; this grants no root closure.
    pub fn borrow_sampler_document_fields_on_control(
        &mut self,
        fields: &DocumentFields<'a>,
    ) -> Result<(), AudioStorageError> {
        self.mutate(|ledger| visit_document(fields, ledger))
    }
}

#[cfg(test)]
use crate::pcm_arc::build_gate_tests as pcm_gate;
#[cfg(test)]
#[path = "../../build_support/document_token_gate.rs"]
mod document_token_gate;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler_host_fields::DocumentTokenFields;
    struct Slot {
        path: std::path::PathBuf,
        contents: Arc<[u8]>,
    }
    impl Slot {
        fn fields(&self) -> DocumentFields<'_> {
            DocumentFields {
                body: BodyRef::of(self),
                path: &self.path,
                token: DocumentTokenFields {
                    body: BodyRef::of(&self.contents),
                    contents: &self.contents,
                },
            }
        }
    }
    fn limits() -> AudioStorageLedgerReservation {
        AudioStorageLedgerReservation {
            data_rows: 32,
            handle_rows: 16,
            root_rows: 4,
            max_retained_bytes: 1_000_000,
            max_construction_peak_bytes: 1_000_000,
        }
    }
    #[test]
    fn document_token_layout_padding_unknown_and_overflow() {
        for (length, requested) in [(0, 16), (1, 24), (8, 24), (9, 32), (4096, 4112)] {
            let (payload, overhead) = parts(length, true).unwrap();
            assert_eq!(payload + overhead, requested);
        }
        assert_eq!(parts(7, false), Err(AudioStorageError::UnknownStorage));
        assert_eq!(parts(usize::MAX, true), Err(AudioStorageError::Overflow));
    }
    #[test]
    fn document_token_actual_fields_alias_omission_dedup_and_independent_equal_bytes() {
        assert!(
            document_file_token_storage_layout_supported(),
            "unsupported FileToken family: no positive native component claim"
        );
        // Synthetic field fixtures exercise the Engine visitor, not nominal
        // Storage origin. Real four-constructor paths are tested in Session.
        let first = Slot {
            path: "A.hex".into(),
            contents: Arc::from(vec![7_u8; 9]),
        };
        let alias = Slot {
            path: "B.hex".into(),
            contents: first.contents.clone(),
        };
        let separate = Slot {
            path: "C.hex".into(),
            contents: Arc::from(&[7_u8; 9][..]),
        };
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        ledger
            .borrow_sampler_document_fields_on_control(&first.fields())
            .unwrap();
        assert!(
            ledger
                .validate_selected_shared_handles_on_control()
                .is_err()
        );
        ledger
            .borrow_sampler_document_fields_on_control(&alias.fields())
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        let paired = ledger.summary().unwrap();
        ledger
            .borrow_sampler_document_fields_on_control(&first.fields())
            .unwrap();
        assert_eq!(ledger.summary().unwrap(), paired);
        ledger
            .borrow_sampler_document_fields_on_control(&separate.fields())
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        assert_eq!(
            ledger.summary().unwrap().data_bytes - paired.data_bytes,
            separate.path.capacity() + 32
        );
    }
    #[test]
    fn document_token_foreign_field_and_synthetic_weak_are_refused() {
        let first = Slot {
            path: "One.hex".into(),
            contents: Arc::from(vec![4_u8; 5]),
        };
        let foreign = Slot {
            path: "Two.hex".into(),
            contents: first.contents.clone(),
        };
        let mut fields = first.fields();
        fields.token.contents = &foreign.contents;
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        assert_eq!(
            ledger.borrow_sampler_document_fields_on_control(&fields),
            Err(AudioStorageError::SourceMismatch)
        );
        assert!(ledger.summary().is_err());
        // Weak control is synthetic only. No Weak ingress/getter is added to
        // actual Storage FileToken constructors.
        let _weak = Arc::downgrade(&first.contents);
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limits()).unwrap();
        assert_eq!(
            ledger.borrow_sampler_document_fields_on_control(&first.fields()),
            Err(AudioStorageError::UnknownStorage)
        );
    }
}
