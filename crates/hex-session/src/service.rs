//! Bounded control worker for decoding, project edits, saves, and graph compilation.
use crate::{PreparedImport, Session, SessionError};
use hex_engine::{PreparedRevision, Project as PlaybackProject, playback::Playback};
use hex_export::ExportReport;
use hex_media::{Limits, ResampleLimits, decode_wav_file, waveform::Waveform};
use hex_project::{Command, Created, Project};
use hex_storage::FileToken;
use std::{
    collections::BTreeMap,
    fmt, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
};

const REQUEST_CAPACITY: usize = 32;
const EVENT_CAPACITY: usize = 2;
const IMPORT_FILE_LIMIT: usize = 64 * 1024 * 1024;
const IMPORT_FRAME_LIMIT: usize = 8_000_000;
const BROWSER_PREVIEW_BINS: usize = 48;
const EXPORT_EVENT_CAPACITY: usize = 1;

/// Read-only model and cached summaries for a single committed project revision.
pub struct ViewState {
    pub project: Arc<Project>,
    pub waveforms: BTreeMap<hex_project::AssetId, Arc<Waveform>>,
    /// Control-prepared MIDI clip summaries matching this project revision.
    pub midi_preview: Arc<hex_engine::MidiArrangementPreview>,
    pub timeline_frames: u64,
    pub can_undo: bool,
    pub can_redo: bool,
    pub revision: u64,
    /// True after any successful model change until save or project replacement.
    /// Undo stays dirty conservatively even when it restores an earlier snapshot.
    pub unsaved: bool,
    /// Lexical absolute path associated with the current document, if saved.
    pub document_path: Option<PathBuf>,
}

/// A worker response. `playback` is present only when a new graph was compiled.
pub struct Event {
    pub state: Arc<ViewState>,
    pub playback: Option<Playback>,
    pub status: Option<String>,
    pub error: Option<String>,
    pub request_complete: bool,
    /// True for each processed Save As or Save Current request, including refusal.
    pub save_complete: bool,
    pub open_complete: bool,
    pub project_replaced: bool,
    pub created: Option<Created>,
    pub duplicate_complete: bool,
    /// Generation echoed for each processed restore request, including errors.
    pub restore_complete: Option<u64>,
}

/// Export completion is separate from canonical view events, so an older render
/// can never reinstall stale project, transport, or dirty state.
pub struct ExportEvent {
    pub revision: u64,
    pub path: PathBuf,
    pub result: Result<ExportReport, String>,
}

/// Bounded audio metadata prepared away from the UI for browser drag feedback.
#[derive(Clone, Debug)]
pub struct AudioPreview {
    pub playback_frames: u64,
    pub sample_rate: u32,
    pub waveform: Arc<Waveform>,
}

/// Decode and convert an audio source using the same limits and conversion path
/// as import, then retain only its bounded waveform summary and playback length.
pub fn prepare_audio_preview(
    path: impl AsRef<Path>,
    project_sample_rate: u32,
) -> Result<AudioPreview, String> {
    let decoded = decode_wav_file(path, Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT))
        .map_err(|error| error.to_string().to_lowercase())?;
    let audio = hex_media::resample(
        decoded.audio(),
        project_sample_rate,
        ResampleLimits {
            max_source_frames: IMPORT_FRAME_LIMIT,
            max_output_frames: IMPORT_FRAME_LIMIT,
        },
    )
    .map_err(|error| error.to_string().to_lowercase())?;
    let playback_frames = u64::try_from(audio.frames().len())
        .map_err(|_| "converted frame count exceeds project range".to_owned())?;
    let waveform = Arc::new(
        hex_media::waveform::summarize(&audio, BROWSER_PREVIEW_BINS)
            .map_err(|error| error.to_string().to_lowercase())?,
    );
    Ok(AudioPreview {
        playback_frames,
        sample_rate: project_sample_rate,
        waveform,
    })
}

/// Bounded requests accepted by the control worker.
#[derive(Debug)]
pub enum Request {
    Import(PathBuf),
    ImportToTrack {
        path: PathBuf,
        track: hex_project::TrackId,
        start: u64,
    },
    ImportToTrackWithOverwrite {
        path: PathBuf,
        track: hex_project::TrackId,
        start: u64,
    },
    ImportToNewTrack {
        path: PathBuf,
        before_track: Option<hex_project::TrackId>,
        start: u64,
    },
    ImportToSession {
        path: PathBuf,
        track: hex_project::TrackId,
        scene: hex_project::SceneId,
    },
    Open(PathBuf),
    Edit(Command),
    CopyTimeRange(hex_project::TimeRange),
    CutTimeRange(hex_project::TimeRange),
    PasteTimeRange {
        track: hex_project::TrackId,
        frame: u64,
    },
    InsertTimeRange {
        track: hex_project::TrackId,
        frame: u64,
    },
    Undo,
    Redo,
    SaveNew(PathBuf),
    SaveCurrent,
    RestorePlayback {
        generation: u64,
    },
    Export(PathBuf),
}

/// A rejected request is returned intact so the caller can retry or report it.
#[derive(Debug)]
pub enum SubmitError {
    Full(Request),
    Closed(Request),
    Busy(Request),
}

#[derive(Debug)]
pub enum ServiceError {
    Session(SessionError),
    Worker(io::Error),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(error) => write!(f, "could not create project session: {error}"),
            Self::Worker(error) => write!(f, "could not start project worker: {error}"),
        }
    }
}

impl std::error::Error for ServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Session(error) => Some(error),
            Self::Worker(error) => Some(error),
        }
    }
}

/// UI-owned endpoints. Dropping both channels detaches the worker without joining it.
pub struct Service {
    requests: SyncSender<Request>,
    events: Receiver<Event>,
    export_events: Receiver<ExportEvent>,
    export_busy: Arc<AtomicBool>,
    _worker: JoinHandle<()>,
    _export_worker: JoinHandle<()>,
}

type Exporter =
    Arc<dyn Fn(&Path, &PlaybackProject, u64, u64) -> Result<ExportReport, String> + Send + Sync>;

struct ExportJob {
    revision: u64,
    path: PathBuf,
    project: Arc<PlaybackProject>,
    frames: u64,
}

enum ExportWork {
    Render(ExportJob),
}

impl Service {
    pub fn new(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
    ) -> Result<Self, ServiceError> {
        Self::with_exporter(
            sample_rate,
            history_capacity,
            max_resident_frames,
            Arc::new(|path, project, start, frames| {
                hex_export::render_new(path, project, start, frames)
                    .map_err(|error| error.to_string().to_lowercase())
            }),
        )
    }

    fn with_exporter(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
        exporter: Exporter,
    ) -> Result<Self, ServiceError> {
        let session = Session::new(sample_rate, history_capacity, max_resident_frames)
            .map_err(ServiceError::Session)?;
        let (request_tx, request_rx) = mpsc::sync_channel(REQUEST_CAPACITY);
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_CAPACITY);
        let (export_work_tx, export_work_rx) = mpsc::sync_channel(1);
        let (export_event_tx, export_event_rx) = mpsc::sync_channel(EXPORT_EVENT_CAPACITY);
        let actor_export_events = export_event_tx.clone();
        let export_busy = Arc::new(AtomicBool::new(false));
        let export_worker = thread::Builder::new()
            .name("hex-export".into())
            .spawn(move || run_export_worker(export_work_rx, export_event_tx, exporter))
            .map_err(ServiceError::Worker)?;
        let worker = thread::Builder::new()
            .name("hex-session".into())
            .spawn(move || {
                run_worker(
                    session,
                    history_capacity,
                    request_rx,
                    event_tx,
                    export_work_tx,
                    actor_export_events,
                )
            })
            .map_err(ServiceError::Worker)?;
        Ok(Self {
            requests: request_tx,
            events: event_rx,
            export_events: export_event_rx,
            export_busy,
            _worker: worker,
            _export_worker: export_worker,
        })
    }

    /// Submit without waiting for worker capacity.
    pub fn submit(&self, request: Request) -> Result<(), SubmitError> {
        if matches!(&request, Request::Export(_)) {
            if self
                .export_busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(SubmitError::Busy(request));
            }
            match try_submit(&self.requests, request) {
                Ok(()) => Ok(()),
                Err(error) => {
                    self.export_busy.store(false, Ordering::Release);
                    Err(error)
                }
            }
        } else {
            try_submit(&self.requests, request)
        }
    }

    /// Poll for one committed state or operation result without waiting.
    pub fn try_event(&self) -> Result<Event, TryRecvError> {
        self.events.try_recv()
    }

    /// Consume one separate export result and release the single-job reservation.
    pub fn try_export_event(&self) -> Result<ExportEvent, TryRecvError> {
        match self.export_events.try_recv() {
            Ok(event) => {
                self.export_busy.store(false, Ordering::Release);
                Ok(event)
            }
            Err(TryRecvError::Disconnected) => {
                self.export_busy.store(false, Ordering::Release);
                Err(TryRecvError::Disconnected)
            }
            Err(TryRecvError::Empty) => Err(TryRecvError::Empty),
        }
    }
}

