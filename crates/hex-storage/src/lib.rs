//! Versioned, bounded persistence of canonical metadata; no media or callback work.
mod files;
mod replacement;
mod schema;
pub use files::{FileToken, load, load_with_token, save_new, save_new_with_token};
use hex_project::Project;
pub use replacement::{
    RecoveryDocument, ReplaceOutcome, ReplaceWarning, load_recovery, recovery_path,
    replace_existing,
};
use std::{fmt, io, path::PathBuf};

pub const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
#[derive(Debug)]
pub enum StorageError {
    Io(io::Error),
    Json(serde_json::Error),
    Invalid(&'static str),
    /// The document bytes differ from the snapshot captured when it was loaded.
    Conflict {
        path: PathBuf,
    },
    /// A complete destination exists, but cleanup or directory durability failed.
    PublishedButNotDurable {
        path: PathBuf,
        operation: &'static str,
        source: io::Error,
    },
    /// Recovery bytes were published, but the target replacement did not commit.
    RecoveryBackupPublished {
        path: PathBuf,
        recovery_path: PathBuf,
        operation: &'static str,
        source: io::Error,
    },
}
impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "project file error: {error}"),
            Self::Json(error) => write!(f, "invalid project json: {error}"),
            Self::Invalid(reason) => f.write_str(reason),
            Self::Conflict { path } => {
                write!(f, "project changed outside hex at {}", path.display())
            }
            Self::PublishedButNotDurable {
                path,
                operation,
                source,
            } => write!(
                f,
                "project was published at {} but could not {operation}: {source}",
                path.display()
            ),
            Self::RecoveryBackupPublished {
                path,
                recovery_path,
                operation,
                source,
            } => write!(
                f,
                "recovery copy for {} was published at {}, but could not {operation}: {source}",
                path.display(),
                recovery_path.display()
            ),
        }
    }
}
impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            Self::Invalid(_) | Self::Conflict { .. } => None,
            Self::PublishedButNotDurable { source, .. }
            | Self::RecoveryBackupPublished { source, .. } => Some(source),
        }
    }
}
impl From<io::Error> for StorageError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<serde_json::Error> for StorageError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

/// Encode schema 18 without allocating an oversized output document.
/// Compact JSON keeps a maximum-size provenance byte array within the existing
/// document ceiling; file tokens continue to compare exact saved bytes.
pub fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    struct BoundedOutput {
        bytes: Vec<u8>,
        exceeded: bool,
    }
    impl io::Write for BoundedOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > MAX_DOCUMENT_BYTES.saturating_sub(self.bytes.len()) {
                self.exceeded = true;
                return Err(io::Error::other("project document exceeds byte limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let document = schema::provenance::DocumentV18::from_project(project)?;
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        exceeded: false,
    };
    let result = serde_json::to_writer(&mut output, &document);
    if output.exceeded {
        return Err(StorageError::Invalid("project document exceeds byte limit"));
    }
    result?;
    Ok(output.bytes)
}
pub fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(StorageError::Invalid("project document exceeds byte limit"));
    }
    schema::DocumentV11::decode(bytes)
}
