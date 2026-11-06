//! # Purpose
//! Own strict canonical project metadata serialization, recoverable publication and independent
//! source-bound automation recording drafts. Drafts are not canonical lane references.
//!
//! # Dependencies
//! Uses hex-project, hex-tuning, hex-lattice-contract, serde/serde_json, sha2 and fs2, plus
//! libc errno/flags, rustix's safe exclusive rename for create-new publication and its safe
//! xattr calls to keep a replaced project's extended attributes.
//!
//! # Does not own
//! Does not decode PCM, construct DSP, access callback state or silently convert unsupported
//! formats. Unsupported/malformed input refuses without rewriting the original file.
pub mod automation_draft;
mod files;
mod journal;
pub mod preset;
pub mod project_name;
mod publication;
pub mod text;
pub mod tuning_library;
pub use journal::{
    Journal, JournalRestartOutcome, JournalRestartToken, MAX_UNTITLED_SLOTS, UntitledSlotClaim,
    journal_path, load_journal, restart_journal, untitled_slot_path,
};
mod audio_edit;
mod extended_attributes;
mod replacement;
mod schema;
mod temporaries;
pub use extended_attributes::MetadataWarning;
pub use files::{
    FileToken, SaveNewOutcome, load, load_with_token, save_new, save_new_with_outcome,
    save_new_with_token,
};
use hex_project::Project;
pub use replacement::{
    PreparedReplacement, RecoveryDocument, ReplaceOutcome, ReplaceWarning, load_recovery,
    prepare_replacement_within, recovery_path, replace_confirmed, replace_existing,
};
use std::{fmt, io, path::PathBuf};

/// Current saved-project schema for projects without sinc v2 media; independent
/// of the application version. Such projects keep the layout older readers open.
/// Schema63 records automatic track names; earlier names migrate as user-set.
/// Schema62 adds independent MIDI clip gain; schema61 migrates at unity.
/// Schema59 is a reserved incompatible development layout and remains unsupported.
pub const CURRENT_SCHEMA_VERSION: u32 = 63;

/// Schema written only for projects containing `SINC_CONVERSION_V2` assets:
/// schema63 plus each catalog row's actual conversion version. Readers before
/// schema64 refuse it as newer, so saving one over an older file needs consent.
pub const SINC_V2_SCHEMA_VERSION: u32 = 64;

/// Schema written only for projects whose audio content names a warp resampler
/// law other than the one its pitch/warp state implies (in practice, content
/// rendered with the v2 law): schema64 (or schema63) plus an optional
/// `warp_resampler` domain field. See `schema::warp_resampler` for migration.
/// Readers before schema65 refuse it as newer, so such a save needs consent.
pub const WARP_RESAMPLER_SCHEMA_VERSION: u32 = 65;

/// Schema written only for projects with a directed audio content view whose
/// read resampler law differs from the one its clip clock implies (in practice,
/// a new musical-clock view read with v2): the schema65-or-older layout plus an
/// optional `read_resampler` view field. See `schema::content_view_resampler`.
/// Readers before schema66 refuse it as newer, so such a save needs consent.
pub const CONTENT_VIEW_RESAMPLER_SCHEMA_VERSION: u32 = 66;

/// Schema written only for projects containing a saved native device state
/// newer than schema66 readers decode (in practice a Sampler v3 state with a
/// loop crossfade): the schema66-or-older layout, unchanged. See
/// `schema::sampler_crossfade`. Readers before schema67 refuse it as newer, so
/// such a save needs consent.
pub const DEVICE_STATE_SCHEMA_VERSION: u32 = 67;

/// Schema68 stores receiver-owned retained source paths; earlier readers refuse
/// these projects, so overwriting an older-format file requires consent.
pub const RETAINED_RECEIVER_SCHEMA_VERSION: u32 = 68;
/// Schema69 stores the Compressor's authored stereo sidechain source.
pub const SIDECHAIN_SCHEMA_VERSION: u32 = 69;
/// Newest schema this build opens; anything above it is refused as newer.
pub const NEWEST_SCHEMA_VERSION: u32 = SAMPLE_RATE_MODE_SCHEMA_VERSION;