fn try_submit(sender: &SyncSender<Request>, request: Request) -> Result<(), SubmitError> {
    match sender.try_send(request) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(request)) => Err(SubmitError::Full(request)),
        Err(TrySendError::Disconnected(request)) => Err(SubmitError::Closed(request)),
    }
}

fn view_state(
    session: &Session,
    midi_preview: Arc<hex_engine::MidiArrangementPreview>,
    revision: u64,
    unsaved: bool,
    document_path: Option<&Path>,
) -> Arc<ViewState> {
    let project = Arc::new(session.project().clone());
    let rate = u64::from(project.sample_rate());
    let latest_clip_end = latest_clip_end(&project);
    let latest_content_end = latest_clip_end.max(
        project
            .loop_region()
            .map(|region| region.end())
            .unwrap_or(0),
    );
    Arc::new(ViewState {
        project,
        waveforms: session.waveforms(),
        midi_preview,
        timeline_frames: latest_content_end
            .saturating_add(rate.saturating_mul(2))
            .max(rate.saturating_mul(8)),
        can_undo: session.can_undo(),
        can_redo: session.can_redo(),
        revision,
        unsaved,
        document_path: document_path.map(Path::to_path_buf),
    })
}

struct DocumentHandle {
    path: PathBuf,
    token: FileToken,
}

struct SaveCurrentCommit {
    dirty: bool,
    status: &'static str,
    error: Option<String>,
}

fn apply_current_save_outcome(
    document: &mut DocumentHandle,
    outcome: hex_storage::ReplaceOutcome,
) -> SaveCurrentCommit {
    document.path = outcome.path.clone();
    document.token = outcome.token;
    match outcome.warning {
        Some(warning) => SaveCurrentCommit {
            dirty: true,
            status: "project was published, but durability could not be confirmed",
            error: Some(format!(
                "complete project was published at {}, but could not {}: {}; recovery copy is at {}",
                outcome.path.display(),
                warning.operation,
                warning.source,
                outcome.recovery_path.display()
            )),
        },
        None => SaveCurrentCommit {
            dirty: false,
            status: "project saved",
            error: None,
        },
    }
}

fn compile(
    session: &Session,
    previous: Option<&PreparedRevision>,
) -> Result<Arc<PreparedRevision>, String> {
    session
        .compile_prepared_revision(previous)
        .map_err(|error| error.to_string().to_lowercase())
}

fn native_playback(prepared: Arc<PreparedRevision>) -> Playback {
    match Playback::try_from_prepared_with_session(Arc::clone(&prepared)) {
        Ok(playback) => playback,
        Err(error) => Playback::from_prepared_with_session_unavailable(prepared, error),
    }
}

fn send_event(sender: &SyncSender<Event>, event: Event) -> bool {
    sender.send(event).is_ok()
}

