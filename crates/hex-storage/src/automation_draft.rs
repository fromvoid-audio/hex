//! Immutable-source-bound, control-side automation capture files.
//!
//! This independent versioned format does not change the project schema. The
//! complete current-format source checkpoint appears once; fixed-size chained
//! records retain exact normalized applications and actual terminal clocks.
//! Capture duration never grows a resident event collection or ordinal index.
//! Append accepts at most 64 events, validates the whole batch before writing,
//! and acknowledges durability only after sync. A write/sync failure is sticky:
//! the file and acknowledged prefix remain available for explicit recovery.
//!
//! The file itself carries a cooperative exclusive writer/shared reader lock.
//! Locks release on close; neither owner deletes files on drop. Readers require
//! writer quiescence and report missing, torn or corrupt terminals visibly. An
//! integrity chain detects accidental damage, not malicious authenticated input.
//! Provider capability/adoption and source path normalization belong to Session;
//! this owner checks neutral source inventory and refuses relative media paths.
//! The closed completed-take reader also owns semantic clock validation and bounded
//! projected spans; Session retains live admission and canonical adoption. Full
//! checkpoints/tokens remain reader-owned, with no native footprint claim.
//! Version 1 retains its original unknown-clock header for recovery. Version 2
//! additionally retains an explicit configured capture clock; it does not infer
//! that clock from the saved loop marker. Version3 adds raw VST3 targets to
//! that clocked header, retaining their exact class in the embedded source.
//! Historical1/2 reject the new target kind. Version4 groups exact VST3
//! targets with one source header and a separate record kind for each actual
//! per-parameter restoration. No format proves live admission.
use crate::{FileToken, StorageError, decode, encode, files, replacement};
use hex_project::{AutomationValue, DeviceAutomationTarget, DeviceId, Project, TrackId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 8] = b"HEXADFT\0";
const VERSION: u32 = 1;
const CLOCK_VERSION: u32 = 2;
// Version3 adds raw VST3 target kind4 to the explicit-clock header. The exact
// class remains in the integrity-checked embedded source; versions1/2 never
// reinterpret this kind. Native/track writers retain their historical format.
const VST3_CLOCK_VERSION: u32 = 3;
const VST3_GROUP_VERSION: u32 = 4;

mod group;
pub use group::{GroupApplication, GroupDraftEvent, MAX_GROUP_TARGETS, Vst3GroupTarget};
use group::{GroupHeader, GroupState};
mod target;
use target::{metadata, resolve_target};
const PREFIX_BYTES: usize = 32;
const RECORD_BYTES: usize = 96;
const PAYLOAD_BYTES: usize = 64;
const MAX_METADATA_BYTES: u64 = 1024;
pub const MAX_APPEND_EVENTS: usize = 64;

mod completed;
pub use completed::{CompletedDraft, CompletedGroupedDraft};
mod take;
pub use take::{
    AutomationSharedData, CompletedAutomationTake, MAX_PREVIEW_QUERIES, MAX_SCALAR_RANGE_FRAMES,
    MAX_SPANS_PER_STEP, PreparedAutomationTake, PreparedAutomationTakeDataLayout,
    ProjectedAutomationSpan, ProjectedGroupLane, ProjectedSpanCursor, SpanReadProgress,
    SpanReadState, TakeOpenError, automation_projection_fingerprint,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftTarget {
    TrackGain {
        track: TrackId,
    },
    Device {
        device: DeviceId,
        target: DeviceAutomationTarget,
    },
    Vst3Group(Vst3GroupTarget),
}

/// Actual confirmed application, never an enqueue receipt or requested clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppliedEvent {
    pub sequence: u64,
    pub performance_frame: u64,
    pub content_frame: u64,
    pub loop_pass: u64,
    pub value: AutomationValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DraftClock {
    pub performance_frame: u64,
    pub content_frame: u64,
    pub loop_pass: u64,
}

/// Configured project-rate capture law, retained independently of saved loop settings.
/// `None` is explicitly linear; absence of this descriptor in version 1 is unknown.
/// The caller must obtain it from the matching playback owner and fence source/transport
/// changes. Storing these facts does not establish that an event was actually delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureClock {
    pub sample_rate: u32,
    pub loop_region: Option<hex_project::LoopRegion>,
}

