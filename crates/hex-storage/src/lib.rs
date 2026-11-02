//! # Purpose
//! Own strict canonical project metadata serialization, recoverable publication and independent
//! source-bound automation recording drafts. Drafts are not canonical lane references.
//!
//! # Dependencies
//! Uses hex-project, hex-tuning, hex-lattice-contract, serde/serde_json, sha2 and fs2.
//!
//! # Does not own
//! Does not decode PCM, construct DSP, access callback state or silently convert unsupported
//! formats. Unsupported/malformed input refuses without rewriting the original file.
pub mod automation_draft;
mod files;
mod journal;
pub mod preset;
pub mod project_name;
pub mod text;
pub mod tuning_library;
pub use journal::{
    Journal, JournalRestartOutcome, JournalRestartToken, journal_path, load_journal,
    restart_journal,
};
mod audio_edit;
mod replacement;
mod schema;
mod temporaries;
pub use files::{
    FileToken, SaveNewOutcome, load, load_with_token, save_new, save_new_with_outcome,
    save_new_with_token,
};
use hex_project::Project;
pub use replacement::{
    RecoveryDocument, ReplaceOutcome, ReplaceWarning, load_recovery, recovery_path,
    replace_confirmed, replace_existing,
};
use std::{fmt, io, path::PathBuf};

/// Current saved-project schema; independent of the application version.
/// Schema63 records automatic track names; earlier names migrate as user-set.
/// Schema62 adds independent MIDI clip gain; schema61 migrates at unity.
/// Schema59 is a reserved incompatible development layout and remains unsupported.
pub const CURRENT_SCHEMA_VERSION: u32 = 63;