/// Schema70 stores an independent original source span for signed musical event placement.
pub const MUSICAL_EVENT_SOURCE_SCHEMA_VERSION: u32 = 70;
/// Schema71 extends the accepted frame extent for sparse one-shot Arrangement
/// event roots. Historical schemas keep the original dense extent contract.
pub const EXTENDED_FRAME_EVENTS_SCHEMA_VERSION: u32 = 71;
/// Schema72 adds canonical track tag assignments to the complete event layout.
pub const TRACK_TAGS_SCHEMA_VERSION: u32 = 72;
/// Schema73 stores a leading silent gate for a finite musical one-shot.
pub const LEADING_AUDIO_ONCE_SCHEMA_VERSION: u32 = 73;
/// Schema74 adds authored Arrangement group output mute ranges.
pub const GROUP_OUTPUT_MUTE_SCHEMA_VERSION: u32 = 74;
/// Schema75 preserves VST3 device envelopes beyond the historical 4 KiB bound.
pub const VST3_STATE_SIZE_SCHEMA_VERSION: u32 = 75;
/// Schema76 persists Follow Interface with its last applied effective rate.
/// Fixed-rate projects keep the oldest compatible schema.
pub const SAMPLE_RATE_MODE_SCHEMA_VERSION: u32 = 76;

fn contains_larger_device_state(project: &Project) -> bool {
    schema::device_assets::authored_states(project)
        .any(|state| state.payload().len() > hex_lattice_contract::MAX_NATIVE_DEVICE_STATE_BYTES)
}

fn contains_leading_audio_once(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .flat_map(|clip| {
            std::iter::once(clip).chain(clip.retained_audio_roots().map(|root| &root.clip))
        })
        .any(|clip| {
            clip.audio_edit()
                .and_then(|edit| edit.placement)
                .is_some_and(|placement| {
                    matches!(placement.mode, hex_project::AudioViewMode::OnceWithLead(_))
                })
        })
}

fn contains_extended_frame_events(project: &Project) -> bool {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .any(|clip| {
            clip.audio_edit().is_some_and(|edit| {
                matches!(edit.domain.clock, hex_project::AudioDomainClock::Frames(length)
                if length > hex_project::MAX_AUDIO_DOMAIN_FRAMES)
            })
        })
}

