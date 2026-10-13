//! Bounded source evidence retained independently of subsequent project edits.
use crate::{Invalid, Project};
use std::{fmt, sync::Arc};

pub const MAX_IMPORT_DOCUMENT_BYTES: usize = 1024 * 1024;

/// Exact original interchange bytes, not an encoding of the edited project.
/// The model does not interpret XML, access source paths or load plugins.
#[derive(Clone, PartialEq, Eq)]
pub struct ImportProvenance {
    adapter_version: u32,
    original_bytes: Arc<[u8]>,
}
impl ImportProvenance {
    /// Validate an envelope before retaining its allocation. Version 1 supports
    /// only the Live-set source kind and adapter contract version 1.
    pub fn new(
        kind: &str,
        version: u32,
        adapter_version: u32,
        original_bytes: Vec<u8>,
    ) -> Result<Self, Invalid> {
        if kind != "ableton-live-set" || version != 1 || adapter_version != 1 {
            return Err(Invalid("unsupported import provenance kind or version"));
        }
        if original_bytes.is_empty() || original_bytes.len() > MAX_IMPORT_DOCUMENT_BYTES {
            return Err(Invalid("import document must contain 1..1048576 bytes"));
        }
        Ok(Self {
            adapter_version,
            original_bytes: original_bytes.into(),
        })
    }
    pub fn kind(&self) -> &'static str {
        "ableton-live-set"
    }
    pub fn version(&self) -> u32 {
        1
    }
    pub fn adapter_version(&self) -> u32 {
        self.adapter_version
    }
    /// Unchanged-input pass-through only. These bytes never reflect Hex edits.
    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }
}
impl fmt::Debug for ImportProvenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportProvenance")
            .field("kind", &self.kind())
            .field("version", &self.version())
            .field("adapter_version", &self.adapter_version)
            .field("byte_count", &self.original_bytes.len())
            .finish()
    }
}
impl Project {
    pub fn import_provenance(&self) -> Option<&ImportProvenance> {
        self.import_provenance.as_ref()
    }
}