/// Older readers rejected documents above this size. This is compatibility
/// metadata for overwrite consent, never an admission limit for this reader.
pub const LEGACY_READER_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
/// Largest project document this build writes or opens. Valid projects stay far
/// below it: the largest contributors are names (tracks, scenes, clips, devices
/// at up to 1 KiB each, at most six JSON bytes per escaped byte), asset paths
/// and 65,536-entry note/point inventories. Asset paths have no separate model
/// bound, so this is a declared admission limit rather than a computed maximum.
/// Encoding refuses a larger document, so hex never saves a project it would
/// then refuse to open; reading refuses a wrong or sparse multi-gigabyte file
/// before allocating for it.
pub const MAX_PROJECT_DOCUMENT_BYTES: usize = 512 * 1024 * 1024;
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
                    crate::text::verbatim(path.display())
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
                write!(
                    f,
                    "project changed outside hex at {}",
                    crate::text::verbatim(path.display())
                )
            }
            Self::PublishedButNotDurable {
                path,
                operation,
                source,
            } => write!(
                f,
                "project was published at {} but could not {operation}: {source}",
                crate::text::verbatim(path.display())
            ),
            Self::RecoveryBackupPublished {
                path,
                recovery_path,
                operation,
                source,
            } => write!(
                f,
                "recovery copy for {} was published at {}, but could not {operation}: {source}",
                crate::text::verbatim(path.display()),
                crate::text::verbatim(recovery_path.display())
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

/// Encode the current schema with complete hierarchy, audio roots, event fades,
/// directed views, activation, MIDI, tuning, device state and retained native sources
/// with explicit per-track source bindings, Console audio cycles and bounded
/// faded Arrangement overlaps, explicit shared clip content (schema52), and
/// typed host gain envelopes (schema53), per-lane read selection (schema55),
/// independent MIDI note activation (schema56), and independent audio event
/// activation, including retained roots (schema57), and bounded nested audio
/// sources (schema58), consistent shared audio event projections (schema60),
/// independent audio event gain (schema61), MIDI clip gain (schema62), and
/// automatic track name origin (schema63).
/// Schema59 is reserved and explicitly refused.
/// There is no blanket byte ceiling; typed model limits still apply. Owned output growth is fallible,
/// but the codec materializes metadata and is not a streaming or OOM-proof reader.
pub fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    encode_document(&schema::track_name_origin::encode(project)?)
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
    schema::track_name_origin::require_user_names(project)?;
    schema::midi_clip_gain::require_unity(project)?;
    schema::event_gain::require_unity(project)?;
    schema::event_activation::require_all_active(project)?;
    schema::note_activation::require_all_active(project)?;
    schema::clip_aliases::require_legacy_compatible(project)?;
    schema::crossfades::require_legacy_disjoint(project)?;
    if project.has_device_automation() {
        return Err(StorageError::Invalid(
            "older schema cannot save device automation",
        ));
    }
    schema::session_audio_loops::refuse_legacy(project)?;
    if !project.native_sources().is_empty() {
        return Err(StorageError::Invalid(
            "older schema cannot save retained native sources",
        ));
    }
    schema::track_groups::require_legacy_compatible(project)?;
    schema::grouped_audio::require_legacy_compatible(project)?;
    schema::audio_events::require_legacy_compatible(project)?;
    schema::content_views::require_legacy_compatible(project)?;
    schema::clip_activation::require_legacy_compatible(project)?;
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
    struct Output(Vec<u8>);
    impl io::Write for Output {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.try_reserve(bytes.len()).map_err(io::Error::other)?;
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut output = Output(Vec::new());
    serde_json::to_writer(&mut output, document)?;
    if output.0.len() > MAX_PROJECT_DOCUMENT_BYTES {
        return Err(StorageError::Invalid(
            "project exceeds the 512 mib document limit; remove unused clips or assets",
        ));
    }
    Ok(output.0)
}
/// Open current and evidenced historical schemas without file/media I/O.
/// Schema55 requires authored/base read selection on every device envelope.
/// Schema53 and older select authored curves. Schema54 belongs to the evidenced
/// unmerged printed-output branch a4c1a99c2 and is explicitly unsupported here;
/// its role/silence fields must never be reinterpreted as read-selection metadata.
/// Schema53 requires a typed host-gain inventory; schema52 and older migrate
/// with no authored host targets. Provider numeric identities are unchanged.
/// Schema52 requires explicit shared source/member inventories; schema51 and older
/// acquire empty aliases while preserving their own strict format contracts.
/// Schemas1–33 select frozen readers and branch-aware adapters using the version
/// and complete field inventories. Only absent-by-design fields acquire their
/// original defaults; authored metadata is never discarded to accept a file.
/// Schema47 retains its evidenced receiverless source layout while allowing
/// retained processing projections on outer musical views and skipping inaudible
/// partial first cycles without rewriting authored cursors. Schema48 adds Console
/// audio cycles while retaining the receiverless singleton-source layout. Schema49
/// adds a required nullable source receiver and shared contributor bindings;
/// schemas46–48 infer a receiver only from their singleton binding.
/// Schema60 supports shared audio event projections that older readers refused.
/// Schema59 is reserved and unsupported. Schema58 retains nested audio sources;
/// earlier schemas migrate without nesting.
/// Schema57 retains inactive audio event identities; schemas1–56 migrate events
/// active. Schema56 retains inactive MIDI note identities; schemas1–55 migrate all notes
/// active. Schema50 adds ordered stable device/parameter envelopes with bounded normalized
/// points. Schemas1–49 migrate with no generic device automation; provider-owned
/// opaque state, including older EQ envelopes, is retained unchanged.
/// Schema45 retains
/// independent sources. Schemas1–44 migrate inline instruments
/// into retained singleton sources, preserving their device identities. Schema44
/// retains track hierarchy.
/// Schema43 retains independent audio sources and their captured processing tempos.
/// Schema42 adds source-clock event fades; schema41 inventories directed content views. The evidenced unmerged
/// grouped41 branch is selected by its disjoint complete inventory; mixed layouts refuse.
/// Earlier schemas migrate to empty root tables/source-zero events and identity views.
/// Pre40 schemas default clip activation to true and elapsed MIDI windows to
/// zero. Schema40 inventories every Arrangement/Session clip exactly once.
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
    #[derive(serde::Deserialize)]
    struct CurrentHeader<'a> {
        format: &'a str,
        version: u32,
    }
    let header: CurrentHeader<'_> = serde_json::from_slice(bytes)?;
    if header.format != "hex-project" {
        return Err(StorageError::Invalid("unsupported project format"));
    }
    let project = match header.version {
        CURRENT_SCHEMA_VERSION => schema::track_name_origin::decode(bytes),
        62 => schema::midi_clip_gain::decode(bytes),
        61 => schema::event_gain::decode(bytes),
        60 => schema::alias_projection_v60::decode(bytes),
        59 => Err(StorageError::Invalid(
            "schema59 is reserved for an unsupported development layout; open a copy with the matching hex build and retain the original for recovery",
        )),
        58 => schema::nested_audio::decode(bytes),
        57 => schema::event_activation::decode(bytes),
        56 => schema::note_activation::decode(bytes),
        55 => schema::automation_read::decode(bytes),
        54 => Err(StorageError::Invalid(
            "schema54 belongs to the printed-output branch; open a copy with the matching hex build; original file unchanged",
        )),
        53 => schema::host_automation::decode(bytes),
        52 => schema::clip_aliases::decode(bytes),
        51 => schema::crossfades::decode(bytes),
        50 => schema::device_automation::decode(bytes),
        49 => schema::session_audio_loops::decode49(bytes),
        48 => schema::session_audio_loops::decode48(bytes),
        47 => schema::native_sources::decode47(bytes),
        46 => schema::native_sources::decode46(bytes),
        45 => schema::native_sources::decode45(bytes),
        44 => schema::track_groups::decode(bytes),
        43 => schema::grouped_audio::decode(bytes),
        42 => schema::event_fades::decode(bytes),
        41 => schema::grouped_audio::decode41(bytes),
        40 => schema::clip_activation::decode(bytes),
        39 => schema::midi_one_shot::decode(bytes),
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
    }?;
    if header.version < 52 {
        schema::clip_aliases::require_legacy_compatible(&project)?;
    }
    if header.version < 51 {
        schema::crossfades::require_legacy_disjoint(&project)?;
    }
    if header.version < 47 && requires_schema47_audio_placement(&project)? {
        return Err(StorageError::Invalid(
            "historical project schema contains unsupported audio placement; open a copy with the hex build that saved it; original file unchanged",
        ));
    }
    Ok(project)
}

