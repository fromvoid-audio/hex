//! Owned-path control-side audio batches, with bounded polling and cancellation.
//! Decode and conversion calls are not internally interruptible. No UI, worker
//! protocol, callback, source-file mutation or partial project event lives here.
use crate::{PreparedImport, Session, SessionError, import_batch::AudioBatchDestination};
use hex_media::{Limits, ResampleLimits, decode_wav_file};
use hex_project::{Created, Project};
use std::{
    fmt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
};

const FILE_BYTES: usize = 64 * 1024 * 1024;
const SOURCE_FRAMES: usize = 8_000_000;
const RETAINED_FRAMES: u64 = 8_000_000;
const OPEN: u8 = 0;
const CANCEL_REQUESTED: u8 = 1;
const COMMITTING: u8 = 2;
const COMPLETED: u8 = 3;
const CANCELED: u8 = 4;
const FAILED: u8 = 5;

/// Worker-owned identity, captured at drop and rechecked before commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioBatchContext {
    pub project_generation: u64,
    pub revision: u64,
    pub active_gesture: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioBatchPhase {
    Queued,
    Decoding,
    Converting,
    Prepared,
    Compiling,
    Committing,
    Completed,
    Canceled,
    Failed,
}

/// One atomic phase/member observation. Member numbers are one-based while
/// decoding/converting; completed counts denote fully prepared members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioBatchProgress {
    pub phase: AudioBatchPhase,
    pub member: usize,
    pub completed_members: usize,
    pub total_members: usize,
    pub cancellation_requested: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioBatchCancel {
    Requested,
    AlreadyRequested,
    TooLate,
    Finished,
}

/// Contains only fixed-size atomics, never paths, PCM or project ownership.
/// Polling uses bounded atomic loads; cancellation uses one CAS and never waits.
/// Phase and counts are atomic together. Commit/cancel arbitration is separate;
/// an intermediate poll may straddle a phase transition. Terminal polls after
/// completion observe the final member counts via acquire/release publication.
#[derive(Debug)]
pub struct AudioBatchControl {
    state: AtomicU8,
    progress: AtomicU64,
    total: usize,
}
impl AudioBatchControl {
    pub(crate) fn new(total: usize) -> Result<Self, AudioBatchError> {
        if total == 0 || total > hex_project::MAX_ASSETS {
            return Err(SessionError::Invalid("audio batch must contain 1..4096 members").into());
        }
        Ok(Self {
            state: AtomicU8::new(OPEN),
            progress: AtomicU64::new(0),
            total,
        })
    }
    pub(crate) fn refuse(&self) {
        // A rejection must never rewrite a prior accepted operation's outcome.
        let _ = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| match state {
                OPEN => Some(FAILED),
                CANCEL_REQUESTED => Some(CANCELED),
                _ => None,
            });
    }

    pub fn request_cancel(&self) -> AudioBatchCancel {
        match self.state.compare_exchange(
            OPEN,
            CANCEL_REQUESTED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => AudioBatchCancel::Requested,
            Err(CANCEL_REQUESTED) => AudioBatchCancel::AlreadyRequested,
            Err(COMMITTING | COMPLETED) => AudioBatchCancel::TooLate,
            Err(_) => AudioBatchCancel::Finished,
        }
    }
    pub fn progress(&self) -> AudioBatchProgress {
        let state = self.state.load(Ordering::Acquire);
        let packed = self.progress.load(Ordering::Acquire);
        let phase = match state {
            COMMITTING => AudioBatchPhase::Committing,
            COMPLETED => AudioBatchPhase::Completed,
            CANCELED => AudioBatchPhase::Canceled,
            FAILED => AudioBatchPhase::Failed,
            _ => match packed & 7 {
                1 => AudioBatchPhase::Decoding,
                2 => AudioBatchPhase::Converting,
                3 => AudioBatchPhase::Prepared,
                4 => AudioBatchPhase::Compiling,
                _ => AudioBatchPhase::Queued,
            },
        };
        AudioBatchProgress {
            phase,
            member: ((packed >> 3) & 8191) as usize,
            completed_members: (packed >> 16) as usize,
            total_members: self.total,
            cancellation_requested: state == CANCEL_REQUESTED || state == CANCELED,
        }
    }
    fn publish(&self, phase: u64, member: usize, completed: usize) {
        self.progress.store(
            phase | ((member as u64) << 3) | ((completed as u64) << 16),
            Ordering::Release,
        );
    }
    fn check_cancel(&self) -> Result<(), AudioBatchError> {
        if self.state.load(Ordering::Acquire) == CANCEL_REQUESTED {
            Err(AudioBatchError::Canceled)
        } else {
            Ok(())
        }
    }
    fn claim_commit(&self) -> Result<(), SessionError> {
        self.state
            .compare_exchange(OPEN, COMMITTING, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| SessionError::Invalid("audio batch canceled before commit"))
    }
}