/// Schema [`encode`] writes for `project`:
/// [`SAMPLE_RATE_MODE_SCHEMA_VERSION`] when the mode is Follow Interface,
/// [`VST3_STATE_SIZE_SCHEMA_VERSION`] when any VST3 state exceeds 4 KiB,
/// [`GROUP_OUTPUT_MUTE_SCHEMA_VERSION`] when a group output has authored mute ranges,
/// [`LEADING_AUDIO_ONCE_SCHEMA_VERSION`] for a finite musical placement with a silent lead,
/// [`TRACK_TAGS_SCHEMA_VERSION`] when track tags are present,
/// [`EXTENDED_FRAME_EVENTS_SCHEMA_VERSION`] for an extended frame-event root,
/// [`MUSICAL_EVENT_SOURCE_SCHEMA_VERSION`] for an independent musical event source,
/// [`SIDECHAIN_SCHEMA_VERSION`] when an authored sidechain is present,
/// [`RETAINED_RECEIVER_SCHEMA_VERSION`] when a retained source path is present,
/// [`DEVICE_STATE_SCHEMA_VERSION`] when a device state is newer than schema66
/// readers decode, [`CONTENT_VIEW_RESAMPLER_SCHEMA_VERSION`] when a directed view names an
/// explicit read law, [`WARP_RESAMPLER_SCHEMA_VERSION`]
/// when content names an explicit warp resampler law,
/// [`SINC_V2_SCHEMA_VERSION`] when it contains sinc v2 media, otherwise
/// [`CURRENT_SCHEMA_VERSION`].
pub fn schema_version_for(project: &Project) -> u32 {
    if schema::sample_rate_mode::contains(project) {
        SAMPLE_RATE_MODE_SCHEMA_VERSION
    } else if contains_larger_device_state(project) {
        VST3_STATE_SIZE_SCHEMA_VERSION
    } else if schema::group_output_mute::contains(project) {
        GROUP_OUTPUT_MUTE_SCHEMA_VERSION
    } else if contains_leading_audio_once(project) {
        LEADING_AUDIO_ONCE_SCHEMA_VERSION
    } else if schema::track_tags::contains(project) {
        TRACK_TAGS_SCHEMA_VERSION
    } else if contains_extended_frame_events(project) {
        EXTENDED_FRAME_EVENTS_SCHEMA_VERSION
    } else if schema::musical_event_source::contains(project) {
        MUSICAL_EVENT_SOURCE_SCHEMA_VERSION
    } else if !project.sidechains().is_empty() {
        SIDECHAIN_SCHEMA_VERSION
    } else if schema::retained_receiver::contains(project) {
        RETAINED_RECEIVER_SCHEMA_VERSION
    } else if schema::sampler_crossfade::contains_newer_states(project) {
        DEVICE_STATE_SCHEMA_VERSION
    } else if schema::content_view_resampler::contains_explicit(project) {
        CONTENT_VIEW_RESAMPLER_SCHEMA_VERSION
    } else if schema::warp_resampler::contains_explicit(project) {
        WARP_RESAMPLER_SCHEMA_VERSION
    } else if schema::conversion_v2::contains_v2(project) {
        SINC_V2_SCHEMA_VERSION
    } else {
        CURRENT_SCHEMA_VERSION
    }
}

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
    /// Another owner currently holds the automation draft's exclusive lock.
    AutomationDraftBusy(io::Error),
    /// The operation's visible state is retained; lock cleanup failed.
    JournalLockRelease {
        path: PathBuf,
        change: JournalChange,
        operation_error: Option<Box<StorageError>>,
        source: io::Error,
    },
    Invalid(&'static str),
    /// The project document bytes differ from the snapshot captured when it was
    /// loaded, typically because another hex instance or editor saved it. Its
    /// message names the next steps: save as, then reopen or compare.
    Conflict {
        path: PathBuf,
    },
    /// A recovery journal slot differs from the bytes this writer last owned.
    /// The slot is left untouched; the open project stays in memory.
    JournalConflict {
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
            Self::AutomationDraftBusy(error) => {
                write!(f, "automation draft is busy; try again: {error}")
            }
            Self::Json(error) => write!(f, "invalid project json: {error}"),
            Self::Invalid(reason) => f.write_str(reason),
            Self::Conflict { path } => write!(
                f,
                "project changed outside hex at {}; your version was not saved. \
                 use save as to keep it under a new name, then reopen the original \
                 to load or compare the outside changes",
                crate::text::verbatim(path.display())
            ),
            Self::JournalConflict { path } => write!(
                f,
                "project changed outside hex at {}",
                crate::text::verbatim(path.display())
            ),
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
            Self::Io(e) | Self::AutomationDraftBusy(e) => Some(e),
            Self::Json(e) => Some(e),
            Self::Invalid(_) | Self::Conflict { .. } | Self::JournalConflict { .. } => None,
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
/// independent audio event gain (schema61), MIDI clip gain (schema62),
/// and automatic track name origin (schema63). Only a project containing sinc
/// v2 media is written as schema64 (see [`schema_version_for`]); every other
/// project stays schema63, byte-compatible with older readers. Only content
/// naming an explicit warp resampler law makes a project schema65, and only a
/// directed view naming an explicit read law makes it schema66, and only a
/// device state newer than schema66 readers decode makes it schema67. A retained
/// receiver path makes it schema68; absent paths migrate as ordinary bindings.
/// A Compressor sidechain makes it schema69; older projects open disconnected.
/// An independent musical event source makes it schema70. A frame-event root
/// beyond the dense processing limit makes it schema71. Tags make it schema72.
/// A leading musical one-shot makes it schema73 and group mute ranges schema74.
/// A VST3 state beyond 4 KiB uses schema75, including inactive A/B variants.
/// Schema59 is reserved and explicitly refused.
/// There is no blanket byte ceiling; typed model limits still apply. Owned output growth is fallible,
/// but the codec materializes metadata and is not a streaming or OOM-proof reader.
pub fn encode(project: &Project) -> Result<Vec<u8>, StorageError> {
    if schema::sample_rate_mode::contains(project) {
        encode_document(&schema::sample_rate_mode::encode(project)?)
    } else if contains_larger_device_state(project) {
        let mut value = schema::group_output_mute::encode(project)?;
        value["version"] = VST3_STATE_SIZE_SCHEMA_VERSION.into();
        encode_document(&value)
    } else if schema::group_output_mute::contains(project) {
        encode_document(&schema::group_output_mute::encode(project)?)
    } else if contains_leading_audio_once(project) {
        let mut value = schema::track_tags::encode(project)?;
        value["version"] = LEADING_AUDIO_ONCE_SCHEMA_VERSION.into();
        encode_document(&value)
    } else if schema::track_tags::contains(project) {
        encode_document(&schema::track_tags::encode(project)?)
    } else if contains_extended_frame_events(project) {
        let mut value = schema::musical_event_source::encode(project)?;
        value["version"] = EXTENDED_FRAME_EVENTS_SCHEMA_VERSION.into();
        encode_document(&value)
    } else if schema::musical_event_source::contains(project) {
        encode_document(&schema::musical_event_source::encode(project)?)
    } else if !project.sidechains().is_empty() {
        encode_document(&schema::sidechain::encode(project)?)
    } else if schema::retained_receiver::contains(project) {
        encode_document(&schema::retained_receiver::encode(project)?)
    } else if schema::sampler_crossfade::contains_newer_states(project) {
        encode_document(&schema::sampler_crossfade::encode(project)?)
    } else if schema::content_view_resampler::contains_explicit(project) {
        encode_document(&schema::content_view_resampler::encode(project)?)
    } else if schema::warp_resampler::contains_explicit(project) {
        encode_document(&schema::warp_resampler::encode(project)?)
    } else if schema::conversion_v2::contains_v2(project) {
        encode_document(&schema::conversion_v2::encode(project)?)
    } else {
        encode_document(&schema::track_name_origin::encode(project)?)
    }
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
    if schema::sample_rate_mode::contains(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save the project sample rate mode",
        ));
    }
    if contains_larger_device_state(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save a larger vst3 device state; save with the current format",
        ));
    }
    if schema::track_tags::contains(project) {
        return Err(StorageError::Invalid("older schema cannot save track tags"));
    }
    if contains_extended_frame_events(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save an extended frame event root",
        ));
    }
    if schema::musical_event_source::contains(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save a retained musical event source",
        ));
    }
    if !project.sidechains().is_empty() {
        return Err(StorageError::Invalid(
            "older schema cannot save a compressor sidechain",
        ));
    }
    if schema::retained_receiver::contains(project) {
        return Err(StorageError::Invalid(
            "older schema cannot save a retained source path",
        ));
    }
    schema::sampler_crossfade::require_older_states(project)?;
    schema::content_view_resampler::require_implied(project)?;
    schema::warp_resampler::require_implied(project)?;
    schema::conversion_v2::require_v1_conversions(project)?;
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
/// Schema64 catalog rows may carry sinc v2 conversion; a v2 row in any earlier
/// schema refuses. Schema63 and older migrate their identity/v1 assets unchanged.
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
/// From schema30 upward the migration chain parses the bytes once (after one
/// duplicate-field check) and each frozen layer adapts that single value in
/// place; layers never re-serialize the document for the next one.
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
        SAMPLE_RATE_MODE_SCHEMA_VERSION => schema::sample_rate_mode::decode(bytes),
        VST3_STATE_SIZE_SCHEMA_VERSION | GROUP_OUTPUT_MUTE_SCHEMA_VERSION => {
            schema::group_output_mute::decode(bytes, header.version)
        }
        LEADING_AUDIO_ONCE_SCHEMA_VERSION => {
            schema::track_tags::decode_with_version(bytes, LEADING_AUDIO_ONCE_SCHEMA_VERSION)
        }
        TRACK_TAGS_SCHEMA_VERSION => schema::track_tags::decode(bytes),
        EXTENDED_FRAME_EVENTS_SCHEMA_VERSION => {
            schema::musical_event_source::decode(bytes, EXTENDED_FRAME_EVENTS_SCHEMA_VERSION)
        }
        MUSICAL_EVENT_SOURCE_SCHEMA_VERSION => schema::track_tags::decode_schema70(bytes),
        SIDECHAIN_SCHEMA_VERSION => schema::sidechain::decode(bytes),
        RETAINED_RECEIVER_SCHEMA_VERSION => schema::retained_receiver::decode(bytes),
        DEVICE_STATE_SCHEMA_VERSION => schema::sampler_crossfade::decode(bytes),
        CONTENT_VIEW_RESAMPLER_SCHEMA_VERSION => schema::content_view_resampler::decode(bytes),
        WARP_RESAMPLER_SCHEMA_VERSION => schema::warp_resampler::decode(bytes),
        SINC_V2_SCHEMA_VERSION => schema::conversion_v2::decode(bytes),
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
        version if version > NEWEST_SCHEMA_VERSION => Err(StorageError::Invalid(
            "project uses a newer schema; open it with a newer hex version; original file unchanged",
        )),
        _ => Err(StorageError::Invalid(
            "unsupported older project schema; open a copy with the hex build that saved it and retain the original for recovery",
        )),
    }?;
    if header.version != SAMPLE_RATE_MODE_SCHEMA_VERSION
        && contains_larger_device_state(&project)
            != (header.version == VST3_STATE_SIZE_SCHEMA_VERSION)
    {
        return Err(StorageError::Invalid(
            "project schema disagrees with vst3 device state size; original file unchanged",
        ));
    }
    if header.version < GROUP_OUTPUT_MUTE_SCHEMA_VERSION
        && contains_leading_audio_once(&project)
            != (header.version == LEADING_AUDIO_ONCE_SCHEMA_VERSION)
    {
        return Err(StorageError::Invalid(
            "project schema disagrees with its leading musical one-shot; original file unchanged",
        ));
    }
    if header.version < TRACK_TAGS_SCHEMA_VERSION
        && contains_extended_frame_events(&project)
            != (header.version == EXTENDED_FRAME_EVENTS_SCHEMA_VERSION)
    {
        return Err(StorageError::Invalid(
            "project schema disagrees with extended frame event roots; original file unchanged",
        ));
    }
    if header.version < DEVICE_STATE_SCHEMA_VERSION
        && schema::sampler_crossfade::require_older_states(&project).is_err()
    {
        return Err(StorageError::Invalid(
            "historical project schema contains a newer device state; original file unchanged",
        ));
    }
    if header.version < CONTENT_VIEW_RESAMPLER_SCHEMA_VERSION
        && schema::content_view_resampler::require_implied(&project).is_err()
    {
        return Err(StorageError::Invalid(
            "historical project schema contains a directed view resampler version; original file unchanged",
        ));
    }
    if header.version < WARP_RESAMPLER_SCHEMA_VERSION
        && schema::warp_resampler::require_implied(&project).is_err()
    {
        return Err(StorageError::Invalid(
            "historical project schema contains a warp resampler version; original file unchanged",
        ));
    }
    if header.version < SINC_V2_SCHEMA_VERSION
        && schema::conversion_v2::require_v1_conversions(&project).is_err()
    {
        return Err(StorageError::Invalid(
            "historical project schema contains unsupported media conversion metadata; original file unchanged",
        ));
    }
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