/// Audit/fencing provenance only. These scalars never authorize adoption.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftProvenance {
    pub sample_rate: u32,
    pub output_identity: u64,
    pub project_generation: u64,
    pub editor_revision: u64,
    pub audio_revision: u64,
    pub transport_epoch: u64,
    pub gesture_serial: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftAbortReason {
    Cancelled,
    SourceChanged,
    TargetUnavailable,
    TransportChanged,
    OutputFailed,
    UnsupportedOutput,
    ClockExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftTerminal {
    Finished(DraftClock),
    Aborted(DraftAbortReason),
    /// Explicit quiescence without a confirmed native terminal or end clock.
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftDamage {
    TruncatedRecord,
    Integrity,
    InvalidRecord,
    TrailingData,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftOutcome {
    Interrupted,
    Finished(DraftClock),
    Aborted(DraftAbortReason),
    Damaged { reason: DraftDamage },
}

/// Accepted means validated for writing; durable means sync acknowledged.
/// On recovery both counts describe the validated prefix visible on disk, not
/// proof that the previous process observed a sync acknowledgement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DraftProgress {
    pub accepted_events: u64,
    pub durable_events: u64,
    pub durable_terminal: Option<DraftTerminal>,
}

/// Integrity identity of a writer's sync-acknowledged event prefix, including its header.
/// This detects accidental replacement or damage; it is not an authentication mechanism.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DraftPrefixIdentity {
    events: u64,
    chain: [u8; 32],
}
impl DraftPrefixIdentity {
    /// Number of sync-acknowledged events bound by this identity.
    pub fn events(self) -> u64 {
        self.events
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DraftReadLimits {
    /// Checked against the framed length before source allocation or decoding.
    pub max_source_bytes: u64,
}

#[derive(Debug)]
pub struct DraftError {
    pub path: PathBuf,
    pub progress: DraftProgress,
    /// True only after create-new publication has exposed the destination.
    pub published: bool,
    pub source: Box<StorageError>,
}
impl fmt::Display for DraftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "automation draft at {}: {} ({} durable events)",
            crate::text::verbatim(self.path.display()),
            self.source,
            self.progress.durable_events
        )
    }
}
impl std::error::Error for DraftError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    target_kind: u8,
    owner: u64,
    parameter: u32,
    provenance: DraftProvenance,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClockMetadata {
    sample_rate: u32,
    loop_region: LoopMetadata,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum LoopMetadata {
    Linear {},
    Region { start: u64, end: u64 },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClockedMetadata {
    metadata: Metadata,
    capture_clock: ClockMetadata,
}

pub struct DraftWriter {
    file: File,
    path: PathBuf,
    progress: DraftProgress,
    previous: Option<AppliedEvent>,
    group: Option<GroupState>,
    chain: [u8; 32],
    durable_chain: [u8; 32],
    failed: bool,
}

impl DraftWriter {
    /// Publish a complete immutable source header with create-new semantics.
    /// Existing targets, including dangling links, are never replaced. A
    /// post-publication error retains the file and reports `published = true`.
    pub fn create_new(
        path: impl AsRef<Path>,
        source: &Project,
        target: DraftTarget,
        provenance: DraftProvenance,
    ) -> Result<Self, DraftError> {
        Self::create_with_finish(
            path.as_ref(),
            source,
            target,
            provenance,
            files::finish_publication,
        )
    }

    /// Publish a version-2 header retaining the caller's explicit configured capture clock.
    /// Version-1 `create_new` remains available for raw recovery without known loop context.
    pub fn create_with_clock(
        path: impl AsRef<Path>,
        source: &Project,
        target: DraftTarget,
        provenance: DraftProvenance,
        clock: CaptureClock,
    ) -> Result<Self, DraftError> {
        Self::create_with_clock_and_finish(
            path.as_ref(),
            source,
            target,
            provenance,
            Some(clock),
            files::finish_publication,
        )
    }

    fn create_with_finish(
        path: &Path,
        source: &Project,
        target: DraftTarget,
        provenance: DraftProvenance,
        finish: impl FnOnce(&Path, &Path) -> Result<(), (&'static str, io::Error)>,
    ) -> Result<Self, DraftError> {
        Self::create_with_clock_and_finish(path, source, target, provenance, None, finish)
    }

    fn create_with_clock_and_finish(
        path: &Path,
        source: &Project,
        target: DraftTarget,
        provenance: DraftProvenance,
        capture_clock: Option<CaptureClock>,
        finish: impl FnOnce(&Path, &Path) -> Result<(), (&'static str, io::Error)>,
    ) -> Result<Self, DraftError> {
        let mut published = false;
        let result = (|| -> Result<Self, StorageError> {
            validate_source(source)?;
            validate_provenance(provenance)?;
            let (version, meta, group) = if let DraftTarget::Vst3Group(target) = target {
                let clock = capture_clock.ok_or(StorageError::Invalid(
                    "grouped vst3 automation draft requires an explicit capture clock",
                ))?;
                target.validate_source(source)?;
                validate_capture_clock(source, provenance, clock)?;
                (
                    VST3_GROUP_VERSION,
                    GroupHeader::encode(target, provenance, clock)?,
                    Some(GroupState::new(target, clock)),
                )
            } else {
                let metadata = metadata(source, target, provenance)?;
                if metadata.target_kind == 4 && capture_clock.is_none() {
                    return Err(StorageError::Invalid(
                        "vst3 automation draft requires an explicit capture clock",
                    ));
                }
                if let Some(clock) = capture_clock {
                    validate_capture_clock(source, provenance, clock)?;
                    (
                        if metadata.target_kind == 4 {
                            VST3_CLOCK_VERSION
                        } else {
                            CLOCK_VERSION
                        },
                        serde_json::to_vec(&ClockedMetadata {
                            metadata,
                            capture_clock: ClockMetadata::from(clock),
                        })?,
                        None,
                    )
                } else {
                    (VERSION, serde_json::to_vec(&metadata)?, None)
                }
            };
            if path.file_name().is_none() {
                return Err(StorageError::Invalid("automation draft needs a filename"));
            }
            match fs::symlink_metadata(path) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "automation draft already exists",
                    )
                    .into());
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            let source = encode(source)?;
            let mut bytes = Vec::new();
            let size = PREFIX_BYTES
                .checked_add(source.len())
                .and_then(|n| n.checked_add(meta.len()))
                .and_then(|n| n.checked_add(32))
                .ok_or(StorageError::Invalid(
                    "automation draft header is too large",
                ))?;
            bytes.try_reserve_exact(size).map_err(io::Error::other)?;
            bytes.extend_from_slice(MAGIC);
            bytes.extend_from_slice(&version.to_le_bytes());
            bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&(source.len() as u64).to_le_bytes());
            bytes.extend_from_slice(&(meta.len() as u64).to_le_bytes());
            bytes.extend_from_slice(&source);
            bytes.extend_from_slice(&meta);
            let chain: [u8; 32] = Sha256::digest(&bytes).into();
            bytes.extend_from_slice(&chain);
            let temporary = replacement::prepare_temp(parent(path), &bytes, None)?;
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&temporary.path)?;
            fs2::FileExt::try_lock_exclusive(&file)?;
            file.seek(SeekFrom::End(0))?;
            // The open locked handle must stay bound to the published draft,
            // so a non-atomic copy is not an acceptable fallback here.
            crate::publication::publish_new_atomic(&temporary.path, path)?;
            published = true;
            finish(&temporary.path, path).map_err(|(operation, source)| {
                StorageError::PublishedButNotDurable {
                    path: path.to_path_buf(),
                    operation,
                    source,
                }
            })?;
            Ok(Self {
                file,
                path: path.to_path_buf(),
                progress: DraftProgress::default(),
                previous: None,
                group,
                chain,
                durable_chain: chain,
                failed: false,
            })
        })();
        result.map_err(|source| DraftError {
            path: path.to_path_buf(),
            progress: DraftProgress::default(),
            published,
            source: Box::new(source),
        })
    }

    pub fn has_group_records(&self) -> bool {
        self.group.is_some()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn progress(&self) -> DraftProgress {
        self.progress
    }

    /// Identity advances only when the event prefix has been sync-acknowledged.
    pub fn durable_prefix_identity(&self) -> DraftPrefixIdentity {
        DraftPrefixIdentity {
            events: self.progress.durable_events,
            chain: self.durable_chain,
        }
    }

    pub fn append(&mut self, events: &[AppliedEvent]) -> Result<DraftProgress, DraftError> {
        self.append_with(events, |file, bytes| file.write_all(bytes), File::sync_all)
    }

    fn append_with(
        &mut self,
        events: &[AppliedEvent],
        write: impl FnMut(&mut File, &[u8]) -> io::Result<()>,
        sync: impl FnOnce(&File) -> io::Result<()>,
    ) -> Result<DraftProgress, DraftError> {
        let result = (|| -> Result<(), StorageError> {
            self.check_batch(events.len())?;
            if self.group.is_some() {
                return Err(StorageError::Invalid(
                    "grouped take requires grouped records",
                ));
            }
            let mut previous = self.previous;
            for &event in events {
                validate_event(previous, event)?;
                previous = Some(event);
            }
            self.write_batch(events, event_record, write, sync)?;
            self.previous = previous;
            Ok(())
        })();
        result
            .map(|()| self.progress)
            .map_err(|source| self.error(source))
    }

    /// Validate the whole offered batch before publishing any record.
    pub fn append_group(
        &mut self,
        events: &[GroupDraftEvent],
    ) -> Result<DraftProgress, DraftError> {
        self.append_group_with(events, |file, bytes| file.write_all(bytes), File::sync_all)
    }

    fn append_group_with(
        &mut self,
        events: &[GroupDraftEvent],
        write: impl FnMut(&mut File, &[u8]) -> io::Result<()>,
        sync: impl FnOnce(&File) -> io::Result<()>,
    ) -> Result<DraftProgress, DraftError> {
        let result = (|| -> Result<(), StorageError> {
            self.check_batch(events.len())?;
            let mut group = self.group.ok_or(StorageError::Invalid(
                "single target draft refuses grouped records",
            ))?;
            for &event in events {
                group.validate(event)?;
            }
            self.write_batch(events, group::encode, write, sync)?;
            self.group = Some(group);
            Ok(())
        })();
        result
            .map(|()| self.progress)
            .map_err(|source| self.error(source))
    }

    fn check_batch(&self, len: usize) -> Result<(), StorageError> {
        self.check_open()?;
        if len > MAX_APPEND_EVENTS {
            return Err(StorageError::Invalid(
                "automation draft append exceeds 64 events",
            ));
        }
        Ok(())
    }

    fn write_batch<T: Copy>(
        &mut self,
        events: &[T],
        encode: impl Fn(T, [u8; 32]) -> [u8; RECORD_BYTES],
        mut write: impl FnMut(&mut File, &[u8]) -> io::Result<()>,
        sync: impl FnOnce(&File) -> io::Result<()>,
    ) -> Result<(), StorageError> {
        let accepted = self
            .progress
            .accepted_events
            .checked_add(events.len() as u64)
            .ok_or(StorageError::Invalid(
                "automation draft event count overflow",
            ))?;
        if events.is_empty() {
            return Ok(());
        }
        self.progress.accepted_events = accepted;
        for &event in events {
            let record = encode(event, self.chain);
            if let Err(error) = write(&mut self.file, &record) {
                self.failed = true;
                return Err(error.into());
            }
            self.chain.copy_from_slice(&record[PAYLOAD_BYTES..]);
        }
        if let Err(error) = sync(&self.file) {
            self.failed = true;
            return Err(error.into());
        }
        self.progress.durable_events = accepted;
        self.durable_chain = self.chain;
        Ok(())
    }

    pub fn finish(&mut self, terminal: DraftTerminal) -> Result<DraftProgress, DraftError> {
        self.finish_with(
            terminal,
            |file, bytes| file.write_all(bytes),
            File::sync_all,
        )
    }

    fn finish_with(
        &mut self,
        terminal: DraftTerminal,
        write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
        sync: impl FnOnce(&File) -> io::Result<()>,
    ) -> Result<DraftProgress, DraftError> {
        let result = (|| -> Result<(), StorageError> {
            self.check_open()?;
            if let Some(group) = self.group {
                group.finish(terminal)?;
            } else {
                validate_terminal(self.previous, terminal)?;
            }
            let record = terminal_record(terminal, self.chain);
            if let Err(error) = write(&mut self.file, &record).and_then(|()| sync(&self.file)) {
                self.failed = true;
                return Err(error.into());
            }
            self.chain.copy_from_slice(&record[PAYLOAD_BYTES..]);
            self.progress.durable_terminal = Some(terminal);
            Ok(())
        })();
        result
            .map(|()| self.progress)
            .map_err(|source| self.error(source))
    }

    fn check_open(&self) -> Result<(), StorageError> {
        if self.failed {
            Err(StorageError::Invalid(
                "automation draft writer failed; preserve file and reopen for recovery",
            ))
        } else if self.progress.durable_terminal.is_some() {
            Err(StorageError::Invalid(
                "automation draft is already terminal",
            ))
        } else {
            Ok(())
        }
    }
    fn error(&self, source: StorageError) -> DraftError {
        DraftError {
            path: self.path.clone(),
            progress: self.progress,
            published: true,
            source: Box::new(source),
        }
    }
}

/// Immutable identity of the exact fully validated completed container. The
/// terminal chain covers the original header and every preceding record. This
/// detects changed content; it is not a signature or native publication lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AutomationContentIdentity {
    version: u32,
    file_bytes: u64,
    terminal_chain: [u8; 32],
}
impl AutomationContentIdentity {
    pub fn version(self) -> u32 {
        self.version
    }
    pub fn file_bytes(self) -> u64 {
        self.file_bytes
    }
    pub fn terminal_chain(self) -> [u8; 32] {
        self.terminal_chain
    }
}

