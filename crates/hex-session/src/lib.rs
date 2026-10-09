//! Control-side owner of canonical editing state and its bounded decoded media.
use hex_engine::{Audio, Project as PlaybackProject};
use hex_media::{
    DecodedAudio, ImportError as MediaInvalid, ResampleLimits, resample,
    waveform::{self, Waveform},
};
use hex_project::{
    AssetId, Command, Created, Editor, Invalid as ProjectInvalid, MAX_ASSETS, Project,
};
use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    fmt, mem,
    path::PathBuf,
    sync::Arc,
};

pub mod device_gesture;
pub mod service;
pub use hex_media::waveform::{Peak as CachedPeak, Waveform as CachedWaveform};

#[derive(Debug)]
pub enum SessionError {
    Invalid(&'static str),
    Project(ProjectInvalid),
    Media(MediaInvalid),
    Engine(hex_engine::Invalid),
    Allocation,
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Project(error) => write!(f, "project edit failed: {error}"),
            Self::Media(error) => write!(f, "media preparation failed: {error}"),
            Self::Engine(error) => write!(f, "playback compile failed: {error}"),
            Self::Allocation => f.write_str("could not reserve session media memory"),
        }
    }
}

impl Error for SessionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Project(error) => Some(error),
            Self::Media(error) => Some(error),
            Self::Engine(error) => Some(error),
            Self::Invalid(_) | Self::Allocation => None,
        }
    }
}

impl From<ProjectInvalid> for SessionError {
    fn from(error: ProjectInvalid) -> Self {
        Self::Project(error)
    }
}

impl From<hex_engine::Invalid> for SessionError {
    fn from(error: hex_engine::Invalid) -> Self {
        Self::Engine(error)
    }
}
impl From<MediaInvalid> for SessionError {
    fn from(error: MediaInvalid) -> Self {
        Self::Media(error)
    }
}

/// A validated WAV with its UI summary prepared before it enters session state.
pub struct PreparedImport {
    path: PathBuf,
    source_sample_rate: u32,
    source_frames: u64,
    conversion_version: u32,
    audio: Audio,
    waveform: Waveform,
}

impl PreparedImport {
    /// Prepare an identity import away from the UI and audio callback.
    pub fn new(decoded: DecodedAudio) -> Result<Self, SessionError> {
        let rate = decoded.sample_rate();
        let frames = decoded.frames();
        Self::at_rate(
            decoded,
            rate,
            ResampleLimits {
                max_source_frames: frames,
                max_output_frames: frames,
            },
        )
    }

    /// Convert and summarize before committing any canonical state. Bounds are
    /// checked before allocating converted PCM; original source metadata stays intact.
    pub fn at_rate(
        decoded: DecodedAudio,
        project_rate: u32,
        limits: ResampleLimits,
    ) -> Result<Self, SessionError> {
        let source_sample_rate = decoded.sample_rate();
        let source_frames = u64::try_from(decoded.frames())
            .map_err(|_| SessionError::Invalid("source frame count exceeds project range"))?;
        let audio = resample(decoded.audio(), project_rate, limits)?;
        let waveform = waveform::summarize(&audio, waveform::MAX_BINS)?;
        Ok(Self {
            path: decoded.path().to_path_buf(),
            source_sample_rate,
            source_frames,
            conversion_version: if source_sample_rate == project_rate {
                hex_project::IDENTITY_CONVERSION
            } else {
                hex_project::SINC_CONVERSION_V1
            },
            audio,
            waveform,
        })
    }

    pub fn waveform(&self) -> &Waveform {
        &self.waveform
    }
}

struct ResidentAsset {
    audio: Audio,
    waveform: Arc<Waveform>,
    frames: u64,
}

#[derive(Clone, Copy)]
struct ResidentImportAccounting {
    total_frames: u64,
    total_waveform_bins: usize,
    total_waveform_bytes: usize,
}

/// Canonical project controller plus retained, bounded decoded assets.
pub struct Session {
    editor: Editor,
    sine_table: Arc<hex_engine::SineTable>,
    media: HashMap<AssetId, ResidentAsset>,
    resident_frames: u64,
    max_resident_frames: u64,
    resident_waveform_bins: usize,
    resident_waveform_bytes: usize,
}