fn run_worker(
    mut session: Session,
    history_capacity: usize,
    requests: Receiver<Request>,
    events: SyncSender<Event>,
    export_work: SyncSender<ExportWork>,
    export_events: SyncSender<ExportEvent>,
) {
    let mut revision = 0_u64;
    let mut unsaved = false;
    let mut range_clipboard: Option<Arc<hex_project::RangeClipboard>> = None;
    // Media paths remain relative to the directory from which the current
    // project was opened. Save As changes the document handle, not this base.
    let mut media_base: Option<PathBuf> = None;
    let mut document: Option<DocumentHandle> = None;
    let mut midi_preview = Arc::new(hex_engine::MidiArrangementPreview::default());
    let mut state = view_state(&session, Arc::clone(&midi_preview), revision, unsaved, None);
    let mut compiled: Option<(u64, Arc<PreparedRevision>)> = None;
    let initial = match compile(&session, None) {
        Ok(prepared) => {
            midi_preview = Arc::clone(prepared.midi_preview());
            state = view_state(&session, Arc::clone(&midi_preview), revision, unsaved, None);
            compiled = Some((revision, Arc::clone(&prepared)));
            Event {
                state: Arc::clone(&state),
                playback: Some(native_playback(prepared)),
                status: Some("project ready".into()),
                error: None,
                request_complete: false,
                save_complete: false,
                open_complete: false,
                project_replaced: false,
                created: None,
                duplicate_complete: false,
                restore_complete: None,
            }
        }
        Err(error) => Event {
            state: Arc::clone(&state),
            playback: None,
            status: None,
            error: Some(error),
            request_complete: false,
            save_complete: false,
            open_complete: false,
            project_replaced: false,
            created: None,
            duplicate_complete: false,
            restore_complete: None,
        },
    };
    if !send_event(&events, initial) {
        return;
    }

    while let Ok(request) = requests.recv() {
        let mut open_complete = false;
        let mut save_complete = false;
        let mut project_replaced = false;
        let mut replacement_revision = None;
        let mut created = None;
        let mut duplicate_complete = false;
        let mut restore_complete = None;
        let mut restore_prepared = None;
        let mut status = None;
        let mut error = None;
        let mut export_completion = None;
        let mut export_job = None;
        match request {
            Request::Import(path) => match import(
                &mut session,
                path,
                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
            ) {
                Ok(prepared) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("audio imported".into());
                }
                Err(reason) => error = Some(reason),
            },
            Request::ImportToTrack { path, track, start } => match import_to_track(
                &mut session,
                path,
                track,
                start,
                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                false,
            ) {
                Ok(prepared) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("audio imported to selected track".into());
                }
                Err(reason) => error = Some(reason),
            },
            Request::ImportToTrackWithOverwrite { path, track, start } => match import_to_track(
                &mut session,
                path,
                track,
                start,
                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                true,
            ) {
                Ok(prepared) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("audio imported, overlapping clips cut".into());
                }
                Err(reason) => error = Some(reason),
            },
            Request::ImportToNewTrack {
                path,
                before_track,
                start,
            } => match import_to_new_track(
                &mut session,
                path,
                before_track,
                start,
                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
            ) {
                Ok(prepared) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("audio imported to new track".into());
                }
                Err(reason) => error = Some(reason),
            },
            Request::ImportToSession { path, track, scene } => match import_to_session(
                &mut session,
                path,
                track,
                scene,
                compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
            ) {
                Ok(prepared) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("audio source imported to session".into());
                }
                Err(reason) => error = Some(reason),
            },
            Request::Open(path) => {
                open_complete = true;
                match open_project(&session, history_capacity, &path) {
                    Ok((candidate, candidate_base, candidate_path, candidate_token)) => {
                        match compile(
                            &candidate,
                            compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                        ) {
                            Ok(prepared) => {
                                session = candidate;
                                media_base = Some(candidate_base);
                                document = Some(DocumentHandle {
                                    path: candidate_path,
                                    token: candidate_token,
                                });
                                unsaved = false;
                                project_replaced = true;
                                replacement_revision = Some(prepared);
                                status = Some("project opened".into());
                            }
                            Err(reason) => error = Some(reason),
                        }
                    }
                    Err(reason) => error = Some(reason),
                }
            }
            Request::CopyTimeRange(range) => match session.project().capture_time_range(&range) {
                Ok(captured) => {
                    range_clipboard = Some(Arc::new(captured));
                    status = Some("time selection copied".into());
                }
                Err(reason) => error = Some(reason.to_string()),
            },
            Request::CutTimeRange(range) => match session.project().capture_time_range(&range) {
                Ok(captured) => {
                    match session.command_prepared_revision(
                        Command::DeleteTimeRange { range },
                        compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    ) {
                        Ok((_, prepared)) => {
                            range_clipboard = Some(Arc::new(captured));
                            if session.project() != state.project.as_ref() {
                                unsaved = true;
                                replacement_revision = Some(prepared);
                            }
                            status = Some("time selection cut".into());
                        }
                        Err(reason) => error = Some(reason.to_string()),
                    }
                }
                Err(reason) => error = Some(reason.to_string()),
            },
            Request::PasteTimeRange { track, frame } => {
                if let Some(clipboard) = &range_clipboard {
                    match session.command_prepared_revision(
                        Command::PasteTimeRange {
                            clipboard: Arc::clone(clipboard),
                            track,
                            frame,
                        },
                        compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    ) {
                        Ok((_, prepared)) => {
                            if session.project() != state.project.as_ref() {
                                unsaved = true;
                                replacement_revision = Some(prepared);
                            }
                            status = Some("time selection pasted".into());
                        }
                        Err(reason) => error = Some(reason.to_string()),
                    }
                } else {
                    error = Some("time clipboard is empty · copy a selection first".into());
                }
            }
            Request::InsertTimeRange { track, frame } => {
                if let Some(clipboard) = &range_clipboard {
                    match session.command_prepared_revision(
                        Command::InsertTimeRange {
                            clipboard: Arc::clone(clipboard),
                            track,
                            frame,
                        },
                        compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    ) {
                        Ok((_, prepared)) => {
                            if session.project() != state.project.as_ref() {
                                unsaved = true;
                                replacement_revision = Some(prepared);
                            }
                            status = Some("time selection inserted".into());
                        }
                        Err(reason) => error = Some(reason.to_string()),
                    }
                } else {
                    error = Some("time clipboard is empty · copy a selection first".into());
                }
            }
            Request::Edit(command) => {
                duplicate_complete = matches!(&command, Command::DuplicateClip(_));
                let before = session.project().clone();
                match session.command_prepared_revision(
                    command,
                    compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                ) {
                    Ok((result, prepared)) if session.project() != &before => {
                        unsaved = true;
                        replacement_revision = Some(prepared);
                        created = Some(result);
                        status = Some("edit committed".into());
                    }
                    Ok(_) => status = Some("no change".into()),
                    Err(reason) => error = Some(reason.to_string().to_lowercase()),
                }
            }
            Request::Undo => match session
                .undo_prepared_revision(compiled.as_ref().map(|(_, artifact)| artifact.as_ref()))
            {
                Ok(Some(prepared)) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("undo committed".into());
                }
                Ok(None) => status = Some("nothing to undo".into()),
                Err(reason) => error = Some(reason.to_string().to_lowercase()),
            },
            Request::Redo => match session
                .redo_prepared_revision(compiled.as_ref().map(|(_, artifact)| artifact.as_ref()))
            {
                Ok(Some(prepared)) => {
                    unsaved = true;
                    replacement_revision = Some(prepared);
                    status = Some("redo committed".into());
                }
                Ok(None) => status = Some("nothing to redo".into()),
                Err(reason) => error = Some(reason.to_string().to_lowercase()),
            },
            Request::SaveNew(path) => {
                save_complete = true;
                match project_for_save(&session, media_base.as_deref()) {
                    Ok(project) => match absolute_document_path(&path) {
                        Ok(path) => match hex_storage::save_new_with_token(&path, &project) {
                            Ok(token) => {
                                document = Some(DocumentHandle { path, token });
                                unsaved = false;
                                state = view_state(
                                    &session,
                                    Arc::clone(&midi_preview),
                                    revision,
                                    unsaved,
                                    document.as_ref().map(|handle| handle.path.as_path()),
                                );
                                status = Some("project saved".into());
                            }
                            Err(reason) => error = Some(reason.to_string().to_lowercase()),
                        },
                        Err(reason) => error = Some(reason),
                    },
                    Err(reason) => error = Some(reason),
                }
            }
            Request::SaveCurrent => {
                save_complete = true;
                match (
                    document.as_mut(),
                    project_for_save(&session, media_base.as_deref()),
                ) {
                    (None, _) => {
                        error = Some("cannot save current project before it has a file path".into())
                    }
                    (_, Err(reason)) => error = Some(reason),
                    (Some(handle), Ok(project)) => {
                        match hex_storage::replace_existing(&handle.path, &handle.token, &project) {
                            Ok(outcome) => {
                                let commit = apply_current_save_outcome(handle, outcome);
                                unsaved = commit.dirty;
                                status = Some(commit.status.into());
                                error = commit.error;
                                state = view_state(
                                    &session,
                                    Arc::clone(&midi_preview),
                                    revision,
                                    unsaved,
                                    document.as_ref().map(|current| current.path.as_path()),
                                );
                            }
                            Err(reason) => error = Some(reason.to_string().to_lowercase()),
                        }
                    }
                }
            }
            Request::Export(path) => {
                status = Some("export queued".into());
                let clip_end = compiled
                    .as_ref()
                    .and_then(|(_, prepared)| prepared.arrangement().audible_end_frame())
                    .unwrap_or(0);
                let job = match (compiled.as_ref(), clip_end) {
                    (_, 0) => Err("nothing to export".to_owned()),
                    (Some((compiled_revision, prepared)), frames)
                        if *compiled_revision == revision =>
                    {
                        Ok(ExportJob {
                            revision,
                            path: path.clone(),
                            project: Arc::clone(prepared.arrangement()),
                            frames,
                        })
                    }
                    _ => Err("latest project revision has not compiled".to_owned()),
                };
                match job {
                    Ok(job) => export_job = Some(job),
                    Err(reason) => {
                        export_completion = Some(ExportEvent {
                            revision,
                            path,
                            result: Err(reason),
                        });
                    }
                }
            }
            Request::RestorePlayback { generation } => {
                restore_complete = Some(generation);
                match compiled.as_ref() {
                    Some((compiled_revision, prepared)) if *compiled_revision == revision => {
                        restore_prepared = Some(Arc::clone(prepared));
                    }
                    _ => match compile(
                        &session,
                        compiled.as_ref().map(|(_, artifact)| artifact.as_ref()),
                    ) {
                        Ok(prepared) => {
                            compiled = Some((revision, Arc::clone(&prepared)));
                            midi_preview = Arc::clone(prepared.midi_preview());
                            state = view_state(
                                &session,
                                Arc::clone(&midi_preview),
                                revision,
                                unsaved,
                                document.as_ref().map(|handle| handle.path.as_path()),
                            );
                            restore_prepared = Some(prepared);
                        }
                        Err(reason) => {
                            compiled = None;
                            error = Some(reason);
                        }
                    },
                }
            }
        }

        if project_replaced {
            range_clipboard = None;
        }
        let playback = if let Some(prepared) = replacement_revision {
            revision = revision.saturating_add(1);
            midi_preview = Arc::clone(prepared.midi_preview());
            state = view_state(
                &session,
                Arc::clone(&midi_preview),
                revision,
                unsaved,
                document.as_ref().map(|handle| handle.path.as_path()),
            );
            compiled = Some((revision, Arc::clone(&prepared)));
            Some(native_playback(prepared))
        } else {
            restore_prepared.map(native_playback)
        };
        if !send_event(
            &events,
            Event {
                state: Arc::clone(&state),
                playback,
                status,
                error,
                request_complete: true,
                save_complete,
                open_complete,
                project_replaced,
                created,
                duplicate_complete,
                restore_complete,
            },
        ) {
            return;
        }
        if let Some(job) = export_job {
            match export_work.try_send(ExportWork::Render(job)) {
                Ok(()) => {}
                Err(TrySendError::Full(ExportWork::Render(job)))
                | Err(TrySendError::Disconnected(ExportWork::Render(job))) => {
                    let _ = export_events.try_send(ExportEvent {
                        revision: job.revision,
                        path: job.path,
                        result: Err("export worker is unavailable".into()),
                    });
                }
            }
        }
        if let Some(event) = export_completion {
            let _ = export_events.try_send(event);
        }
    }
}