/// The only fixed-record I/O owner, shared by full recovery and lean preparation.
/// File close/unlock belongs on control/worker after native callback quiescence.
struct RecordReader {
    file: File,
    path: PathBuf,
    progress: DraftProgress,
    records_start: u64,
    header_chain: [u8; 32],
}

pub struct DraftReader {
    records: RecordReader,
    source: Project,
    token: FileToken,
    target: DraftTarget,
    provenance: DraftProvenance,
    capture_clock: Option<CaptureClock>,
    outcome: DraftOutcome,
    complete_identity: Option<AutomationContentIdentity>,
}

impl DraftReader {
    /// Obtain a shared lock and validate the full file in one fixed record buffer.
    /// EOF without a terminal is interrupted; damaged tails retain their valid
    /// prefix. Header damage/future versions refuse before exposing source state.
    pub fn open(path: impl AsRef<Path>, limits: DraftReadLimits) -> Result<Self, DraftError> {
        let path = path.as_ref();
        Self::open_inner(path, limits, None).map_err(|source| DraftError {
            path: path.to_path_buf(),
            progress: DraftProgress::default(),
            published: true,
            source: Box::new(source),
        })
    }
    /// Accept a validated prefix of at most `max_events`, inspecting one further
    /// record to detect excess events, a terminal, or a damaged tail.
    pub fn open_bounded(
        path: impl AsRef<Path>,
        limits: DraftReadLimits,
        max_events: u64,
    ) -> Result<Self, DraftError> {
        let path = path.as_ref();
        Self::open_inner(path, limits, Some(max_events)).map_err(|source| DraftError {
            path: path.to_path_buf(),
            progress: DraftProgress::default(),
            published: true,
            source: Box::new(source),
        })
    }
    fn open_inner(
        path: &Path,
        limits: DraftReadLimits,
        max_events: Option<u64>,
    ) -> Result<Self, StorageError> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(StorageError::Invalid(
                "automation draft must be a regular non-symbolic file",
            ));
        }
        let mut file = File::open(path)?;
        fs2::FileExt::try_lock_shared(&file).map_err(|error| {
            if error.raw_os_error().is_some()
                && error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
            {
                StorageError::AutomationDraftBusy(error)
            } else {
                StorageError::Io(error)
            }
        })?;
        let mut prefix = [0; PREFIX_BYTES];
        file.read_exact(&mut prefix)?;
        if &prefix[..8] != MAGIC || prefix[12..16] != [0; 4] {
            return Err(StorageError::Invalid("invalid automation draft header"));
        }
        let version = u32_at(&prefix, 8);
        if !matches!(
            version,
            VERSION | CLOCK_VERSION | VST3_CLOCK_VERSION | VST3_GROUP_VERSION
        ) {
            return Err(StorageError::Invalid(
                "unsupported automation draft version; preserve original file and open it with a compatible hex build",
            ));
        }
        let source_len = u64_at(&prefix, 16);
        let meta_len = u64_at(&prefix, 24);
        if source_len > limits.max_source_bytes || meta_len > MAX_METADATA_BYTES {
            return Err(StorageError::Invalid(
                "automation draft header exceeds caller read budget",
            ));
        }
        let total = source_len
            .checked_add(meta_len)
            .and_then(|n| n.checked_add(PREFIX_BYTES as u64 + 32))
            .ok_or(StorageError::Invalid(
                "automation draft header size overflow",
            ))?;
        if total > file.metadata()?.len() {
            return Err(StorageError::Invalid(
                "truncated automation draft source header",
            ));
        }
        let source_bytes = read_sized(&mut file, source_len)?;
        let meta_bytes = read_sized(&mut file, meta_len)?;
        let mut hash = Sha256::new();
        hash.update(prefix);
        hash.update(&source_bytes);
        hash.update(&meta_bytes);
        let mut chain = [0; 32];
        file.read_exact(&mut chain)?;
        let expected: [u8; 32] = hash.finalize().into();
        if chain != expected {
            return Err(StorageError::Invalid(
                "automation draft source integrity failure",
            ));
        }
        let source = decode(&source_bytes)?;
        validate_source(&source)?;
        let (target, provenance, capture_clock) = if version == VST3_GROUP_VERSION {
            let (target, provenance, clock) = GroupHeader::decode(&meta_bytes, &source)?;
            (DraftTarget::Vst3Group(target), provenance, Some(clock))
        } else {
            let (meta, capture_clock) = if version == VERSION {
                (serde_json::from_slice::<Metadata>(&meta_bytes)?, None)
            } else {
                let clocked: ClockedMetadata = serde_json::from_slice(&meta_bytes)?;
                let clock = CaptureClock::try_from(clocked.capture_clock)?;
                validate_capture_clock(&source, clocked.metadata.provenance, clock)?;
                (clocked.metadata, Some(clock))
            };
            let target = resolve_target(&source, &meta, version)?;
            (target, meta.provenance, capture_clock)
        };
        validate_provenance(provenance)?;
        // Keep the source file bytes untouched. Identity uses the owning codec's
        // canonical current representation after any explicitly supported
        // migration, so a later project schema need not strand an older draft.
        let token = FileToken::from_validated_bytes(encode(&source)?.into());
        let mut reader = Self {
            records: RecordReader {
                file,
                path: path.to_path_buf(),
                progress: DraftProgress::default(),
                records_start: total,
                header_chain: chain,
            },
            source,
            token,
            target,
            provenance,
            capture_clock,
            outcome: DraftOutcome::Interrupted,
            complete_identity: None,
        };
        let mut validator = RecordValidation::new(target, capture_clock)?;
        loop {
            let mut record = [0; RECORD_BYTES];
            let count = read_record(&mut reader.records.file, &mut record)?;
            if count == 0 {
                break;
            }
            let damage = if count != RECORD_BYTES {
                Some(DraftDamage::TruncatedRecord)
            } else if !valid_hash(&record, chain) {
                Some(DraftDamage::Integrity)
            } else {
                None
            };
            if let Some(reason) = damage {
                reader.outcome = DraftOutcome::Damaged { reason };
                break;
            }
            match validator.inspect(&record) {
                Ok(Record::Event(_) | Record::GroupEvent) => {
                    if max_events.is_some_and(|max| reader.records.progress.durable_events >= max) {
                        return Err(StorageError::Invalid(
                            "automation draft exceeds caller event budget; preserve original file",
                        ));
                    }
                    let Some(count) = reader.records.progress.durable_events.checked_add(1) else {
                        reader.outcome = DraftOutcome::Damaged {
                            reason: DraftDamage::InvalidRecord,
                        };
                        break;
                    };
                    reader.records.progress.durable_events = count;
                    reader.records.progress.accepted_events = count;
                    chain.copy_from_slice(&record[PAYLOAD_BYTES..]);
                }
                Ok(Record::Terminal(terminal)) => {
                    reader.records.progress.durable_terminal = Some(terminal);
                    reader.outcome = match terminal {
                        DraftTerminal::Finished(clock) => DraftOutcome::Finished(clock),
                        DraftTerminal::Aborted(reason) => DraftOutcome::Aborted(reason),
                        DraftTerminal::Interrupted => DraftOutcome::Interrupted,
                    };
                    let mut extra = [0; 1];
                    if read_record(&mut reader.records.file, &mut extra)? != 0 {
                        reader.records.progress.durable_terminal = None;
                        reader.outcome = DraftOutcome::Damaged {
                            reason: DraftDamage::TrailingData,
                        };
                    } else if matches!(terminal, DraftTerminal::Finished(_)) {
                        let mut terminal_chain = [0; 32];
                        terminal_chain.copy_from_slice(&record[PAYLOAD_BYTES..]);
                        reader.complete_identity = Some(AutomationContentIdentity {
                            version,
                            file_bytes: reader.records.file.stream_position()?,
                            terminal_chain,
                        });
                    }
                    break;
                }
                _ => {
                    reader.outcome = DraftOutcome::Damaged {
                        reason: DraftDamage::InvalidRecord,
                    };
                    break;
                }
            }
        }
        Ok(reader)
    }
    pub fn source(&self) -> &Project {
        &self.source
    }
    pub fn target(&self) -> DraftTarget {
        self.target
    }
    pub fn provenance(&self) -> DraftProvenance {
        self.provenance
    }
    pub fn capture_clock(&self) -> Option<CaptureClock> {
        self.capture_clock
    }
    pub fn progress(&self) -> DraftProgress {
        self.records.progress
    }
    pub fn outcome(&self) -> DraftOutcome {
        self.outcome
    }
    pub fn path(&self) -> &Path {
        &self.records.path
    }
    /// Revalidate and compare the writer's acknowledged prefix under the reader lock.
    /// Work is proportional to the identity's event count and bounded by the opened prefix.
    pub fn matches_prefix(&mut self, identity: DraftPrefixIdentity) -> Result<bool, DraftError> {
        let result = (|| -> Result<bool, StorageError> {
            if identity.events > self.records.progress.durable_events {
                return Ok(false);
            }
            self.records
                .file
                .seek(SeekFrom::Start(self.records.records_start))?;
            let mut chain = self.records.header_chain;
            let mut validator = RecordValidation::new(self.target, self.capture_clock)?;
            for _ in 0..identity.events {
                let mut record = [0; RECORD_BYTES];
                self.records.file.read_exact(&mut record)?;
                if !valid_hash(&record, chain) {
                    return Err(StorageError::Invalid(
                        "automation draft prefix integrity changed",
                    ));
                }
                if !matches!(
                    validator.inspect(&record)?,
                    Record::Event(_) | Record::GroupEvent
                ) {
                    return Err(StorageError::Invalid("automation draft prefix changed"));
                }
                chain.copy_from_slice(&record[PAYLOAD_BYTES..]);
            }
            Ok(chain == identity.chain)
        })();
        result.map_err(|source| self.error(source))
    }
    /// Compare complete strict canonical project encoding, not volatile identity.
    pub fn source_matches(&self, source: &Project) -> Result<bool, DraftError> {
        encode(source)
            .map(|bytes| bytes.as_slice() == self.token.0.as_ref())
            .map_err(|source| self.error(source))
    }
    /// Read a caller-owned ordinal page from the validated prefix. Unused slots
    /// are cleared. There is no retained event list/index and no duration cap.
    /// Shared locks exclude cooperating writers; external noncooperating writes
    /// are outside this lock contract and require opening/validating a new reader.
    pub fn read_events(
        &mut self,
        first_ordinal: u64,
        output: &mut [Option<AppliedEvent>],
    ) -> Result<usize, DraftError> {
        if matches!(self.target, DraftTarget::Vst3Group(_)) {
            output.fill(None);
            return Err(self.error(StorageError::Invalid(
                "grouped draft requires grouped reads",
            )));
        }
        self.records.read_events(first_ordinal, output)
    }

    /// Read only from the full validated group prefix under the same lock.
    pub fn read_group_events(
        &mut self,
        first_ordinal: u64,
        output: &mut [Option<GroupDraftEvent>],
    ) -> Result<usize, DraftError> {
        let DraftTarget::Vst3Group(group) = self.target else {
            output.fill(None);
            return Err(self.error(StorageError::Invalid(
                "single target draft has no grouped records",
            )));
        };
        self.records
            .read_page(first_ordinal, output, |record, ordinal| {
                let event = group::decode(record)?;
                if event.ordinal.checked_sub(1) != Some(ordinal)
                    || group.ids().binary_search(&event.parameter).is_err()
                {
                    return Err(StorageError::Invalid(
                        "grouped automation draft page identity changed",
                    ));
                }
                Ok(event)
            })
    }

    fn error(&self, source: StorageError) -> DraftError {
        self.records.error(source)
    }
}