impl Session {
    pub fn new(
        sample_rate: u32,
        history_capacity: usize,
        max_resident_frames: u64,
    ) -> Result<Self, SessionError> {
        Ok(Self {
            editor: Editor::new(Project::new(sample_rate)?, history_capacity)?,
            sine_table: Arc::new(hex_engine::SineTable::new()),
            media: HashMap::new(),
            resident_frames: 0,
            max_resident_frames,
            resident_waveform_bins: 0,
            resident_waveform_bytes: 0,
        })
    }

    /// Build a clean-history session from validated project metadata and its
    /// decoded assets. The caller prepares every asset before replacing a live
    /// session, so a missing or mismatched file cannot partially load a project.
    pub fn from_loaded(
        project: Project,
        history_capacity: usize,
        max_resident_frames: u64,
        decoded_assets: Vec<(AssetId, DecodedAudio)>,
    ) -> Result<Self, SessionError> {
        let mut prepared_assets = Vec::new();
        prepared_assets
            .try_reserve_exact(decoded_assets.len())
            .map_err(|_| SessionError::Allocation)?;
        let mut remaining = max_resident_frames;
        for (id, decoded) in decoded_assets {
            let metadata = project
                .asset(id)
                .ok_or(SessionError::Invalid("decoded asset is not in project"))?;
            if decoded.sample_rate() != metadata.source_sample_rate()
                || u64::try_from(decoded.frames()).ok() != Some(metadata.source_frames())
            {
                return Err(SessionError::Invalid(
                    "source media differs from project metadata",
                ));
            }
            let prepared = PreparedImport::at_rate(
                decoded,
                project.sample_rate(),
                ResampleLimits {
                    max_source_frames: 8_000_000,
                    max_output_frames: usize::try_from(remaining).map_err(|_| {
                        SessionError::Invalid("resident frame limit exceeds addressable range")
                    })?,
                },
            )?;
            remaining = remaining
                .checked_sub(metadata.playback_frames())
                .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
            prepared_assets.push((id, prepared));
        }
        Self::from_prepared(
            project,
            history_capacity,
            max_resident_frames,
            prepared_assets,
        )
    }

    /// Assemble a clean session from one-at-a-time prepared assets. Validate both
    /// source provenance and project-domain PCM before accepting the replacement.
    pub fn from_prepared(
        project: Project,
        history_capacity: usize,
        max_resident_frames: u64,
        prepared_assets: Vec<(AssetId, PreparedImport)>,
    ) -> Result<Self, SessionError> {
        Self::from_prepared_with_sine_table(
            project,
            history_capacity,
            max_resident_frames,
            prepared_assets,
            Arc::new(hex_engine::SineTable::new()),
        )
    }

    /// Assemble a replacement while retaining the controller's stable prepared
    /// instrument table across project opens.
    pub fn from_prepared_with_sine_table(
        project: Project,
        history_capacity: usize,
        max_resident_frames: u64,
        prepared_assets: Vec<(AssetId, PreparedImport)>,
        sine_table: Arc<hex_engine::SineTable>,
    ) -> Result<Self, SessionError> {
        let asset_count = project.assets().count();
        let editor = Editor::new(project, history_capacity)?;
        if asset_count != prepared_assets.len() {
            return Err(SessionError::Invalid(
                "prepared media does not match project assets",
            ));
        }
        let mut media = HashMap::new();
        media
            .try_reserve(asset_count)
            .map_err(|_| SessionError::Allocation)?;
        let mut resident_frames = 0_u64;
        let mut resident_waveform_bins = 0_usize;
        let mut resident_waveform_bytes = 0_usize;
        for (asset_id, prepared) in prepared_assets {
            let metadata = editor
                .project()
                .asset(asset_id)
                .ok_or(SessionError::Invalid("decoded asset is not in project"))?;
            let frames = u64::try_from(prepared.audio.frames().len())
                .map_err(|_| SessionError::Invalid("decoded frame count exceeds project range"))?;
            if prepared.audio.sample_rate() != editor.project().sample_rate()
                || frames != metadata.playback_frames()
                || prepared.source_sample_rate != metadata.source_sample_rate()
                || prepared.source_frames != metadata.source_frames()
                || prepared.conversion_version != metadata.conversion_version()
            {
                return Err(SessionError::Invalid(
                    "decoded media rate or frame count differs from project metadata",
                ));
            }
            resident_frames = resident_frames
                .checked_add(frames)
                .filter(|total| *total <= max_resident_frames)
                .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
            let audio = prepared.audio;
            let waveform = prepared.waveform;
            let bins = waveform.cached_bin_count();
            let bytes = bins
                .checked_mul(mem::size_of::<[waveform::Peak; 2]>())
                .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
            resident_waveform_bins = resident_waveform_bins
                .checked_add(bins)
                .ok_or(SessionError::Invalid("waveform bin count overflow"))?;
            resident_waveform_bytes = resident_waveform_bytes
                .checked_add(bytes)
                .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
            if media
                .insert(
                    asset_id,
                    ResidentAsset {
                        audio,
                        waveform: Arc::new(waveform),
                        frames,
                    },
                )
                .is_some()
            {
                return Err(SessionError::Invalid("duplicate decoded project asset"));
            }
        }
        Ok(Self {
            editor,
            sine_table,
            media,
            resident_frames,
            max_resident_frames,
            resident_waveform_bins,
            resident_waveform_bytes,
        })
    }