fn latest_clip_end(project: &Project) -> u64 {
    let audio_end = project
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .filter_map(|clip| clip.start().checked_add(clip.duration()))
        .max()
        .unwrap_or(0);
    let midi_end = project
        .tracks()
        .iter()
        .flat_map(|track| track.midi_clips())
        .filter_map(|clip| clip.start().raw().checked_add(clip.length().raw()))
        .filter_map(|raw| {
            project
                .tempo_map()
                .beat_to_frame(hex_project::BeatTime::from_raw(raw))
                .ok()
        })
        .max()
        .unwrap_or(0);
    audio_end.max(midi_end)
}

fn run_export_worker(
    work: Receiver<ExportWork>,
    events: SyncSender<ExportEvent>,
    exporter: Exporter,
) {
    while let Ok(work) = work.recv() {
        let event = match work {
            ExportWork::Render(job) => ExportEvent {
                revision: job.revision,
                path: job.path.clone(),
                result: exporter(&job.path, &job.project, 0, job.frames),
            },
        };
        if events.send(event).is_err() {
            return;
        }
    }
}

fn open_project(
    current: &Session,
    history_capacity: usize,
    path: &Path,
) -> Result<(Session, PathBuf, PathBuf, FileToken), String> {
    let (project, token) =
        hex_storage::load_with_token(path).map_err(|error| error.to_string().to_lowercase())?;
    if project.sample_rate() != current.project().sample_rate() {
        return Err("project sample rate differs from the active audio device".into());
    }
    let project_dir = absolute_document_base(path)?;
    let mut prepared_assets = Vec::new();
    prepared_assets
        .try_reserve_exact(project.assets().count())
        .map_err(|_| "could not reserve decoded media table".to_owned())?;
    let mut remaining_frames = current.max_resident_frames();
    // Load unreferenced assets too: the editor can place them later, so missing
    // media in any asset record rejects the candidate instead of dropping it.
    for asset in project.assets() {
        let media_path = if asset.path().is_absolute() {
            asset.path().to_path_buf()
        } else {
            project_dir.join(asset.path())
        };
        if asset.playback_frames() > remaining_frames {
            return Err("project media exceeds resident audio frame limit".into());
        }
        let decoded = decode_wav_file(
            media_path,
            Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT),
        )
        .map_err(|error| error.to_string().to_lowercase())?;
        if decoded.sample_rate() != asset.source_sample_rate()
            || u64::try_from(decoded.frames()).ok() != Some(asset.source_frames())
        {
            return Err(format!(
                "media {} rate or frame count differs from project metadata",
                asset.path().display()
            ));
        }
        let prepared = PreparedImport::at_rate(
            decoded,
            project.sample_rate(),
            ResampleLimits {
                max_source_frames: IMPORT_FRAME_LIMIT,
                max_output_frames: usize::try_from(remaining_frames)
                    .map_err(|_| "resident frame limit exceeds addressable range".to_owned())?,
            },
        )
        .map_err(|error| error.to_string().to_lowercase())?;
        remaining_frames = remaining_frames
            .checked_sub(asset.playback_frames())
            .ok_or_else(|| "project media exceeds resident audio frame limit".to_owned())?;
        prepared_assets.push((asset.id(), prepared));
    }
    let session = Session::from_prepared_with_sine_table(
        project,
        history_capacity,
        current.max_resident_frames(),
        prepared_assets,
        current.sine_table(),
    )
    .map_err(|error| error.to_string().to_lowercase())?;
    let document_path = absolute_document_path(path)?;
    Ok((session, project_dir, document_path, token))
}

fn absolute_document_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|error| error.to_string().to_lowercase())
    }
}

fn absolute_document_base(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if parent.is_absolute() {
        Ok(parent.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(parent))
            .map_err(|error| error.to_string().to_lowercase())
    }
}

fn project_for_save(
    session: &Session,
    document_base: Option<&Path>,
) -> Result<hex_project::Project, String> {
    let mut snapshot = session.project().snapshot();
    for asset in &mut snapshot.assets {
        if !asset.path.is_absolute() {
            let base = document_base.ok_or_else(|| {
                "relative media path has no project directory; refusing ambiguous save".to_owned()
            })?;
            asset.path = base.join(&asset.path);
        }
    }
    hex_project::Project::from_snapshot(snapshot).map_err(|error| error.to_string().to_lowercase())
}

fn import(
    session: &mut Session,
    path: PathBuf,
    previous: Option<&PreparedRevision>,
) -> Result<Arc<PreparedRevision>, String> {
    let remaining_frames = session
        .max_resident_frames()
        .saturating_sub(session.resident_frames());
    let output_limit = usize::try_from(remaining_frames)
        .map_err(|_| "resident frame limit exceeds addressable range".to_owned())?;
    if output_limit == 0 {
        return Err("resident audio frame limit reached".into());
    }
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string().to_lowercase())?
            .join(path)
    };
    let decoded = decode_wav_file(path, Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT))
        .map_err(|error| error.to_string().to_lowercase())?;
    let prepared = PreparedImport::at_rate(
        decoded,
        session.project().sample_rate(),
        ResampleLimits {
            max_source_frames: IMPORT_FRAME_LIMIT,
            max_output_frames: output_limit,
        },
    )
    .map_err(|error| error.to_string().to_lowercase())?;
    session
        .import_prepared_revision(prepared, previous)
        .map(|(_, revision)| revision)
        .map_err(|error| error.to_string().to_lowercase())
}

fn import_to_track(
    session: &mut Session,
    path: PathBuf,
    track: hex_project::TrackId,
    start: u64,
    previous: Option<&PreparedRevision>,
    overwrite: bool,
) -> Result<Arc<PreparedRevision>, String> {
    let remaining_frames = session
        .max_resident_frames()
        .saturating_sub(session.resident_frames());
    let output_limit = usize::try_from(remaining_frames)
        .map_err(|_| "resident frame limit exceeds addressable range".to_owned())?;
    if output_limit == 0 {
        return Err("resident audio frame limit reached".into());
    }
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string().to_lowercase())?
            .join(path)
    };
    let decoded = decode_wav_file(path, Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT))
        .map_err(|error| error.to_string().to_lowercase())?;
    let prepared = PreparedImport::at_rate(
        decoded,
        session.project().sample_rate(),
        ResampleLimits {
            max_source_frames: IMPORT_FRAME_LIMIT,
            max_output_frames: output_limit,
        },
    )
    .map_err(|error| error.to_string().to_lowercase())?;
    let imported = if overwrite {
        session.import_prepared_audio_clip_with_overwrite_revision(prepared, track, start, previous)
    } else {
        session.import_prepared_audio_clip_revision(prepared, track, start, previous)
    };
    imported
        .map(|(_, revision)| revision)
        .map_err(|error| error.to_string().to_lowercase())
}