impl RecordReader {
    pub fn read_events(
        &mut self,
        first_ordinal: u64,
        output: &mut [Option<AppliedEvent>],
    ) -> Result<usize, DraftError> {
        let mut previous = None;
        self.read_page(first_ordinal, output, |record, _| {
            let Record::Event(event) = decode_record(record)? else {
                return Err(StorageError::Invalid("automation draft prefix changed"));
            };
            validate_event(previous, event)?;
            previous = Some(event);
            Ok(event)
        })
    }

    fn read_page<T: Copy>(
        &mut self,
        first_ordinal: u64,
        output: &mut [Option<T>],
        mut decode: impl FnMut(&[u8; RECORD_BYTES], u64) -> Result<T, StorageError>,
    ) -> Result<usize, DraftError> {
        output.fill(None);
        let result = (|| -> Result<usize, StorageError> {
            if first_ordinal > self.progress.durable_events {
                return Err(StorageError::Invalid(
                    "automation draft ordinal exceeds validated prefix",
                ));
            }
            let count =
                (self.progress.durable_events - first_ordinal).min(output.len() as u64) as usize;
            let offset = first_ordinal
                .checked_mul(RECORD_BYTES as u64)
                .and_then(|n| self.records_start.checked_add(n))
                .ok_or(StorageError::Invalid("automation draft ordinal overflow"))?;
            if count == 0 {
                return Ok(0);
            }
            let mut chain = self.header_chain;
            if first_ordinal != 0 {
                let predecessor =
                    offset
                        .checked_sub(RECORD_BYTES as u64)
                        .ok_or(StorageError::Invalid(
                            "automation draft predecessor overflow",
                        ))?;
                self.file.seek(SeekFrom::Start(predecessor))?;
                let mut record = [0; RECORD_BYTES];
                self.file.read_exact(&mut record)?;
                decode(&record, first_ordinal - 1)?;
                chain.copy_from_slice(&record[PAYLOAD_BYTES..]);
            }
            self.file.seek(SeekFrom::Start(offset))?;
            for (index, slot) in output[..count].iter_mut().enumerate() {
                let mut record = [0; RECORD_BYTES];
                self.file.read_exact(&mut record)?;
                if !valid_hash(&record, chain) {
                    return Err(StorageError::Invalid(
                        "automation draft page integrity changed; preserve original file",
                    ));
                }
                *slot = Some(decode(&record, first_ordinal + index as u64)?);
                chain.copy_from_slice(&record[PAYLOAD_BYTES..]);
            }
            Ok(count)
        })();
        if result.is_err() {
            output.fill(None);
        }
        result.map_err(|source| self.error(source))
    }
    fn error(&self, source: StorageError) -> DraftError {
        DraftError {
            path: self.path.clone(),
            progress: self.progress,
            published: true,
            source: Box::new(source),
        }
    }
}

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}
impl From<CaptureClock> for ClockMetadata {
    fn from(clock: CaptureClock) -> Self {
        Self {
            sample_rate: clock.sample_rate,
            loop_region: match clock.loop_region {
                Some(region) => LoopMetadata::Region {
                    start: region.start(),
                    end: region.end(),
                },
                None => LoopMetadata::Linear {},
            },
        }
    }
}
impl TryFrom<ClockMetadata> for CaptureClock {
    type Error = StorageError;