    pub fn project(&self) -> &Project {
        self.editor.project()
    }

    /// Resolve a canonical device's class from its owning track slot. UI
    /// requests carry stable IDs, never authority to select a registry.
    pub(crate) fn native_device_class(
        &self,
        device: hex_project::DeviceId,
    ) -> Result<hex_engine::NativeDeviceClass, SessionError> {
        for track in self.project().tracks() {
            if track
                .instrument()
                .is_some_and(|instrument| instrument.id() == device)
            {
                return Ok(hex_engine::NativeDeviceClass::Instrument);
            }
            if track.devices().iter().any(|effect| effect.id() == device) {
                return Ok(hex_engine::NativeDeviceClass::Effect);
            }
        }
        Err(SessionError::Invalid("device no longer exists"))
    }

    pub fn sine_table(&self) -> Arc<hex_engine::SineTable> {
        Arc::clone(&self.sine_table)
    }

    pub const fn resident_frames(&self) -> u64 {
        self.resident_frames
    }

    pub const fn max_resident_frames(&self) -> u64 {
        self.max_resident_frames
    }

    fn reserve_waveform_accounting(
        &self,
        waveform: &Waveform,
    ) -> Result<(usize, usize), SessionError> {
        let bins = waveform.cached_bin_count();
        let bytes = bins
            .checked_mul(mem::size_of::<[waveform::Peak; 2]>())
            .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
        let total_bins = self
            .resident_waveform_bins
            .checked_add(bins)
            .ok_or(SessionError::Invalid("waveform bin count overflow"))?;
        let total_bytes = self
            .resident_waveform_bytes
            .checked_add(bytes)
            .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
        Ok((total_bins, total_bytes))
    }

    fn commit_resident_import(
        &mut self,
        id: AssetId,
        audio: Audio,
        waveform: Waveform,
        frames: u64,
        accounting: ResidentImportAccounting,
    ) {
        self.media.insert(
            id,
            ResidentAsset {
                audio,
                waveform: Arc::new(waveform),
                frames,
            },
        );
        self.resident_frames = accounting.total_frames;
        self.resident_waveform_bins = accounting.total_waveform_bins;
        self.resident_waveform_bytes = accounting.total_waveform_bytes;
    }

    pub const fn resident_waveform_bins(&self) -> usize {
        self.resident_waveform_bins
    }

    pub const fn resident_waveform_bytes(&self) -> usize {
        self.resident_waveform_bytes
    }

    pub fn can_undo(&self) -> bool {
        self.editor.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.editor.can_redo()
    }

    /// Execute an edit that does not create a new audio asset.
    pub fn command(&mut self, command: Command) -> Result<Created, SessionError> {
        self.command_prepared(command).map(|(created, _)| created)
    }

