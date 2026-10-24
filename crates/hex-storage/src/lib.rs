//! # Purpose
//! Own strict bounded serialization and recoverable publication of canonical project metadata.
//!
//! # Dependencies
//! Uses hex-project, hex-tuning, hex-lattice-contract, serde/serde_json and fs2.
//!
//! # Does not own
//! Does not decode PCM, construct DSP, access callback state or silently convert unsupported
//! formats. Unsupported/malformed input refuses without rewriting the original file.
mod files;
mod journal;
pub mod preset;
pub mod tuning_library;
pub use journal::{
    Journal, JournalRestartOutcome, JournalRestartToken, journal_path, load_journal,
    restart_journal,
};
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

/// Current saved-project schema; independent of the application version.
pub const CURRENT_SCHEMA_VERSION: u32 = 39;

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

/// Encode schema39 with required MIDI one-shot policies, fade positions, tuning and device asset
/// and MIDI clip-volume inventories, preserving all active project capabilities.
pub fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    encode_document(&schema::midi_one_shot::encode(project)?)
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
    if project.tracks().iter().flat_map(|track| track.midi_clips()).any(|clip| clip.source().one_shot_boundary() != hex_project::MidiOneShotBoundary::SourceEnd)
        || project.session_cells().iter().any(|cell| matches!(cell.content(), hex_project::SessionCellContent::Midi(clip) if clip.source().one_shot_boundary() != hex_project::MidiOneShotBoundary::SourceEnd)) {
        return Err(StorageError::Invalid("older schema cannot save midi one-shot boundaries"));
    }

    if !project.arrangement_sections().is_empty() {
        return Err(StorageError::Invalid(
            "older schema cannot save arrangement sections",
        ));
    }
    if project.tracks().iter().flat_map(|track| track.midi_clips()).any(|clip| !clip.volume().points().is_empty())
        || project.session_cells().iter().any(|cell| matches!(cell.content(), hex_project::SessionCellContent::Midi(clip) if !clip.volume().points().is_empty()))
    {
        return Err(StorageError::Invalid("older schema cannot save midi clip volume"));
    }
    if project
        .assets()
        .any(|asset| asset.original_source().is_some())
    {
        return Err(StorageError::Invalid(
            "older schema cannot save device-original source identities",
        ));
    }
    if !project.main_devices().is_empty() {
        return Err(StorageError::Invalid(
            "older schema cannot save main devices",
        ));
    }
    if project.tuning() != hex_tuning::ProjectTuning::default() {
        return Err(StorageError::Invalid(
            "older schema cannot save project tuning",
        ));
    }
    schema::device_assets::refuse_legacy(project)?;
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
/// Open current and evidenced historical schemas without file/media I/O.
/// Schemas1–33 select frozen readers and branch-aware adapters using the version
/// and complete field inventories. Only absent-by-design fields acquire their
/// original defaults; authored metadata is never discarded to accept a file.
/// Pre38 schemas default horizontal fade positions to center, preserving their
/// original sample-exact rational laws.
/// Schemas34/35 predate sections;34 also predates MIDI clip volume (unity).
/// Schema36 had independent tempo and sections branches, selected by presence of
/// the section inventory. Historical audio timing, projection/offset and gain
/// seam/source metadata migrate by those exact branch contracts; other fields
/// remain strict. No tempo-map collapse,
/// overlap repair or opaque device-state conversion is performed. Older projects
/// violating current model invariants refuse rather than lose musical data.
/// Newer/unknown fields, duplicate keys and malformed inventories refuse.
/// Saving uses the current schema through existing atomic publication/recovery.
pub fn decode(bytes: &[u8]) -> Result<Project, StorageError> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(StorageError::Invalid("project document exceeds byte limit"));
    }
    #[derive(serde::Deserialize)]
    struct CurrentHeader<'a> {
        format: &'a str,
        version: u32,
    }
    let header: CurrentHeader<'_> = serde_json::from_slice(bytes)?;
    if header.format != "hex-project" {
        return Err(StorageError::Invalid("unsupported project format"));
    }
    match header.version {
        CURRENT_SCHEMA_VERSION => schema::midi_one_shot::decode(bytes),
        38 => schema::fade_curve_positions::decode(bytes),
        37 => schema::DocumentV11::decode(bytes),
        1..=33 => schema::legacy::decode(bytes, header.version),
        34..=36 => schema::tuning::decode_version(bytes, header.version),
        version if version > CURRENT_SCHEMA_VERSION => Err(StorageError::Invalid(
            "project uses a newer schema; open it with a newer hex version; original file unchanged",
        )),
        _ => Err(StorageError::Invalid(
            "unsupported older project schema; open a copy with the hex build that saved it and retain the original for recovery",
        )),
    }
}