    fn try_from(clock: ClockMetadata) -> Result<Self, Self::Error> {
        Ok(Self {
            sample_rate: clock.sample_rate,
            loop_region: match clock.loop_region {
                LoopMetadata::Linear {} => None,
                LoopMetadata::Region { start, end } => Some(
                    hex_project::LoopRegion::new(start, end, true)
                        .map_err(|_| StorageError::Invalid("invalid automation capture loop"))?,
                ),
            },
        })
    }
}
fn validate_capture_clock(
    source: &Project,
    provenance: DraftProvenance,
    clock: CaptureClock,
) -> Result<(), StorageError> {
    if clock.sample_rate != source.sample_rate() || clock.sample_rate != provenance.sample_rate {
        return Err(StorageError::Invalid(
            "automation capture clock rate must match source and provenance",
        ));
    }
    if clock.loop_region.is_some_and(|region| !region.enabled()) {
        return Err(StorageError::Invalid(
            "automation capture loop must be enabled or explicitly linear",
        ));
    }
    Ok(())
}
fn validate_source(source: &Project) -> Result<(), StorageError> {
    if source.assets().any(|asset| !asset.path().is_absolute()) {
        return Err(StorageError::Invalid(
            "automation draft source media paths must be absolute",
        ));
    }
    Ok(())
}
fn validate_provenance(p: DraftProvenance) -> Result<(), StorageError> {
    if p.sample_rate == 0
        || p.output_identity == 0
        || p.editor_revision == 0
        || p.audio_revision == 0
        || p.transport_epoch == 0
        || p.gesture_serial == 0
    {
        return Err(StorageError::Invalid("invalid automation draft provenance"));
    }
    Ok(())
}
fn validate_event(previous: Option<AppliedEvent>, event: AppliedEvent) -> Result<(), StorageError> {
    if event.sequence == 0 {
        return Err(StorageError::Invalid(
            "automation draft sequence must be nonzero",
        ));
    }
    AutomationValue::new(event.value.get())
        .map_err(|_| StorageError::Invalid("invalid automation draft value"))?;
    if let Some(previous) = previous
        && (event.sequence <= previous.sequence || !clock_follows(clock(previous), clock(event)))
    {
        return Err(StorageError::Invalid(
            "automation draft sequence or clocks are out of order",
        ));
    }
    Ok(())
}
fn clock(event: AppliedEvent) -> DraftClock {
    DraftClock {
        performance_frame: event.performance_frame,
        content_frame: event.content_frame,
        loop_pass: event.loop_pass,
    }
}
fn clock_follows(previous: DraftClock, next: DraftClock) -> bool {
    next.performance_frame >= previous.performance_frame
        && next.loop_pass >= previous.loop_pass
        && (next.loop_pass > previous.loop_pass || next.content_frame >= previous.content_frame)
}
fn validate_terminal(
    previous: Option<AppliedEvent>,
    terminal: DraftTerminal,
) -> Result<(), StorageError> {
    if let (Some(previous), DraftTerminal::Finished(next)) = (previous, terminal)
        && !clock_follows(clock(previous), next)
    {
        return Err(StorageError::Invalid(
            "automation draft terminal clocks are out of order",
        ));
    }
    Ok(())
}
fn read_sized(file: &mut File, count: u64) -> Result<Vec<u8>, StorageError> {
    let count = usize::try_from(count)
        .map_err(|_| StorageError::Invalid("automation draft header exceeds address space"))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(count).map_err(io::Error::other)?;
    bytes.resize(count, 0);
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn read_record(file: &mut File, record: &mut [u8]) -> io::Result<usize> {
    let mut read = 0;
    while read < record.len() {
        match file.read(&mut record[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(read)
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    let mut value = [0; 8];
    value.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(value)
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    let mut value = [0; 4];
    value.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(value)
}
fn put(record: &mut [u8], field: usize, value: u64) {
    record[field * 8..field * 8 + 8].copy_from_slice(&value.to_le_bytes());
}
fn seal(mut record: [u8; RECORD_BYTES], chain: [u8; 32]) -> [u8; RECORD_BYTES] {
    let mut hash = Sha256::new();
    hash.update(chain);
    hash.update(&record[..PAYLOAD_BYTES]);
    record[PAYLOAD_BYTES..].copy_from_slice(&hash.finalize());
    record
}
fn valid_hash(record: &[u8; RECORD_BYTES], chain: [u8; 32]) -> bool {
    seal(*record, chain)[PAYLOAD_BYTES..] == record[PAYLOAD_BYTES..]
}
fn event_record(event: AppliedEvent, chain: [u8; 32]) -> [u8; RECORD_BYTES] {
    let mut record = [0; RECORD_BYTES];
    put(&mut record, 0, 1);
    put(&mut record, 1, event.sequence);
    put(&mut record, 2, event.performance_frame);
    put(&mut record, 3, event.content_frame);
    put(&mut record, 4, event.loop_pass);
    put(&mut record, 5, u64::from(event.value.get().to_bits()));
    seal(record, chain)
}
fn abort_code(reason: DraftAbortReason) -> u64 {
    match reason {
        DraftAbortReason::Cancelled => 1,
        DraftAbortReason::SourceChanged => 2,
        DraftAbortReason::TargetUnavailable => 3,
        DraftAbortReason::TransportChanged => 4,
        DraftAbortReason::OutputFailed => 5,
        DraftAbortReason::UnsupportedOutput => 6,
        DraftAbortReason::ClockExhausted => 7,
    }
}
fn terminal_record(terminal: DraftTerminal, chain: [u8; 32]) -> [u8; RECORD_BYTES] {
    let mut record = [0; RECORD_BYTES];
    match terminal {
        DraftTerminal::Finished(c) => {
            put(&mut record, 0, 2);
            put(&mut record, 2, c.performance_frame);
            put(&mut record, 3, c.content_frame);
            put(&mut record, 4, c.loop_pass);
        }
        DraftTerminal::Aborted(reason) => {
            put(&mut record, 0, 3);
            put(&mut record, 6, abort_code(reason));
        }
        DraftTerminal::Interrupted => put(&mut record, 0, 4),
    }
    seal(record, chain)
}
enum Record {
    Event(AppliedEvent),
    GroupEvent,
    Terminal(DraftTerminal),
}

/// Shared validator for the initial scan and later durable prefix verification.
struct RecordValidation {
    previous: Option<AppliedEvent>,
    group: Option<GroupState>,
}
impl RecordValidation {
    fn new(target: DraftTarget, clock: Option<CaptureClock>) -> Result<Self, StorageError> {
        let group = if let DraftTarget::Vst3Group(target) = target {
            let clock =
                clock.ok_or(StorageError::Invalid("grouped automation clock is missing"))?;
            Some(GroupState::new(target, clock))
        } else {
            None
        };
        Ok(Self {
            previous: None,
            group,
        })
    }
    fn inspect(&mut self, bytes: &[u8; RECORD_BYTES]) -> Result<Record, StorageError> {
        if let Some(group) = self.group.as_mut()
            && matches!(bytes[0], 5 | 6)
        {
            group.validate(group::decode(bytes)?)?;
            return Ok(Record::GroupEvent);
        }
        let record = decode_record(bytes)?;
        match record {
            Record::Event(event) if self.group.is_none() => {
                validate_event(self.previous, event)?;
                self.previous = Some(event);
            }
            Record::Terminal(terminal) => {
                if let Some(group) = self.group {
                    group.finish(terminal)?;
                } else {
                    validate_terminal(self.previous, terminal)?;
                }
            }
            _ => {
                return Err(StorageError::Invalid(
                    "automation draft record differs from its format",
                ));
            }
        }
        Ok(record)
    }
}
fn decode_record(record: &[u8; RECORD_BYTES]) -> Result<Record, StorageError> {
    let invalid = || StorageError::Invalid("invalid automation draft record");
    if u64_at(record, 56) != 0 {
        return Err(invalid());
    }
    let kind = u64_at(record, 0);
    if kind != 1 && (u64_at(record, 8) != 0 || u64_at(record, 40) != 0) {
        return Err(invalid());
    }
    if kind != 3 && u64_at(record, 48) != 0 {
        return Err(invalid());
    }
    if matches!(kind, 3 | 4) && record[8..48].iter().any(|b| *b != 0) {
        return Err(invalid());
    }
    match kind {
        1 => {
            let bits = u32::try_from(u64_at(record, 40)).map_err(|_| invalid())?;
            let value = AutomationValue::new(f32::from_bits(bits)).map_err(|_| invalid())?;
            if value.get().to_bits() != bits {
                return Err(invalid());
            }
            Ok(Record::Event(AppliedEvent {
                sequence: u64_at(record, 8),
                performance_frame: u64_at(record, 16),
                content_frame: u64_at(record, 24),
                loop_pass: u64_at(record, 32),
                value,
            }))
        }
        2 => Ok(Record::Terminal(DraftTerminal::Finished(DraftClock {
            performance_frame: u64_at(record, 16),
            content_frame: u64_at(record, 24),
            loop_pass: u64_at(record, 32),
        }))),
        3 => {
            let reason = match u64_at(record, 48) {
                1 => DraftAbortReason::Cancelled,
                2 => DraftAbortReason::SourceChanged,
                3 => DraftAbortReason::TargetUnavailable,
                4 => DraftAbortReason::TransportChanged,
                5 => DraftAbortReason::OutputFailed,
                6 => DraftAbortReason::UnsupportedOutput,
                7 => DraftAbortReason::ClockExhausted,
                _ => return Err(invalid()),
            };
            Ok(Record::Terminal(DraftTerminal::Aborted(reason)))
        }
        4 => Ok(Record::Terminal(DraftTerminal::Interrupted)),
        _ => Err(invalid()),
    }
}

#[cfg(test)]
mod tests;
