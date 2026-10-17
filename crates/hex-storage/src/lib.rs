//! Versioned, bounded persistence of canonical metadata; no media or callback work.
mod files;
mod journal;
pub mod preset;
pub use journal::{Journal, journal_path, load_journal};
mod audio_edit;
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
/// Visible checkpoint state when releasing journal ownership failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalChange {
    Unchanged,
    CheckpointPublished,
    CheckpointCleared,
}

#[derive(Debug)]
pub enum StorageError {
    Io(io::Error),
    Json(serde_json::Error),
    /// The operation's visible state is retained; lock cleanup failed.
    JournalLockRelease {
        path: PathBuf,
        change: JournalChange,
        operation_error: Option<Box<StorageError>>,
        source: io::Error,
    },
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
            Self::JournalLockRelease {
                path,
                change,
                operation_error,
                source,
            } => {
                let state = match change {
                    JournalChange::Unchanged => "checkpoint unchanged",
                    JournalChange::CheckpointPublished => "checkpoint published",
                    JournalChange::CheckpointCleared => "checkpoint cleared",
                };
                write!(
                    f,
                    "{state} at {}; journal lock release failed: {source}",
                    path.display()
                )?;
                if let Some(error) = operation_error {
                    write!(f, "; operation reported: {error}")?;
                }
                Ok(())
            }
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
            Self::JournalLockRelease { source, .. } => Some(source),
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

/// Encode the complete schema27 envelope, preserving device host gains, import
/// provenance, typed routing and complete gain-lane metadata together.
/// Historical readers remain explicit.
pub fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    encode_document(&schema::appearance::DocumentV27::from_project(project)?)
}

/// Explicit schema22 encoding; newer playable routing is refused.
pub fn encode_schema22(project: &Project) -> Result<Vec<u8>, StorageError> {
    refuse_legacy_routes(project)?;
    encode_document(&schema::gain_touch::DocumentV22::from_project(project)?)
}

/// Explicit schema20 encoding, including empty-feed projects.
pub fn encode_schema20(project: &Project) -> Result<Vec<u8>, StorageError> {
    refuse_legacy_routes(project)?;
    encode_document(&schema::return_feeds::DocumentV20::from_project(project)?)
}

fn refuse_legacy_routes(project: &Project) -> Result<(), StorageError> {
    if project.tracks().iter().any(|t| {
        t.clips().iter().any(|c| c.name() != "clip")
            || t.midi_clips().iter().any(|c| c.name() != "clip")
    }) {
        return Err(StorageError::Invalid(
            "older schema cannot save arrangement clip names",
        ));
    }
    schema::appearance::refuse_legacy(project)?;
    schema::musical_audio::refuse_legacy_audio(project)?;
    if !project.audio_feeds().is_empty() || !project.midi_routes().is_empty() {
        return Err(StorageError::Invalid(
            "older schema cannot save playable routing",
        ));
    }
    Ok(())
}

fn encode_document(document: &impl serde::Serialize) -> Result<Vec<u8>, StorageError> {
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
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        exceeded: false,
    };
    let result = serde_json::to_writer(&mut output, document);
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