    /// Compile before committing history, returning the exact prepared graph for
    /// publication without compiling the same candidate again.
    pub fn command_prepared(
        &mut self,
        command: Command,
    ) -> Result<(Created, PlaybackProject), SessionError> {
        if matches!(
            &command,
            Command::AddAsset { .. }
                | Command::ImportAudioTrack { .. }
                | Command::ImportPreparedAudioTrack { .. }
                | Command::ImportPreparedAudioTrackAt { .. }
                | Command::ImportPreparedAudioClip { .. }
                | Command::ImportPreparedAudioClipWithOverwrite { .. }
                | Command::ImportPreparedSessionAudioClip { .. }
        ) {
            return Err(SessionError::Invalid(
                "audio media must be committed through a prepared import",
            ));
        }
        let (created, prepared) = self.command_prepared_revision(command, None)?;
        Ok((created, prepared.arrangement().as_ref().clone()))
    }

    /// Prepare playback and its Arrangement MIDI preview before committing the
    /// edit, returning both outputs from the exact accepted candidate compile.
    pub fn command_prepared_with_midi_preview(
        &mut self,
        command: Command,
    ) -> Result<
        (
            Created,
            PlaybackProject,
            Arc<hex_engine::MidiArrangementPreview>,
        ),
        SessionError,
    > {
        if matches!(
            &command,
            Command::AddAsset { .. }
                | Command::ImportAudioTrack { .. }
                | Command::ImportPreparedAudioTrack { .. }
                | Command::ImportPreparedAudioTrackAt { .. }
                | Command::ImportPreparedAudioClip { .. }
                | Command::ImportPreparedAudioClipWithOverwrite { .. }
        ) {
            return Err(SessionError::Invalid(
                "audio media must be committed through a prepared import",
            ));
        }
        let (created, prepared) = self.command_prepared_revision(command, None)?;
        Ok((
            created,
            prepared.arrangement().as_ref().clone(),
            Arc::clone(prepared.midi_preview()),
        ))
    }

    /// Prepare all worker products before committing one canonical edit.
    pub fn command_prepared_revision(
        &mut self,
        command: Command,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        if matches!(
            &command,
            Command::AddAsset { .. }
                | Command::ImportAudioTrack { .. }
                | Command::ImportPreparedAudioTrack { .. }
                | Command::ImportPreparedAudioTrackAt { .. }
                | Command::ImportPreparedAudioClip { .. }
                | Command::ImportPreparedAudioClipWithOverwrite { .. }
        ) {
            return Err(SessionError::Invalid(
                "audio media must be committed through a prepared import",
            ));
        }
        let media = self.media_bank();
        let sine_table = self.sine_table();
        self.editor.execute_prepared(command, |candidate| {
            hex_engine::PreparedRevision::prepare(
                candidate,
                &media,
                Arc::clone(&sine_table),
                previous.map(|artifact| artifact.session().as_ref()),
            )
            .map(Arc::new)
            .map_err(SessionError::from)
        })
    }

    /// Prepare a typed native parameter overlay without changing canonical
    /// metadata, dirty state or history. All construction stays on control.
    pub fn preview_device_parameter(
        &self,
        device: hex_project::DeviceId,
        parameter: hex_lattice_contract::ParameterId,
        value: hex_lattice_contract::ParameterValue,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Arc<Project>, Arc<hex_engine::PreparedRevision>), SessionError> {
        let class = self.native_device_class(device)?;
        let current = self
            .project()
            .device(device)
            .ok_or(SessionError::Invalid("device no longer exists"))?;
        let state = hex_engine::patch_native_device_parameter_for_class(
            class,
            current.state(),
            parameter,
            value,
        )?;
        let mut preview = Editor::new(self.project().clone(), 1)?;
        let media = self.media_bank();
        let table = self.sine_table();
        let (_, prepared) = preview.execute_prepared(
            Command::ReplaceDeviceState { device, state },
            |candidate| {
                hex_engine::PreparedRevision::prepare(
                    candidate,
                    &media,
                    Arc::clone(&table),
                    previous.map(|artifact| artifact.session().as_ref()),
                )
                .map(Arc::new)
                .map_err(SessionError::from)
            },
        )?;
        Ok((Arc::new(preview.project().clone()), prepared))
    }