#[derive(Debug)]
pub enum AudioBatchError {
    Canceled,
    ContextChanged,
    Session(SessionError),
    Member {
        member: usize,
        path: PathBuf,
        source: SessionError,
    },
}
impl fmt::Display for AudioBatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Canceled => f.write_str("audio batch canceled before commit"),
            Self::ContextChanged => {
                f.write_str("audio batch canceled · project or gesture changed")
            }
            Self::Session(source) => write!(f, "audio batch refused: {source}"),
            Self::Member {
                member,
                path,
                source,
            } => write!(
                f,
                "audio batch refused at file {member} · {}: {source}",
                path.display()
            ),
        }
    }
}
impl std::error::Error for AudioBatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Session(source) | Self::Member { source, .. } => Some(source),
            _ => None,
        }
    }
}
impl From<SessionError> for AudioBatchError {
    fn from(error: SessionError) -> Self {
        Self::Session(error)
    }
}

/// One owned operation. Consuming `run` prevents duplicate execution. Staged PCM
/// stays on its calling control worker and only escapes as a committed revision.
/// A successful result is final; later journal/publication errors cannot roll it
/// back. Callers must report them separately and offer the single canonical Undo.
pub struct AudioBatchImport {
    paths: Vec<PathBuf>,
    destination: AudioBatchDestination,
    context: AudioBatchContext,
    project: Project,
    resident_frames: u64,
    resident_assets: usize,
    max_resident_frames: u64,
    source_epoch: u64,
    control: Arc<AudioBatchControl>,
}
impl AudioBatchImport {
    /// Capture owned payload order and actual Session capacities without I/O.
    pub fn capture(
        paths: Vec<PathBuf>,
        destination: AudioBatchDestination,
        session: &Session,
        context: AudioBatchContext,
    ) -> Result<Self, AudioBatchError> {
        let control = Arc::new(AudioBatchControl::new(paths.len())?);
        Self::capture_with_control(paths, destination, session, context, control)
    }

    pub(crate) fn capture_with_control(
        paths: Vec<PathBuf>,
        destination: AudioBatchDestination,
        session: &Session,
        context: AudioBatchContext,
        control: Arc<AudioBatchControl>,
    ) -> Result<Self, AudioBatchError> {
        if context.active_gesture {
            return Err(AudioBatchError::ContextChanged);
        }
        let count = paths.len();
        if count != control.total {
            return Err(SessionError::Invalid("audio batch control count differs").into());
        }
        control.check_cancel()?;
        match control.state.load(Ordering::Acquire) {
            OPEN => {}
            CANCEL_REQUESTED | CANCELED => return Err(AudioBatchError::Canceled),
            _ => return Err(SessionError::Invalid("audio batch control already used").into()),
        }
        if count == 0
            || count > hex_project::MAX_ASSETS
            || session
                .media
                .len()
                .checked_add(count)
                .is_none_or(|n| n > hex_project::MAX_ASSETS)
        {
            return Err(
                SessionError::Invalid("audio batch exceeds retained asset capacity").into(),
            );
        }
        match destination {
            AudioBatchDestination::Track { track, .. } => {
                session
                    .project()
                    .track(track)
                    .ok_or(SessionError::Invalid("track no longer exists"))?;
            }
            AudioBatchDestination::NewTracks { before_track, .. } => {
                if session
                    .project()
                    .tracks()
                    .len()
                    .checked_add(count)
                    .is_none_or(|n| n > hex_project::MAX_TRACKS)
                {
                    return Err(SessionError::Invalid("track limit reached").into());
                }
                if before_track.is_some_and(|id| session.project().track(id).is_none()) {
                    return Err(
                        SessionError::Invalid("track insertion anchor no longer exists").into(),
                    );
                }
            }
        }
        Ok(Self {
            control,
            paths,
            destination,
            context,
            project: session.project().clone(),
            resident_frames: session.resident_frames(),
            resident_assets: session.media.len(),
            max_resident_frames: session.max_resident_frames(),
            source_epoch: session.semantic_source_epoch,
        })
    }
    pub fn control(&self) -> Arc<AudioBatchControl> {
        Arc::clone(&self.control)
    }