fn import_to_new_track(
    session: &mut Session,
    path: PathBuf,
    before_track: Option<hex_project::TrackId>,
    start: u64,
    previous: Option<&PreparedRevision>,
) -> Result<Arc<PreparedRevision>, String> {
    let project = session.project();
    if project.tracks().len() >= hex_project::MAX_TRACKS {
        return Err("track limit reached".into());
    }
    if before_track.is_some_and(|track| project.track(track).is_none()) {
        return Err("track insertion anchor no longer exists".into());
    }
    if project.assets().count() >= hex_project::MAX_ASSETS {
        return Err("asset limit reached".into());
    }
    let remaining_frames = session
        .max_resident_frames()
        .saturating_sub(session.resident_frames());
    let output_limit = usize::try_from(remaining_frames)
        .map_err(|_| "resident frame limit exceeds addressable range".to_owned())?;
    if output_limit == 0 {
        return Err("resident audio frame limit reached".into());
    }
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string().to_lowercase())?
            .join(path)
    };
    let decoded = decode_wav_file(path, Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT))
        .map_err(|error| error.to_string().to_lowercase())?;
    let prepared = PreparedImport::at_rate(
        decoded,
        session.project().sample_rate(),
        ResampleLimits {
            max_source_frames: IMPORT_FRAME_LIMIT,
            max_output_frames: output_limit,
        },
    )
    .map_err(|error| error.to_string().to_lowercase())?;
    session
        .import_prepared_audio_track_at_revision(prepared, before_track, start, previous)
        .map(|(_, revision)| revision)
        .map_err(|error| error.to_string().to_lowercase())
}