    /// Commit only this parameter onto the latest authored device state.
    pub fn commit_device_parameter(
        &mut self,
        device: hex_project::DeviceId,
        parameter: hex_lattice_contract::ParameterId,
        value: hex_lattice_contract::ParameterValue,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        let class = self.native_device_class(device)?;
        let current = self
            .project()
            .device(device)
            .ok_or(SessionError::Invalid("device no longer exists"))?;
        let state = hex_engine::patch_native_device_parameter_for_class(
            class,
            current.state(),
            parameter,
            value,
        )?;
        self.command_prepared_revision(Command::ReplaceDeviceState { device, state }, previous)
            .map(|(_, prepared)| prepared)
    }

    /// Commit one prepared import. All resource checks and fallible reservations
    /// happen before the canonical project edit, so the commit cannot orphan it.
    pub fn import(&mut self, prepared: PreparedImport) -> Result<Created, SessionError> {
        self.import_with_midi_preview(prepared)
            .map(|(created, _, _)| created)
    }

    /// Prepare the candidate playback graph and MIDI preview before committing
    /// imported metadata or adding the decoded asset to resident session state.
    pub fn import_with_midi_preview(
        &mut self,
        prepared: PreparedImport,
    ) -> Result<
        (
            Created,
            PlaybackProject,
            Arc<hex_engine::MidiArrangementPreview>,
        ),
        SessionError,
    > {
        let (created, revision) = self.import_prepared_revision(prepared, None)?;
        Ok((
            created,
            revision.arrangement().as_ref().clone(),
            Arc::clone(revision.midi_preview()),
        ))
    }

    pub fn import_prepared_revision(
        &mut self,
        prepared: PreparedImport,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.import_prepared_audio_track_at_revision(prepared, None, 0, previous)
    }

    /// Insert a new hybrid track before `before_track`, or append it when absent,
    /// and import the prepared audio at `start` as one history operation.
    pub fn import_prepared_audio_track_at_revision(
        &mut self,
        prepared: PreparedImport,
        before_track: Option<hex_project::TrackId>,
        start: u64,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        if prepared.audio.sample_rate() != self.project().sample_rate() {
            return Err(SessionError::Invalid(
                "prepared audio sample rate differs from project",
            ));
        }
        if self.media.len() >= MAX_ASSETS {
            return Err(SessionError::Invalid("resident asset limit reached"));
        }
        let frames = u64::try_from(prepared.waveform.source_frames)
            .map_err(|_| SessionError::Invalid("decoded frame count exceeds project range"))?;
        let new_resident_frames = self
            .resident_frames
            .checked_add(frames)
            .filter(|total| *total <= self.max_resident_frames)
            .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
        if self.media.try_reserve(1).is_err() {
            return Err(SessionError::Allocation);
        }
        let bins = prepared.waveform.cached_bin_count();
        let waveform_bytes = bins
            .checked_mul(mem::size_of::<[waveform::Peak; 2]>())
            .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
        let new_resident_bins = self
            .resident_waveform_bins
            .checked_add(bins)
            .ok_or(SessionError::Invalid("waveform bin count overflow"))?;
        let new_resident_waveform_bytes = self
            .resident_waveform_bytes
            .checked_add(waveform_bytes)
            .ok_or(SessionError::Invalid("waveform memory count overflow"))?;
        let mut candidate_media = self.media_bank();
        let staged_audio = prepared.audio.clone();
        let staged_asset = prepared.audio.clone();
        let sine_table = self.sine_table();
        let (result, (asset_id, revision)) = self.editor.execute_prepared(
            Command::ImportPreparedAudioTrackAt {
                name: "imported audio".into(),
                before_track,
                start,
                path: prepared.path,
                source_sample_rate: prepared.source_sample_rate,
                source_frames: prepared.source_frames,
                playback_frames: frames,
                conversion_version: prepared.conversion_version,
            },
            |candidate| {
                let mut added_assets = candidate
                    .assets()
                    .filter(|asset| !candidate_media.contains_key(&asset.id()));
                let asset_id = added_assets
                    .next()
                    .map(|asset| asset.id())
                    .ok_or(SessionError::Invalid("import candidate has no new asset"))?;
                if added_assets.next().is_some() {
                    return Err(SessionError::Invalid(
                        "import candidate contains multiple new assets",
                    ));
                }
                candidate_media.insert(asset_id, staged_asset.clone());
                let revision = hex_engine::PreparedRevision::prepare(
                    candidate,
                    &candidate_media,
                    Arc::clone(&sine_table),
                    previous.map(|artifact| artifact.session().as_ref()),
                )
                .map(Arc::new)
                .map_err(SessionError::from)?;
                Ok((asset_id, revision))
            },
        )?;
        let resident = ResidentAsset {
            audio: staged_audio,
            waveform: Arc::new(prepared.waveform),
            frames,
        };
        self.media.insert(asset_id, resident);
        self.resident_frames = new_resident_frames;
        self.resident_waveform_bins = new_resident_bins;
        self.resident_waveform_bytes = new_resident_waveform_bytes;
        Ok((result, revision))
    }