/// Historical schemas admitted retained processing and outer placements as
/// separate states, and required a non-exhausted first occurrence to contain
/// frames. Apply that frozen contract after decoding into validated
/// project objects, including grouped retained source clips; opaque device state
/// is intentionally outside this audio-model check.
fn requires_schema47_audio_placement(project: &Project) -> Result<bool, StorageError> {
    let has_state =
        |clip: &hex_project::Clip, tempo: &hex_project::TempoMap| -> Result<bool, StorageError> {
            let Some(edit) = clip.audio_edit() else {
                return Ok(false);
            };
            if edit.processing_projection.is_some() && edit.placement.is_some() {
                return Ok(true);
            }
            if edit.placement.is_none() && edit.content_view.is_none() {
                return Ok(false);
            }
            let projection = hex_project::AudioViewProjection::new(clip, tempo)
                .map_err(|error| StorageError::Invalid(error.0))?;
            Ok(projection.has_subframe_intro())
        };
    for track in project.tracks() {
        for clip in track.clips() {
            if has_state(clip, project.tempo_map())? {
                return Ok(true);
            }
            if let Some(events) = clip
                .audio_edit()
                .and_then(|edit| edit.domain.events.as_ref())
            {
                for source in &events.sources {
                    if has_state(&source.clip, &source.tempo)? {
                        return Ok(true);
                    }
                }
            }
        }
    }
    Ok(false)
}