fn import_to_session(
    session: &mut Session,
    path: PathBuf,
    track: hex_project::TrackId,
    scene: hex_project::SceneId,
    previous: Option<&PreparedRevision>,
) -> Result<Arc<PreparedRevision>, String> {
    let remaining_frames = session
        .max_resident_frames()
        .saturating_sub(session.resident_frames());
    let output_limit = usize::try_from(remaining_frames)
        .map_err(|_| "resident frame limit exceeds addressable range".to_owned())?;
    if output_limit == 0 {
        return Err("resident audio frame limit reached".into());
    }
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string().to_lowercase())?
            .join(path)
    };
    let decoded = decode_wav_file(path, Limits::new(IMPORT_FILE_LIMIT, IMPORT_FRAME_LIMIT))
        .map_err(|error| error.to_string().to_lowercase())?;
    let prepared = PreparedImport::at_rate(
        decoded,
        session.project().sample_rate(),
        ResampleLimits {
            max_source_frames: IMPORT_FRAME_LIMIT,
            max_output_frames: output_limit,
        },
    )
    .map_err(|error| error.to_string().to_lowercase())?;
    session
        .import_session_audio_clip_revision(prepared, track, scene, previous)
        .map(|(_, revision)| revision)
        .map_err(|error| error.to_string().to_lowercase())
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use std::{
        fs,
        sync::{Barrier, atomic::AtomicU64, mpsc},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    struct TestDir(PathBuf);
    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    impl TestDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "hex-session-export-{}-{nonce}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create test directory");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn wav_mono(samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36_u32 + data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&96_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes
    }

    #[test]
    fn time_clipboard_is_worker_owned_cut_is_one_revision_and_open_clears_it() {
        let directory = TestDir::new();
        let path = directory.0.join("time.hex");
        let service = Service::new(48_000, 16, 0).expect("service");
        next_event(&service);
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "hybrid".into(),
            }))
            .expect("track");
        let track = next_event(&service).state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("length"),
            }))
            .expect("clip");
        let placed = next_event(&service);
        let range = hex_project::TimeRange::new(vec![track], 24_000, 72_000).expect("range");
        service
            .submit(Request::CopyTimeRange(range.clone()))
            .expect("copy");
        let copied = next_event(&service);
        assert!(copied.error.is_none());
        assert_eq!(copied.state.revision, placed.state.revision);
        assert!(copied.playback.is_none());
        service
            .submit(Request::InsertTimeRange {
                track,
                frame: 120_000,
            })
            .expect("insert from worker clipboard");
        let inserted = next_event(&service);
        assert!(inserted.error.is_none());
        assert_eq!(inserted.state.revision, copied.state.revision + 1);
        assert_eq!(inserted.state.project.tracks()[0].midi_clips().len(), 2);
        service.submit(Request::Undo).expect("undo insert");
        let insert_undone = next_event(&service);
        assert_eq!(insert_undone.state.revision, inserted.state.revision + 1);
        assert_eq!(
            insert_undone.state.project.tracks()[0].midi_clips().len(),
            1
        );
        let mut other =
            hex_project::Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
        other
            .execute(Command::AddTrack {
                name: "first".into(),
            })
            .expect("first");
        let missing = match other
            .execute(Command::AddTrack {
                name: "second".into(),
            })
            .expect("second")
        {
            Created::Track(id) => id,
            result => panic!("unexpected result {result:?}"),
        };
        service
            .submit(Request::CutTimeRange(
                hex_project::TimeRange::new(vec![missing], 0, 1).expect("shape"),
            ))
            .expect("refused cut");
        let refused_cut = next_event(&service);
        assert!(refused_cut.error.is_some());
        let mut refused_snapshot = refused_cut.state.project.snapshot();
        let copied_snapshot = copied.state.project.snapshot();
        refused_snapshot.next_id = copied_snapshot.next_id;
        assert_eq!(refused_snapshot, copied_snapshot);
        service
            .submit(Request::Open(directory.0.join("missing.hex")))
            .expect("refused open");
        let refused_open = next_event(&service);
        assert!(refused_open.error.is_some());
        assert!(!refused_open.project_replaced);
        service
            .submit(Request::PasteTimeRange {
                track,
                frame: 120_000,
            })
            .expect("retained clipboard");
        let retained = next_event(&service);
        assert!(retained.error.is_none());
        service.submit(Request::Undo).expect("undo retained paste");
        let restored = next_event(&service);
        service.submit(Request::CutTimeRange(range)).expect("cut");
        let cut = next_event(&service);
        assert!(cut.error.is_none());
        assert_eq!(cut.state.revision, restored.state.revision + 1);
        assert_eq!(cut.state.project.tracks()[0].midi_clips().len(), 2);
        service.submit(Request::Undo).expect("undo");
        assert_eq!(
            next_event(&service).state.project.tracks()[0]
                .midi_clips()
                .len(),
            1
        );
        service
            .submit(Request::PasteTimeRange {
                track,
                frame: 96_000,
            })
            .expect("paste");
        let pasted = next_event(&service);
        assert!(pasted.error.is_none());
        assert_eq!(pasted.state.project.tracks()[0].midi_clips().len(), 2);
        service
            .submit(Request::SaveNew(path.clone()))
            .expect("save");
        assert!(next_event(&service).error.is_none());
        service.submit(Request::Open(path)).expect("open");
        let opened = next_event(&service);
        assert!(opened.project_replaced);
        service
            .submit(Request::PasteTimeRange {
                track,
                frame: 192_000,
            })
            .expect("empty paste");
        let refused = next_event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.revision, opened.state.revision);
        assert_eq!(refused.state.project, opened.state.project);
    }

    #[test]
    fn browser_audio_preview_reports_converted_bounded_waveform_metadata() {
        let directory = TestDir::new();
        let path = directory.0.join("tone.WAV");
        fs::write(&path, wav_mono(&[4_000; 96])).expect("write wav fixture");

        let preview = prepare_audio_preview(&path, 24_000).expect("prepare preview");

        assert_eq!(preview.playback_frames, 48);
        assert_eq!(preview.sample_rate, 24_000);
        assert_eq!(preview.waveform.source_frames, 48);
        assert!(preview.waveform.bins.len() <= BROWSER_PREVIEW_BINS);
        assert!(!preview.waveform.bins.is_empty());
    }

    fn next_event(service: &Service) -> Event {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match service.try_event() {
                Ok(event) => return event,
                Err(TryRecvError::Empty) if Instant::now() < deadline => {
                    thread::yield_now();
                }
                Err(error) => panic!("service event did not arrive: {error}"),
            }
        }
    }

    fn next_export_event(service: &Service) -> ExportEvent {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match service.try_export_event() {
                Ok(event) => return event,
                Err(TryRecvError::Empty) if Instant::now() < deadline => {
                    thread::yield_now();
                }
                Err(error) => panic!("export event did not arrive: {error}"),
            }
        }
    }

    #[test]
    fn committed_save_warning_adopts_published_token_and_keeps_next_save_valid() {
        let directory = TestDir::new();
        let path = directory.0.join("CurrentCase.hex");
        let previous = Project::new(48_000).expect("initial project");
        let token = hex_storage::save_new_with_token(&path, &previous)
            .expect("create project and capture token");
        let mut editor = hex_project::Editor::new(previous, 8).expect("editor");
        editor
            .execute(Command::AddTrack {
                name: "published".into(),
            })
            .expect("edit project");

        let mut outcome = hex_storage::replace_existing(&path, &token, editor.project())
            .expect("commit replacement");
        let recovery_path = outcome.recovery_path.clone();
        outcome.warning = Some(hex_storage::ReplaceWarning {
            operation: "synchronize the project directory",
            source: io::Error::other("injected durability failure"),
        });
        let mut document = DocumentHandle {
            path: path.clone(),
            token,
        };
        let commit = apply_current_save_outcome(&mut document, outcome);

        assert!(commit.dirty);
        assert_eq!(
            commit.status,
            "project was published, but durability could not be confirmed"
        );
        let warning = commit.error.expect("visible durability warning");
        assert!(warning.contains(&format!(
            "complete project was published at {}",
            path.display()
        )));
        assert!(warning.contains("injected durability failure"));
        assert!(warning.contains(&format!("recovery copy is at {}", recovery_path.display())));
        assert!(document.token.check_unchanged(&path).is_ok());

        let next = hex_storage::replace_existing(&document.path, &document.token, editor.project())
            .expect("next Save Current must match the committed token");
        assert!(next.warning.is_none());
        assert_eq!(next.path, path);
    }

    #[test]
    fn export_keeps_edit_worker_responsive_and_returns_captured_revision_separately() {
        let directory = TestDir::new();
        let source = directory.0.join("source.wav");
        let output = directory.0.join("render.wav");
        fs::write(&source, wav_mono(&[16_384, 8_192])).expect("source fixture");
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let exporter: Exporter = {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            Arc::new(move |path, project, start, frames| {
                entered.wait();
                release.wait();
                hex_export::render_new(path, project, start, frames)
                    .map_err(|error| error.to_string().to_lowercase())
            })
        };
        let service = Service::with_exporter(48_000, 16, 8, exporter).expect("service");
        let _ = next_event(&service);
        service
            .submit(Request::Import(source))
            .expect("submit import");
        let imported = next_event(&service);
        let revision = imported.state.revision;
        let track = imported.state.project.tracks()[0].id();

        service
            .submit(Request::Export(output.clone()))
            .expect("submit export");
        let accepted = next_event(&service);
        assert!(accepted.request_complete);
        assert_eq!(accepted.state.revision, revision);
        entered.wait();

        assert!(matches!(
            service.submit(Request::Export(directory.0.join("busy.wav"))),
            Err(SubmitError::Busy(Request::Export(_)))
        ));
        service
            .submit(Request::Edit(Command::ToggleMute(track)))
            .expect("submit edit while rendering");
        let edited = next_event(&service);
        assert!(edited.request_complete);
        assert_eq!(edited.state.revision, revision + 1);
        assert!(edited.state.unsaved);
        assert!(edited.playback.is_some());

        release.wait();
        let completed = next_export_event(&service);
        assert_eq!(completed.revision, revision);
        assert_eq!(completed.path, output);
        assert!(completed.result.is_ok());
        let bytes = fs::read(completed.path).expect("read exported snapshot");
        assert_eq!(read_u16(&bytes, 44), 0x4000);
        assert_eq!(read_u16(&bytes, 46), 0x4000);
        assert_eq!(read_u16(&bytes, 48), 0x2000);
        assert_eq!(read_u16(&bytes, 50), 0x2000);
    }

    fn read_u16(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    }

    #[test]
    fn empty_export_returns_a_separate_refusal_completion() {
        let directory = TestDir::new();
        let service = Service::new(48_000, 8, 8).expect("service");
        let _ = next_event(&service);
        let path = directory.0.join("empty.wav");
        service
            .submit(Request::Export(path.clone()))
            .expect("submit export");
        let accepted = next_event(&service);
        assert!(accepted.request_complete);
        assert_eq!(accepted.state.revision, 0);
        let result = next_export_event(&service);
        assert_eq!(result.revision, 0);
        assert_eq!(result.path, path);
        assert_eq!(
            result.result.expect_err("empty project refusal"),
            "nothing to export"
        );
        assert!(!result.path.exists());
    }

    #[test]
    fn export_reservation_rolls_back_when_request_admission_fails() {
        let (requests, _request_receiver) = mpsc::sync_channel(0);
        let (_event_sender, events) = mpsc::sync_channel(1);
        let (_export_sender, export_events) = mpsc::sync_channel(1);
        let service = Service {
            requests,
            events,
            export_events,
            export_busy: Arc::new(AtomicBool::new(false)),
            _worker: thread::spawn(|| {}),
            _export_worker: thread::spawn(|| {}),
        };
        assert!(matches!(
            service.submit(Request::Export(PathBuf::from("busy.wav"))),
            Err(SubmitError::Full(Request::Export(_)))
        ));
        assert!(!service.export_busy.load(Ordering::Acquire));
    }

    #[test]
    fn midi_edge_resize_compiles_before_commit_and_undo_restores_source_marker() {
        let service = Service::new(48_000, 8, 0).expect("service");
        let _initial = next_event(&service);
        let beat = |n| hex_project::BeatTime::from_quarters(n).expect("small beat");
        service
            .submit(Request::Edit(Command::AddTrack {
                name: "keys".into(),
            }))
            .expect("track");
        let added = next_event(&service);
        let track = added.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: beat(4),
                length: beat(8),
            }))
            .expect("clip");
        let placed = next_event(&service);
        let clip = placed.state.project.tracks()[0].midi_clips()[0].id();
        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: beat(4),
                duration: beat(1),
                pitch: 60,
                channel: 0,
                velocity: 100,
            }))
            .expect("note");
        let before = next_event(&service);
        let original = before.state.project.midi_clip(clip).expect("clip").clone();
        service
            .submit(Request::Edit(Command::TrimMidiClipEdge {
                clip,
                edge: hex_project::TrimEdge::Left,
                beat: beat(7),
            }))
            .expect("resize");
        let resized = next_event(&service);
        assert!(resized.error.is_none());
        assert!(resized.state.revision > before.state.revision);
        assert!(
            resized
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .is_some()
        );
        let result = resized.state.project.midi_clip(clip).expect("clip");
        assert_eq!(result.start(), beat(7));
        assert_eq!(result.source_start(), beat(3));
        assert_eq!(result.notes(), original.notes());
        service.submit(Request::Undo).expect("undo");
        let undone = next_event(&service);
        assert!(undone.error.is_none());
        assert_eq!(undone.state.project.midi_clip(clip), Some(&original));
    }

    #[test]
    fn rejected_loop_expansion_preserves_committed_graph_revision_dirty_state_and_redo() {
        let service = Service::new(48_000, 8, 0).expect("service");
        let initial = next_event(&service);
        assert_eq!(initial.state.revision, 0);
        assert!(!initial.state.unsaved);
        assert!(initial.playback.is_some());

        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "keys".into(),
            }))
            .expect("add instrument track");
        let track_state = next_event(&service);
        let track = track_state.state.project.tracks()[0].id();

        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("arrangement length"),
            }))
            .expect("place midi clip");
        let clip_state = next_event(&service);
        let clip = clip_state.state.project.tracks()[0].midi_clips()[0].id();

        service
            .submit(Request::Edit(Command::SetMidiClipSource {
                clip,
                source_length: hex_project::BeatTime::from_raw(2),
                source_start: hex_project::BeatTime::from_raw(1),
                loop_start: hex_project::BeatTime::ZERO,
                loop_end: hex_project::BeatTime::from_raw(1),
                looping: false,
                length: hex_project::BeatTime::from_raw(65_538),
            }))
            .expect("set hidden one-shot source");
        let source_state = next_event(&service);
        assert!(source_state.error.is_none());

        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_raw(1),
                pitch: 60,
                channel: 0,
                velocity: 100,
            }))
            .expect("add source-local note hidden by source offset");
        let note_state = next_event(&service);
        assert!(note_state.error.is_none());

        service
            .submit(Request::Edit(Command::AddTrack {
                name: "redo branch".into(),
            }))
            .expect("add redo branch");
        let branch = next_event(&service);
        service.submit(Request::Undo).expect("undo branch edit");
        let before = next_event(&service);
        assert!(before.state.unsaved);
        assert!(before.state.can_redo);
        assert!(before.state.revision > branch.state.revision);
        let committed_artifact = Arc::clone(
            before
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("edit playback retains exact prepared revision"),
        );
        let saved_revision = before.state.revision;
        let saved_project = before.state.project.as_ref().clone();

        service
            .submit(Request::Edit(Command::SetMidiClipSource {
                clip,
                source_length: hex_project::BeatTime::from_raw(2),
                source_start: hex_project::BeatTime::ZERO,
                loop_start: hex_project::BeatTime::ZERO,
                loop_end: hex_project::BeatTime::from_raw(1),
                looping: true,
                length: hex_project::BeatTime::from_raw(65_538),
            }))
            .expect("submit over-budget local loop");
        let refused = next_event(&service);
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|error| error.ends_with("compiled midi occurrence limit reached"))
        );
        assert_eq!(refused.state.revision, saved_revision);
        assert_eq!(refused.state.project.as_ref(), &saved_project);
        assert!(refused.state.unsaved);
        assert!(refused.state.can_redo);
        assert!(refused.playback.is_none());

        service
            .submit(Request::RestorePlayback { generation: 7 })
            .expect("restore prior committed playback");
        let restored = next_event(&service);
        assert_eq!(restored.restore_complete, Some(7));
        assert_eq!(restored.state.revision, saved_revision);
        let restored_artifact = restored
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("restore returns committed artifact");
        assert!(Arc::ptr_eq(&committed_artifact, restored_artifact));

        service
            .submit(Request::Redo)
            .expect("redo survives refusal");
        let redone = next_event(&service);
        assert_eq!(redone.state.revision, saved_revision + 1);
        assert!(!redone.state.can_redo);
        assert_eq!(
            redone.state.project.tracks().len(),
            saved_project.tracks().len() + 1
        );
    }

    #[test]
    fn prepared_midi_preview_tracks_edits_history_open_and_metadata_only_events() {
        let directory = TestDir::new();
        let path = directory.0.join("preview.hex");
        let service = Service::new(48_000, 8, 0).expect("service");
        let initial = next_event(&service);
        assert!(initial.state.midi_preview.clips().is_empty());

        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "keys".into(),
            }))
            .expect("add instrument track");
        let track = next_event(&service).state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(4).expect("clip length"),
            }))
            .expect("place clip");
        let placed = next_event(&service);
        assert!(placed.playback.is_some());
        let clip = placed.state.project.tracks()[0].midi_clips()[0].id();
        assert_eq!(
            placed
                .state
                .midi_preview
                .get(clip)
                .unwrap()
                .occurrence_count,
            0
        );

        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_ratio(1, 4).expect("note gate"),
                pitch: 60,
                channel: 0,
                velocity: 100,
            }))
            .expect("add note");
        let noted = next_event(&service);
        assert!(noted.playback.is_some());
        assert_eq!(
            noted.state.midi_preview.get(clip).unwrap().occurrence_count,
            1
        );
        assert!(noted.state.revision > placed.state.revision);

        service.submit(Request::Undo).expect("undo note");
        let undone = next_event(&service);
        assert!(undone.playback.is_some());
        assert_eq!(
            undone
                .state
                .midi_preview
                .get(clip)
                .unwrap()
                .occurrence_count,
            0
        );
        service.submit(Request::Redo).expect("redo note");
        let redone = next_event(&service);
        assert!(redone.playback.is_some());
        let redone_artifact = Arc::clone(
            redone
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("redo playback retains prepared revision"),
        );
        assert_eq!(
            redone
                .state
                .midi_preview
                .get(clip)
                .unwrap()
                .occurrence_count,
            1
        );

        service
            .submit(Request::Edit(Command::SetMidiClipSource {
                clip,
                source_length: hex_project::BeatTime::from_raw(2),
                source_start: hex_project::BeatTime::ZERO,
                loop_start: hex_project::BeatTime::ZERO,
                loop_end: hex_project::BeatTime::from_raw(1),
                looping: true,
                length: hex_project::BeatTime::from_raw(65_538),
            }))
            .expect("submit refused loop edit");
        let refused = next_event(&service);
        assert!(refused.error.is_some());
        assert!(refused.playback.is_none());
        assert_eq!(refused.state.revision, redone.state.revision);
        assert!(Arc::ptr_eq(
            &refused.state.midi_preview,
            &redone.state.midi_preview
        ));

        service
            .submit(Request::SaveNew(path.clone()))
            .expect("save preview project");
        let saved = next_event(&service);
        assert!(saved.error.is_none());
        assert!(saved.playback.is_none());
        assert_eq!(saved.state.revision, redone.state.revision);
        assert!(Arc::ptr_eq(
            &saved.state.midi_preview,
            &redone.state.midi_preview
        ));

        service
            .submit(Request::Open(path))
            .expect("reopen saved project");
        let reopened = next_event(&service);
        assert!(reopened.error.is_none());
        assert!(reopened.playback.is_some());
        assert!(reopened.project_replaced);
        assert_eq!(reopened.state.revision, saved.state.revision + 1);
        let reopened_artifact = reopened
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("opened playback retains prepared revision");
        assert!(Arc::ptr_eq(
            redone_artifact.session().sine_table(),
            reopened_artifact.session().sine_table()
        ));
        assert_eq!(
            reopened
                .state
                .midi_preview
                .get(clip)
                .unwrap()
                .occurrence_count,
            1
        );
    }

    #[test]
    fn prepared_revision_tracks_session_inventory_history_open_restore_and_refusals() {
        let directory = TestDir::new();
        let saved_path = directory.0.join("session-grid.hex");
        let invalid_path = directory.0.join("overdense-session.hex");
        let service = Service::new(48_000, 64, 0).expect("service");
        let _ = next_event(&service);

        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "keys".into(),
            }))
            .expect("add instrument track");
        let track = next_event(&service).state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::AddScene {
                name: "first".into(),
            }))
            .expect("add scene");
        let scene = next_event(&service).state.project.scenes()[0].id();
        service
            .submit(Request::Edit(Command::CreateSessionMidiClip {
                track,
                scene,
                name: "cell".into(),
                source_length: hex_project::BeatTime::from_quarters(4).expect("source"),
            }))
            .expect("create Session MIDI cell");
        let clip_event = next_event(&service);
        let clip = match clip_event.created.expect("created Session clip") {
            hex_project::Created::SessionClip(id) => id,
            result => panic!("unexpected creation result: {result:?}"),
        };
        let empty_artifact = clip_event
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("compiled Session inventory");
        assert_eq!(
            empty_artifact.session().cell(track, scene),
            Some(hex_engine::PreparedSessionCellContent::Midi(clip))
        );
        assert_eq!(
            empty_artifact
                .session()
                .midi_clip(clip)
                .unwrap()
                .work
                .active_voices,
            0
        );

        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_quarters(1).expect("gate"),
                pitch: 60,
                channel: 0,
                velocity: 100,
            }))
            .expect("add Session note");
        let noted = next_event(&service);
        let noted_artifact = Arc::clone(
            noted
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("note edit artifact"),
        );
        assert_eq!(
            noted
                .state
                .project
                .session_midi_clip(clip)
                .unwrap()
                .source()
                .notes()
                .len(),
            1
        );
        assert_eq!(
            noted_artifact
                .session()
                .midi_clip(clip)
                .unwrap()
                .work
                .active_voices,
            1
        );

        service.submit(Request::Undo).expect("undo Session note");
        let undone = next_event(&service);
        assert_eq!(
            undone
                .state
                .project
                .session_midi_clip(clip)
                .unwrap()
                .source()
                .notes()
                .len(),
            0
        );
        assert_eq!(
            undone
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .unwrap()
                .session()
                .midi_clip(clip)
                .unwrap()
                .work
                .active_voices,
            0
        );
        service.submit(Request::Redo).expect("redo Session note");
        let redone = next_event(&service);
        assert_eq!(
            redone
                .state
                .project
                .session_midi_clip(clip)
                .unwrap()
                .source()
                .notes()
                .len(),
            1
        );
        let redone_artifact = Arc::clone(
            redone
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("redo artifact"),
        );
        assert_eq!(
            redone_artifact
                .session()
                .midi_clip(clip)
                .unwrap()
                .work
                .active_voices,
            1
        );

        service
            .submit(Request::SaveNew(saved_path.clone()))
            .expect("save populated Session project");
        let saved = next_event(&service);
        assert!(saved.error.is_none());
        service
            .submit(Request::Open(saved_path))
            .expect("reopen populated Session project");
        let reopened = next_event(&service);
        assert!(reopened.error.is_none());
        assert_eq!(
            reopened
                .state
                .project
                .session_midi_clip(clip)
                .unwrap()
                .source()
                .notes()
                .len(),
            1
        );
        let reopened_artifact = Arc::clone(
            reopened
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("opened artifact"),
        );
        assert_eq!(
            reopened_artifact.session().cell(track, scene),
            Some(hex_engine::PreparedSessionCellContent::Midi(clip))
        );
        assert_eq!(
            reopened_artifact
                .session()
                .midi_clip(clip)
                .unwrap()
                .work
                .active_voices,
            1
        );

        service
            .submit(Request::Edit(Command::AddScene {
                name: "dense".into(),
            }))
            .expect("add dense scene");
        let dense_scene = match next_event(&service).created.unwrap() {
            hex_project::Created::Scene(id) => id,
            result => panic!("unexpected scene result: {result:?}"),
        };
        service
            .submit(Request::Edit(Command::CreateSessionMidiClip {
                track,
                scene: dense_scene,
                name: "unlaunched dense cell".into(),
                source_length: hex_project::BeatTime::from_quarters(4).expect("source"),
            }))
            .expect("create unlaunched cell");
        let dense_clip = match next_event(&service).created.unwrap() {
            hex_project::Created::SessionClip(id) => id,
            result => panic!("unexpected clip result: {result:?}"),
        };
        let notes = (0..64_u8)
            .map(|pitch| hex_project::MidiNoteInsert {
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_quarters(1).expect("gate"),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .collect();
        service
            .submit(Request::Edit(Command::AddMidiNotes {
                clip: dense_clip,
                notes,
            }))
            .expect("add admitted dense source");
        let dense = next_event(&service);
        assert!(dense.error.is_none());
        assert_eq!(
            dense
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .unwrap()
                .session()
                .midi_clip(dense_clip)
                .unwrap()
                .work
                .active_voices,
            64
        );
        service
            .submit(Request::Edit(Command::AddScene {
                name: "redo branch".into(),
            }))
            .expect("create redo branch");
        let _ = next_event(&service);
        service.submit(Request::Undo).expect("undo scene branch");
        let dense_before_refusal = next_event(&service);
        assert!(dense_before_refusal.state.can_redo);
        let accepted_artifact = Arc::clone(
            dense_before_refusal
                .playback
                .as_ref()
                .and_then(Playback::prepared_revision)
                .expect("accepted dense artifact"),
        );
        let prior_revision = dense_before_refusal.state.revision;
        let prior_project = dense_before_refusal.state.project.as_ref().clone();
        service
            .submit(Request::Edit(Command::AddMidiNote {
                clip: dense_clip,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::from_quarters(1).expect("gate"),
                pitch: 64,
                channel: 0,
                velocity: 100,
            }))
            .expect("submit over-dense edit");
        let refused = next_event(&service);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.revision, prior_revision);
        assert_eq!(refused.state.project.as_ref(), &prior_project);
        assert!(refused.state.can_redo);
        assert!(refused.playback.is_none());
        service
            .submit(Request::RestorePlayback { generation: 9 })
            .expect("restore accepted Session artifact");
        let restored = next_event(&service);
        let restored_artifact = restored
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("restored Session artifact");
        assert!(Arc::ptr_eq(&accepted_artifact, restored_artifact));
        assert_eq!(
            restored_artifact
                .session()
                .midi_clip(dense_clip)
                .unwrap()
                .work
                .active_voices,
            64
        );

        let mut invalid_editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        let invalid_track = match invalid_editor
            .execute(Command::AddMidiTrack {
                name: "invalid".into(),
            })
            .unwrap()
        {
            hex_project::Created::Track(id) => id,
            _ => panic!("track result"),
        };
        let invalid_scene = match invalid_editor
            .execute(Command::AddScene {
                name: "invalid".into(),
            })
            .unwrap()
        {
            hex_project::Created::Scene(id) => id,
            _ => panic!("scene result"),
        };
        let invalid_clip = match invalid_editor
            .execute(Command::CreateSessionMidiClip {
                track: invalid_track,
                scene: invalid_scene,
                name: "too many voices".into(),
                source_length: hex_project::BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        {
            hex_project::Created::SessionClip(id) => id,
            _ => panic!("clip result"),
        };
        for pitch in 0..65_u8 {
            invalid_editor
                .execute(Command::AddMidiNote {
                    clip: invalid_clip,
                    start: hex_project::BeatTime::ZERO,
                    duration: hex_project::BeatTime::from_quarters(1).unwrap(),
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
        hex_storage::save_new(&invalid_path, invalid_editor.project())
            .expect("save invalid fixture");
        service
            .submit(Request::Open(invalid_path))
            .expect("submit over-dense Session open");
        let rejected_open = next_event(&service);
        assert!(rejected_open.open_complete);
        assert!(rejected_open.error.is_some());
        assert!(!rejected_open.project_replaced);
        assert_eq!(rejected_open.state.revision, prior_revision);
        assert_eq!(rejected_open.state.project.as_ref(), &prior_project);
        service
            .submit(Request::RestorePlayback { generation: 10 })
            .expect("restore after failed open");
        let restored_after_open = next_event(&service);
        let restored_open_artifact = restored_after_open
            .playback
            .as_ref()
            .and_then(Playback::prepared_revision)
            .expect("retained artifact after failed open");
        assert!(Arc::ptr_eq(&accepted_artifact, restored_open_artifact));
    }

    #[test]
    fn dropping_service_detaches_an_active_export_worker() {
        let directory = TestDir::new();
        let source = directory.0.join("source.wav");
        fs::write(&source, wav_mono(&[16_384, 8_192])).expect("source fixture");
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (finished_sender, finished_receiver) = mpsc::channel();
        let exporter: Exporter = {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            Arc::new(move |path, project, _, frames| {
                entered.wait();
                release.wait();
                let _ = finished_sender.send(());
                Ok(ExportReport {
                    path: path.to_path_buf(),
                    sample_rate: project.sample_rate(),
                    frames,
                    data_bytes: u32::try_from(frames * 4).unwrap_or(u32::MAX),
                })
            })
        };
        let service = Service::with_exporter(48_000, 8, 8, exporter).expect("service");
        let _ = next_event(&service);
        service
            .submit(Request::Import(source))
            .expect("submit import");
        let imported = next_event(&service);
        assert!(imported.error.is_none());
        service
            .submit(Request::Export(directory.0.join("detached.wav")))
            .expect("submit export");
        let _ = next_event(&service);
        entered.wait();

        let (dropped_sender, dropped_receiver) = mpsc::channel();
        let drop_thread = thread::spawn(move || {
            drop(service);
            let _ = dropped_sender.send(());
        });
        dropped_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("dropping endpoints must not join the active exporter");
        release.wait();
        finished_receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("detached export worker should finish after release");
        drop_thread.join().expect("drop thread");
    }
}