    /// Import a full-length source clip onto an existing hybrid track as one
    /// history edit, preparing media and playback before publishing metadata.
    pub fn import_prepared_audio_clip_revision(
        &mut self,
        prepared: PreparedImport,
        track: hex_project::TrackId,
        start: u64,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.import_prepared_audio_clip_revision_mode(prepared, track, start, previous, false)
    }

    /// Import prepared media onto a hybrid track, cutting its destination
    /// interval from overlapping Arrangement clips as one history edit.
    pub fn import_prepared_audio_clip_with_overwrite_revision(
        &mut self,
        prepared: PreparedImport,
        track: hex_project::TrackId,
        start: u64,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        self.import_prepared_audio_clip_revision_mode(prepared, track, start, previous, true)
    }

    fn import_prepared_audio_clip_revision_mode(
        &mut self,
        prepared: PreparedImport,
        track: hex_project::TrackId,
        start: u64,
        previous: Option<&hex_engine::PreparedRevision>,
        overwrite: bool,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        if prepared.audio.sample_rate() != self.project().sample_rate() {
            return Err(SessionError::Invalid(
                "prepared audio sample rate differs from project",
            ));
        }
        if self.media.len() >= MAX_ASSETS {
            return Err(SessionError::Invalid("resident asset limit reached"));
        }
        let frames = u64::try_from(prepared.waveform.source_frames)
            .map_err(|_| SessionError::Invalid("decoded frame count exceeds project range"))?;
        let new_resident_frames = self
            .resident_frames
            .checked_add(frames)
            .filter(|total| *total <= self.max_resident_frames)
            .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
        self.media
            .try_reserve(1)
            .map_err(|_| SessionError::Allocation)?;
        let (new_resident_bins, new_resident_waveform_bytes) =
            self.reserve_waveform_accounting(&prepared.waveform)?;
        let mut candidate_media = self.media_bank();
        let staged_audio = prepared.audio.clone();
        let staged_asset = prepared.audio.clone();
        let sine_table = self.sine_table();
        let (created, (asset_id, revision)) = self.editor.execute_prepared(
            if overwrite {
                Command::ImportPreparedAudioClipWithOverwrite {
                    track,
                    start,
                    path: prepared.path,
                    source_sample_rate: prepared.source_sample_rate,
                    source_frames: prepared.source_frames,
                    playback_frames: frames,
                    conversion_version: prepared.conversion_version,
                }
            } else {
                Command::ImportPreparedAudioClip {
                    track,
                    start,
                    path: prepared.path,
                    source_sample_rate: prepared.source_sample_rate,
                    source_frames: prepared.source_frames,
                    playback_frames: frames,
                    conversion_version: prepared.conversion_version,
                }
            },
            |candidate| {
                let mut added = candidate
                    .assets()
                    .filter(|asset| !candidate_media.contains_key(&asset.id()));
                let asset_id = added
                    .next()
                    .map(|asset| asset.id())
                    .ok_or(SessionError::Invalid("import candidate has no new asset"))?;
                if added.next().is_some() {
                    return Err(SessionError::Invalid(
                        "import candidate contains multiple new assets",
                    ));
                }
                candidate_media.insert(asset_id, staged_asset.clone());
                let revision = hex_engine::PreparedRevision::prepare(
                    candidate,
                    &candidate_media,
                    Arc::clone(&sine_table),
                    previous.map(|artifact| artifact.session().as_ref()),
                )
                .map(Arc::new)
                .map_err(SessionError::from)?;
                Ok((asset_id, revision))
            },
        )?;
        self.media.insert(
            asset_id,
            ResidentAsset {
                audio: staged_audio,
                waveform: Arc::new(prepared.waveform),
                frames,
            },
        );
        self.resident_frames = new_resident_frames;
        self.resident_waveform_bins = new_resident_bins;
        self.resident_waveform_bytes = new_resident_waveform_bytes;
        Ok((created, revision))
    }