    /// Run on the canonical control worker. `current` supplies actual current
    /// generation/revision/gesture state, not an optimistic UI projection.
    /// Cancellation is checked at member/phase boundaries and atomically
    /// arbitrated immediately before backend commit. A running decode/resample
    /// finishes before cancellation can be observed; no latency bound is claimed.
    pub fn run(
        mut self,
        session: &mut Session,
        previous: Option<&hex_engine::PreparedRevision>,
        mut current: impl FnMut() -> AudioBatchContext,
    ) -> Result<(Vec<Created>, Arc<hex_engine::PreparedRevision>), AudioBatchError> {
        let result = self.run_inner(session, previous, &mut current);
        match &result {
            Ok(_) => {
                self.control.publish(0, self.paths.len(), self.paths.len());
                self.control.state.store(COMPLETED, Ordering::Release);
            }
            Err(AudioBatchError::Canceled) => self.control.state.store(CANCELED, Ordering::Release),
            Err(_) => self.control.state.store(FAILED, Ordering::Release),
        }
        result
    }
    fn check(
        &self,
        current: &mut impl FnMut() -> AudioBatchContext,
    ) -> Result<(), AudioBatchError> {
        self.control.check_cancel()?;
        if current() != self.context {
            return Err(AudioBatchError::ContextChanged);
        }
        // The caller can request cancellation while obtaining current context.
        self.control.check_cancel()
    }
    fn run_inner(
        &mut self,
        session: &mut Session,
        previous: Option<&hex_engine::PreparedRevision>,
        current: &mut impl FnMut() -> AudioBatchContext,
    ) -> Result<(Vec<Created>, Arc<hex_engine::PreparedRevision>), AudioBatchError> {
        self.check(current)?;
        if session.project() != &self.project
            || session.resident_frames() != self.resident_frames
            || session.media.len() != self.resident_assets
            || session.max_resident_frames() != self.max_resident_frames
            || session.semantic_source_epoch != self.source_epoch
        {
            return Err(AudioBatchError::ContextChanged);
        }
        if session.audio_mode() == crate::DocumentAudioMode::VerifiedStreaming {
            return self.run_verified(session, previous, current);
        }
        let mut remaining = self
            .max_resident_frames
            .min(RETAINED_FRAMES)
            .checked_sub(self.resident_frames)
            .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
        let mut prepared = Vec::new();
        prepared
            .try_reserve_exact(self.paths.len())
            .map_err(|_| SessionError::Allocation)?;
        for (index, original) in self.paths.iter().enumerate() {
            self.check(current)?;
            if remaining == 0 {
                return Err(SessionError::Invalid("resident audio frame limit reached").into());
            }
            let path = if original.is_absolute() {
                original.clone()
            } else {
                std::env::current_dir()
                    .map_err(|e| AudioBatchError::Member {
                        member: index + 1,
                        path: original.clone(),
                        source: SessionError::Media(hex_media::ImportError::Io(e)),
                    })?
                    .join(original)
            };
            self.control.publish(1, index + 1, index);
            self.check(current)?;
            let decoded =
                decode_wav_file(path, Limits::new(FILE_BYTES, SOURCE_FRAMES)).map_err(|e| {
                    AudioBatchError::Member {
                        member: index + 1,
                        path: original.clone(),
                        source: SessionError::Media(e),
                    }
                })?;
            self.check(current)?;
            self.control.publish(2, index + 1, index);
            self.check(current)?;
            let member = PreparedImport::at_rate(
                decoded,
                self.project.sample_rate(),
                ResampleLimits {
                    max_source_frames: SOURCE_FRAMES,
                    max_output_frames: usize::try_from(remaining).map_err(|_| {
                        SessionError::Invalid("resident frame limit exceeds addressable range")
                    })?,
                },
            )
            .map_err(|source| AudioBatchError::Member {
                member: index + 1,
                path: original.clone(),
                source,
            })?;
            self.check(current)?;
            remaining = remaining
                .checked_sub(u64::try_from(member.waveform.source_frames).map_err(|_| {
                    SessionError::Invalid("decoded frame count exceeds project range")
                })?)
                .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
            prepared.push(member);
            self.control.publish(3, index + 1, index + 1);
        }
        self.check(current)?;
        self.control.publish(4, self.paths.len(), self.paths.len());
        // The canonical backend admits twice: before candidate construction and
        // after its single compile, immediately before history/media commit.
        let mut admissions = 0;
        let mut admission_error = None;
        let result = session.import_prepared_batch_revision(
            prepared,
            self.destination,
            &self.project,
            previous,
            || {
                if let Err(error) = self.check(current) {
                    admission_error = Some(error);
                    return Err(SessionError::Invalid(
                        "audio batch canceled · context or cancellation changed",
                    ));
                }
                admissions += 1;
                if admissions == 2 {
                    if let Err(error) = self.control.claim_commit() {
                        admission_error = Some(AudioBatchError::Canceled);
                        return Err(error);
                    }
                }
                Ok(())
            },
        );
        match result {
            Ok(value) => Ok(value),
            Err(_) if self.control.state.load(Ordering::Acquire) == CANCEL_REQUESTED => {
                Err(AudioBatchError::Canceled)
            }
            Err(error) => Err(admission_error.unwrap_or_else(|| error.into())),
        }
    }
    fn run_verified(
        &self,
        session: &mut Session,
        previous: Option<&hex_engine::PreparedRevision>,
        current: &mut impl FnMut() -> AudioBatchContext,
    ) -> Result<(Vec<Created>, Arc<hex_engine::PreparedRevision>), AudioBatchError> {
        let mut sources = Vec::new();
        sources
            .try_reserve_exact(self.paths.len())
            .map_err(|_| SessionError::Allocation)?;
        for (index, original) in self.paths.iter().enumerate() {
            self.check(current)?;
            let path = if original.is_absolute() {
                original.clone()
            } else {
                std::env::current_dir()
                    .map_err(|e| AudioBatchError::Member {
                        member: index + 1,
                        path: original.clone(),
                        source: SessionError::Media(hex_media::ImportError::Io(e)),
                    })?
                    .join(original)
            };
            self.control.publish(1, index + 1, index);
            let inspected = session.verify_import_with_pending(path, &sources, || {
                self.control.check_cancel().is_err()
            });
            self.check(current)?;
            let source = inspected.map_err(|source| AudioBatchError::Member {
                member: index + 1,
                path: original.clone(),
                source,
            })?;
            sources.push(source);
            self.control.publish(3, index + 1, index + 1);
        }
        self.check(current)?;
        self.control.publish(4, self.paths.len(), self.paths.len());
        let mut admissions = 0;
        let mut admission_error = None;
        let result = session.import_verified_batch_revision(
            sources,
            self.destination,
            &self.project,
            previous,
            || {
                if let Err(error) = self.check(current) {
                    admission_error = Some(error);
                    return Err(SessionError::Invalid("audio batch context changed"));
                }
                admissions += 1;
                if admissions == 2 {
                    self.control.claim_commit()?;
                }
                Ok(())
            },
        );
        match result {
            Ok(value) => Ok(value),
            Err(_) if self.control.state.load(Ordering::Acquire) == CANCEL_REQUESTED => {
                Err(AudioBatchError::Canceled)
            }
            Err(error) => Err(admission_error.unwrap_or_else(|| error.into())),
        }
    }
}

#[cfg(test)]
mod tests;