    /// Import media and create a one-shot Session cell as one history edit.
    /// Decoding, resampling and waveform preparation are already complete before
    /// this method commits either metadata or resident PCM.
    pub fn import_session_audio_clip_revision(
        &mut self,
        prepared: PreparedImport,
        track: hex_project::TrackId,
        scene: hex_project::SceneId,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<(Created, Arc<hex_engine::PreparedRevision>), SessionError> {
        if prepared.audio.sample_rate() != self.project().sample_rate() {
            return Err(SessionError::Invalid(
                "prepared audio sample rate differs from project",
            ));
        }
        if self.media.len() >= MAX_ASSETS {
            return Err(SessionError::Invalid("resident asset limit reached"));
        }
        let frames = u64::try_from(prepared.waveform.source_frames)
            .map_err(|_| SessionError::Invalid("decoded frame count exceeds project range"))?;
        let new_resident_frames = self
            .resident_frames
            .checked_add(frames)
            .filter(|total| *total <= self.max_resident_frames)
            .ok_or(SessionError::Invalid("resident audio frame limit reached"))?;
        self.media
            .try_reserve(1)
            .map_err(|_| SessionError::Allocation)?;
        let (new_resident_bins, new_resident_waveform_bytes) =
            self.reserve_waveform_accounting(&prepared.waveform)?;
        let mut candidate_media = self.media_bank();
        let staged_audio = prepared.audio.clone();
        let staged_asset = prepared.audio.clone();
        let sine_table = self.sine_table();
        let source_end = u64::try_from(prepared.audio.frames().len())
            .map_err(|_| SessionError::Invalid("prepared frame count exceeds project range"))?;
        let (result, (asset_id, revision)) = self.editor.execute_prepared(
            Command::ImportPreparedSessionAudioClip {
                track,
                scene,
                name: prepared
                    .path
                    .file_stem()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "imported audio".into()),
                path: prepared.path,
                source_sample_rate: prepared.source_sample_rate,
                source_frames: prepared.source_frames,
                playback_frames: frames,
                conversion_version: prepared.conversion_version,
                source_start: 0,
                source_end,
            },
            |candidate| {
                let mut added_assets = candidate
                    .assets()
                    .filter(|asset| !candidate_media.contains_key(&asset.id()));
                let asset_id = added_assets
                    .next()
                    .map(|asset| asset.id())
                    .ok_or(SessionError::Invalid("import candidate has no new asset"))?;
                if added_assets.next().is_some() {
                    return Err(SessionError::Invalid(
                        "import candidate contains multiple new assets",
                    ));
                }
                candidate_media.insert(asset_id, staged_asset.clone());
                let revision = hex_engine::PreparedRevision::prepare(
                    candidate,
                    &candidate_media,
                    Arc::clone(&sine_table),
                    previous.map(|artifact| artifact.session().as_ref()),
                )
                .map(Arc::new)
                .map_err(SessionError::from)?;
                Ok((asset_id, revision))
            },
        )?;
        self.commit_resident_import(
            asset_id,
            staged_audio,
            prepared.waveform,
            frames,
            ResidentImportAccounting {
                total_frames: new_resident_frames,
                total_waveform_bins: new_resident_bins,
                total_waveform_bytes: new_resident_waveform_bytes,
            },
        );
        Ok((result, revision))
    }

    /// Restore metadata history while keeping all decoded assets available.
    pub fn undo(&mut self) -> bool {
        self.undo_prepared_revision(None)
            .is_ok_and(|result| result.is_some())
    }

    /// Compile the previous history snapshot before committing Undo.
    pub fn undo_prepared_with_midi_preview(
        &mut self,
    ) -> Result<Option<(PlaybackProject, Arc<hex_engine::MidiArrangementPreview>)>, SessionError>
    {
        self.undo_prepared_revision(None).map(|result| {
            result.map(|prepared| {
                (
                    prepared.arrangement().as_ref().clone(),
                    Arc::clone(prepared.midi_preview()),
                )
            })
        })
    }

    pub fn undo_prepared_revision(
        &mut self,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Option<Arc<hex_engine::PreparedRevision>>, SessionError> {
        let media = self.media_bank();
        let sine_table = self.sine_table();
        self.editor.undo_prepared(|candidate| {
            hex_engine::PreparedRevision::prepare(
                candidate,
                &media,
                Arc::clone(&sine_table),
                previous.map(|artifact| artifact.session().as_ref()),
            )
            .map(Arc::new)
            .map_err(SessionError::from)
        })
    }

    pub fn redo(&mut self) -> bool {
        self.redo_prepared_revision(None)
            .is_ok_and(|result| result.is_some())
    }

    /// Compile the next history snapshot before committing Redo.
    pub fn redo_prepared_with_midi_preview(
        &mut self,
    ) -> Result<Option<(PlaybackProject, Arc<hex_engine::MidiArrangementPreview>)>, SessionError>
    {
        self.redo_prepared_revision(None).map(|result| {
            result.map(|prepared| {
                (
                    prepared.arrangement().as_ref().clone(),
                    Arc::clone(prepared.midi_preview()),
                )
            })
        })
    }

    pub fn redo_prepared_revision(
        &mut self,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Option<Arc<hex_engine::PreparedRevision>>, SessionError> {
        let media = self.media_bank();
        let sine_table = self.sine_table();
        self.editor.redo_prepared(|candidate| {
            hex_engine::PreparedRevision::prepare(
                candidate,
                &media,
                Arc::clone(&sine_table),
                previous.map(|artifact| artifact.session().as_ref()),
            )
            .map(Arc::new)
            .map_err(SessionError::from)
        })
    }

    pub fn audio(&self, asset: AssetId) -> Option<&Audio> {
        self.media.get(&asset).map(|resident| &resident.audio)
    }

    pub fn waveform(&self, asset: AssetId) -> Option<Arc<Waveform>> {
        self.media
            .get(&asset)
            .map(|resident| Arc::clone(&resident.waveform))
    }

    /// Clone only the small asset map and its shared waveform handles.
    pub fn waveforms(&self) -> BTreeMap<AssetId, Arc<Waveform>> {
        self.media
            .iter()
            .map(|(id, resident)| (*id, Arc::clone(&resident.waveform)))
            .collect()
    }

    pub fn resident_asset_frames(&self, asset: AssetId) -> Option<u64> {
        self.media.get(&asset).map(|resident| resident.frames)
    }

    /// Compile the current canonical snapshot on a control worker.
    pub fn compile(&self) -> Result<PlaybackProject, SessionError> {
        Ok(self
            .compile_prepared_revision(None)?
            .arrangement()
            .as_ref()
            .clone())
    }

    /// Compile the current snapshot once for playback and prepared UI summaries.
    pub fn compile_with_midi_preview(
        &self,
    ) -> Result<(PlaybackProject, Arc<hex_engine::MidiArrangementPreview>), SessionError> {
        let prepared = self.compile_prepared_revision(None)?;
        Ok((
            prepared.arrangement().as_ref().clone(),
            Arc::clone(prepared.midi_preview()),
        ))
    }

    pub fn compile_prepared_revision(
        &self,
        previous: Option<&hex_engine::PreparedRevision>,
    ) -> Result<Arc<hex_engine::PreparedRevision>, SessionError> {
        Ok(Arc::new(hex_engine::PreparedRevision::prepare(
            self.project(),
            &self.media_bank(),
            self.sine_table(),
            previous.map(|artifact| artifact.session().as_ref()),
        )?))
    }

    fn media_bank(&self) -> BTreeMap<AssetId, Audio> {
        self.media
            .iter()
            .map(|(id, resident)| (*id, resident.audio.clone()))
            .collect()
    }
}
