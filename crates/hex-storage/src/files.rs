use crate::{ReplaceWarning, StorageError, decode, encode};
use hex_project::Project;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
};
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub fn load(path: impl AsRef<Path>) -> Result<Project, StorageError> {
    decode(&read_document(path.as_ref())?)
}

/// True when `left` and `right` both exist and resolve to the same underlying
/// file (including hard links and symbolic-link aliases).
pub fn same_existing_file(left: &Path, right: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (Ok(left_meta), Ok(right_meta)) = (fs::metadata(left), fs::metadata(right)) else {
            return false;
        };
        if left_meta.dev() == right_meta.dev() && left_meta.ino() == right_meta.ino() {
            return true;
        }
    }
    #[cfg(windows)]
    {
        if same_file::is_same_file(left, right).unwrap_or(false) {
            return true;
        }
    }
    matches!(
        (fs::canonicalize(left), fs::canonicalize(right)),
        (Ok(left_path), Ok(right_path)) if left_path == right_path
    )
}

/// Opaque exact-byte content snapshot, shared cheaply between owners.
/// This is a content check, not a filesystem lock, identity or overwrite permit.
#[derive(Clone)]
pub struct FileToken(pub(super) Arc<[u8]>, Option<u32>, Option<Vec<String>>);

impl std::fmt::Debug for FileToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileToken")
            .field("byte_count", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl FileToken {
    /// True when the file is older than [`crate::CURRENT_SCHEMA_VERSION`], so
    /// saving any project over it writes a format older readers cannot open.
    /// Tokens are constructed only after validated decoding or current encoding;
    /// an unexpected header conservatively requires confirmation too.
    pub fn requires_format_upgrade(&self) -> bool {
        self.1.is_none_or(|version| {
            !(crate::CURRENT_SCHEMA_VERSION..=crate::NEWEST_SCHEMA_VERSION).contains(&version)
        })
    }

    /// True when saving `project` writes a newer schema or requires capabilities
    /// absent from this file's saved format, for example adding sinc v2 media to
    /// a schema63 file or adding a new capability to a schema126 manifest.
    /// Compares the actual saved formats, not application versions.
    pub fn requires_schema_upgrade_for(&self, project: &Project) -> bool {
        if self.requires_format_upgrade() {
            return true;
        }
        let Some(version) = self.1 else {
            return true;
        };
        if version < crate::schema_version_for(project) {
            return true;
        }
        if matches!(
            version,
            crate::CAPABILITY_MANIFEST_SCHEMA_VERSION
                | crate::MIDI_EXPRESSION_SCHEMA_VERSION
                | crate::AUTOMATION_CURVES_AND_FREEZE_SCHEMA_VERSION
                | crate::TRACK_IO_ROUTING_SCHEMA_VERSION
                | crate::SESSION_FOLLOW_ACTION_SCHEMA_VERSION
                | crate::LINKED_SCENE_PLACEMENTS_SCHEMA_VERSION
        ) {
            let Some(existing) = self.2.as_deref() else {
                return true;
            };
            return crate::capabilities_for(project).into_iter().any(|cap| {
                existing
                    .binary_search_by(|item| item.as_str().cmp(cap))
                    .is_err()
            });
        }
        false
    }

    /// Control-side preflight for replacing a file an older reader could open.
    /// The saved layout is unchanged, but crossing its former byte ceiling loses
    /// reader compatibility just as a schema upgrade does. Already-large files
    /// do not acquire a new compatibility loss on subsequent saves.
    pub fn requires_save_confirmation(&self, project: &Project) -> Result<bool, StorageError> {
        self.requires_save_confirmation_within(project, crate::LEGACY_READER_DOCUMENT_BYTES)
    }

    /// The same preflight against an explicit older-reader ceiling. Production
    /// uses [`crate::LEGACY_READER_DOCUMENT_BYTES`]; callers may pass a smaller
    /// ceiling to exercise their consent flow without encoding 8 MiB documents.
    pub fn requires_save_confirmation_within(
        &self,
        project: &Project,
        limit: usize,
    ) -> Result<bool, StorageError> {
        Ok(self.requires_schema_upgrade_for(project)
            || (self.0.len() <= limit && encode(project)?.len() > limit))
    }

    pub(super) fn from_validated_bytes(bytes: Arc<[u8]>) -> Self {
        #[derive(serde::Deserialize)]
        struct Header {
            version: u32,
            #[serde(default)]
            capabilities: Option<Vec<String>>,
        }
        let parsed = serde_json::from_slice::<Header>(&bytes).ok();
        let version = parsed.as_ref().map(|header| header.version);
        let capabilities = parsed.and_then(|header| header.capabilities);
        Self(bytes, version, capabilities)
    }

    /// Compare the current document with the captured bytes without modifying it.
    /// Byte-identical replacement is accepted. A later noncooperating write can
    /// still race any future publication; this method is not a compare-and-swap.
    pub fn check_unchanged(&self, path: impl AsRef<Path>) -> Result<(), StorageError> {
        let path = path.as_ref();
        if read_document(path)?.as_slice() == self.0.as_ref() {
            Ok(())
        } else {
            Err(StorageError::Conflict {
                path: path.to_path_buf(),
            })
        }
    }

    /// True when the original on-disk document bytes already contain `needle`.
    pub fn contains_bytes(&self, needle: &[u8]) -> bool {
        !needle.is_empty() && self.0.windows(needle.len()).any(|window| window == needle)
    }
}

/// Decode and capture the same single read, preserving original saved-format
/// bytes including whitespace rather than re-encoding the decoded project.
pub fn load_with_token(path: impl AsRef<Path>) -> Result<(Project, FileToken), StorageError> {
    let bytes = read_document(path.as_ref())?;
    let project = decode(&bytes)?;
    Ok((project, FileToken::from_validated_bytes(bytes.into())))
}

/// Read a project document or recovery sidecar of at most
/// [`crate::MAX_PROJECT_DOCUMENT_BYTES`] plus [`ENVELOPE_HEADROOM`].
pub(super) fn read_document(path: &Path) -> Result<Vec<u8>, StorageError> {
    read_bounded(path, crate::MAX_PROJECT_DOCUMENT_BYTES + ENVELOPE_HEADROOM)
}

/// Recovery sidecars frame a document with a header and its target file name.
const ENVELOPE_HEADROOM: usize = 64 * 1024;

/// Refuse oversized files from metadata before reading, then read at most one
/// byte past `limit`, so a file growing after the check is refused too.
fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, StorageError> {
    let file = File::open(path)?;
    let too_large = || {
        std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!(
                "{} exceeds the {} mib project document limit",
                crate::text::verbatim(path.display()),
                limit / (1024 * 1024)
            ),
        )
    };
    let limit_u64 = u64::try_from(limit).map_err(|_| too_large())?;
    if file.metadata()?.len() > limit_u64 {
        return Err(too_large().into());
    }
    let bytes = read_bytes(file.take(limit_u64.saturating_add(1)))?;
    if bytes.len() > limit {
        return Err(too_large().into());
    }
    Ok(bytes)
}

/// Grow owned I/O buffers fallibly in chunks; callers bound the reader.
fn read_bytes(mut reader: impl Read) -> Result<Vec<u8>, StorageError> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        let count = match reader.read(&mut chunk) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Ok(bytes);
        }
        bytes.try_reserve(count).map_err(std::io::Error::other)?;
        bytes.extend_from_slice(&chunk[..count]);
    }
}

/// Track a reserved sibling staging pathname without blindly unlinking it on
/// drop. Another process may have replaced the pathname with unrelated data, or
/// it may hold recoverable staged bytes after a pre-publication failure.
struct Temporary(PathBuf);

fn with_retained_stage(stage: &Path, source: std::io::Error) -> std::io::Error {
    let kind = source.kind();
    std::io::Error::new(
        kind,
        format!(
            "{source}; staging path {} was retained; inspect it before using or removing it",
            crate::text::verbatim(stage.display())
        ),
    )
}

/// Create a complete new project without overwriting another file. Failed writes
/// do not leave partial JSON at the target path; old targets always remain intact.
/// Volumes without hard links or exclusive rename publish an exclusive synced
/// copy instead, where only a crash during that copy can leave an incomplete
/// new file (see `crate::publication`).
/// A failure after linking reports `PublishedButNotDurable`; the complete target
/// is retained and callers must not treat that error as absence of a saved file.
pub fn save_new(path: impl AsRef<Path>, project: &Project) -> Result<(), StorageError> {
    save_new_with_token(path, project).map(|_| ())
}

/// Create a new project and return the exact serialized bytes published at its path.
/// The token is returned only after cleanup and directory durability succeed.
pub fn save_new_with_token(
    path: impl AsRef<Path>,
    project: &Project,
) -> Result<FileToken, StorageError> {
    durable_token(save_with_finish(
        path.as_ref(),
        project,
        finish_publication,
    )?)
}

fn durable_token(outcome: SaveNewOutcome) -> Result<FileToken, StorageError> {
    match outcome.warning {
        None => Ok(outcome.token),
        Some(warning) => Err(StorageError::PublishedButNotDurable {
            path: outcome.path,
            operation: warning.operation,
            source: warning.source,
        }),
    }
}

/// A new project that is complete and visible at `path`. `warning` reports a
/// failed directory sync after publication; the token still matches the
/// published bytes, so callers can bind the document. `retained_stage` is set
/// when publication did not consume the staging pathname (e.g. `fclonefileat`
/// or descriptor-bound exclusive copy); it is always an independent inode
/// isolated from `path` and is intentionally not unlinked by pathname.
#[derive(Debug)]
pub struct SaveNewOutcome {
    pub path: PathBuf,
    pub token: FileToken,
    pub warning: Option<ReplaceWarning>,
    pub retained_stage: Option<PathBuf>,
}

/// Create a new project like [`save_new_with_token`], but return a published
/// file with its token and durability warning instead of an error.
pub fn save_new_with_outcome(
    path: impl AsRef<Path>,
    project: &Project,
) -> Result<SaveNewOutcome, StorageError> {
    save_with_hooks(path.as_ref(), project, |_| {}, finish_publication)
}

type FinishError = (&'static str, std::io::Error);

fn save_with_finish(
    path: &Path,
    project: &Project,
    finish: impl FnOnce(&Path, &Path) -> Result<(), FinishError>,
) -> Result<SaveNewOutcome, StorageError> {
    save_with_hooks(path, project, |_| {}, finish)
}

fn save_with_hooks(
    path: &Path,
    project: &Project,
    before_publish: impl FnOnce(&Path),
    finish: impl FnOnce(&Path, &Path) -> Result<(), FinishError>,
) -> Result<SaveNewOutcome, StorageError> {
    if path.file_name().is_none() {
        return Err(StorageError::Invalid("project target needs a filename"));
    }
    // Every later save and checkpoint derives sibling names from this one, so
    // admit only a name they can all use. Callers preflight with
    // `project_name::check` for a message naming the file.
    if crate::project_name::check(path).is_err() {
        return Err(StorageError::Invalid(
            "project name is too long to keep recovery files beside it; choose a shorter name",
        ));
    }
    // Refuse known collisions before serialization or temporary-file work. The
    // final publication still refuses an existing destination against races.
    match fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "project destination already exists",
            )
            .into());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let bytes = encode(project)?;
    let mut temporary = None;
    for _ in 0..32 {
        let id = NEXT_TEMP.fetch_add(1, Relaxed);
        let candidate = parent.join(format!(".hex-pending-{}-{id}", std::process::id()));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                temporary = Some((Temporary(candidate), file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let (temporary, mut file) = temporary.ok_or(StorageError::Invalid(
        "could not reserve project temporary file",
    ))?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        return Err(with_retained_stage(&temporary.0, error).into());
    }
    before_publish(&temporary.0);
    let published =
        match crate::publication::publish_new_isolated_from_file(&mut file, &temporary.0, path) {
            Ok(published) => published,
            Err(error) => return Err(with_retained_stage(&temporary.0, error).into()),
        };
    drop(file);
    let retained_stage = published.staging_remains().then(|| temporary.0.clone());
    let warning = finish(&temporary.0, path)
        .err()
        .map(|(operation, source)| ReplaceWarning { operation, source });
    Ok(SaveNewOutcome {
        path: path.to_path_buf(),
        token: FileToken::from_validated_bytes(Arc::from(bytes)),
        warning,
        retained_stage,
    })
}

pub(crate) fn finish_publication(_temporary: &Path, path: &Path) -> Result<(), FinishError> {
    // Never unlink `temporary` by pathname after publication: exclusive rename
    // already consumed it, and non-consuming modes (`fclonefileat`, exclusive
    // copy) intentionally retain an isolated stage rather than risking deletion
    // of a concurrently substituted foreign file.
    #[cfg(unix)]
    let sync_error = {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        File::open(parent).and_then(|file| file.sync_all()).err()
    };
    #[cfg(not(unix))]
    let sync_error: Option<std::io::Error> = {
        let _ = path;
        None
    };
    if let Some(error) = sync_error {
        return Err(("synchronize the project directory", error));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let id = NEXT_TEMP.fetch_add(1, Relaxed);
            let path =
                std::env::temp_dir().join(format!("hex-save-finish-{}-{id}", std::process::id()));
            fs::create_dir(&path).expect("unique test directory");
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn sparse_multi_gigabyte_file_is_refused_before_reading() {
        let dir = TestDir::new();
        let path = dir.0.join("huge.hex");
        let file = File::create(&path).expect("sparse fixture");
        file.set_len(4 << 30).expect("sparse length");
        drop(file);
        let started = std::time::Instant::now();
        let Err(StorageError::Io(error)) = load(&path) else {
            panic!("oversized document must be refused as a file error");
        };
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(error.kind(), std::io::ErrorKind::FileTooLarge);
        let message = error.to_string();
        assert!(message.contains("huge.hex") && message.contains("512 mib"));
        assert_eq!(fs::metadata(&path).expect("kept").len(), 4 << 30);
    }

    #[test]
    fn bounded_read_accepts_the_limit_and_refuses_one_byte_more() {
        let dir = TestDir::new();
        let path = dir.0.join("bounded.hex");
        fs::write(&path, [b'x'; 16]).expect("fixture");
        assert_eq!(read_bounded(&path, 16).expect("at limit").len(), 16);
        let Err(StorageError::Io(error)) = read_bounded(&path, 15) else {
            panic!("one byte over the limit must be refused");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::FileTooLarge);
    }

    #[test]
    fn token_preserves_exact_loaded_bytes_and_detects_equivalent_json_reformatting() {
        let dir = TestDir::new();
        let path = dir.0.join("MixedCase.hex");
        let project = Project::new(48_000).expect("project");
        save_new(&path, &project).expect("save fixture");
        let bytes = fs::read(&path).expect("original bytes");
        let (loaded, token) = load_with_token(&path).expect("load with token");
        assert_eq!(loaded, project);
        token
            .clone()
            .check_unchanged(&path)
            .expect("unchanged clone");
        // Identity is intentionally content-based, not tied to an inode.
        fs::remove_file(&path).expect("remove owned fixture");
        fs::write(&path, &bytes).expect("identical new file");
        token
            .check_unchanged(&path)
            .expect("byte-identical replacement");
        let mut reformatted = bytes.clone();
        reformatted.push(b'\n');
        fs::write(&path, &reformatted).expect("external formatting edit");
        assert_eq!(load(&path).expect("same canonical project"), project);
        assert!(
            matches!(token.check_unchanged(&path), Err(StorageError::Conflict { path: changed }) if changed == path)
        );
        assert_eq!(fs::read(&path).expect("no mutation"), reformatted);
        assert!(!format!("{token:?}").contains("sample_rate"));
    }

    #[test]
    fn opaque_saturator_state_survives_project_save_and_load() {
        use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
        use hex_project::{Command, Created, DeviceSnapshot, Editor};

        let dir = TestDir::new();
        let path = dir.0.join("saturator.hex");
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let track = match editor
            .execute(Command::AddMidiTrack {
                name: "saturator persistence".into(),
            })
            .expect("track")
        {
            Created::Track(track) => track,
            _ => panic!("add midi track returned another entity"),
        };
        let mut snapshot = editor.project().snapshot();
        let device_id = snapshot.next_id;
        let mut payload = Vec::with_capacity(13);
        payload.extend_from_slice(&36.0_f32.to_le_bytes());
        payload.extend_from_slice(&0.0_f32.to_le_bytes());
        payload.extend_from_slice(&1.0_f32.to_le_bytes());
        payload.push(1);
        let track_snapshot = snapshot
            .tracks
            .iter_mut()
            .find(|candidate| candidate.id == track.value())
            .expect("new track");
        track_snapshot.devices.push(DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: device_id,
            bypassed: false,
            state: NativeDeviceState::new(DeviceTypeId::from_static("hex.saturator"), 1, payload)
                .expect("bounded opaque native state"),
        });
        snapshot.next_id = device_id + 1;
        let project = Project::from_snapshot(snapshot).expect("valid project state");

        save_new(&path, &project).expect("save project");
        let loaded = load(&path).expect("load project");
        assert_eq!(loaded, project);
        let device = loaded
            .tracks()
            .iter()
            .find(|candidate| candidate.id().value() == track.value())
            .and_then(|candidate| candidate.devices().first())
            .expect("saved Saturator");
        assert_eq!(device.state().type_id().as_str(), Ok("hex.saturator"));
        assert_eq!(device.state().state_version(), 1);
        assert_eq!(device.state().payload()[12], 1);
    }

    #[test]
    fn opaque_compressor_state_survives_project_save_and_load() {
        use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
        use hex_project::{Command, Created, DeviceSnapshot, Editor};

        let dir = TestDir::new();
        let path = dir.0.join("compressor.hex");
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let track = match editor
            .execute(Command::AddMidiTrack {
                name: "compressor persistence".into(),
            })
            .expect("track")
        {
            Created::Track(track) => track,
            _ => panic!("add midi track returned another entity"),
        };
        let mut snapshot = editor.project().snapshot();
        let device_id = snapshot.next_id;
        let mut payload = Vec::with_capacity(32);
        for value in [-42.0_f32, 8.0, 0.5, 250.0, 9.0, 3.0, 0.375] {
            payload.extend_from_slice(&value.to_le_bytes());
        }
        payload.extend_from_slice(&1_u32.to_le_bytes());
        let track_snapshot = snapshot
            .tracks
            .iter_mut()
            .find(|candidate| candidate.id == track.value())
            .expect("new track");
        track_snapshot.devices.push(DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: device_id,
            bypassed: false,
            state: NativeDeviceState::new(DeviceTypeId::from_static("hex.compressor"), 3, payload)
                .expect("bounded opaque native state"),
        });
        snapshot.next_id = device_id + 1;
        let project = Project::from_snapshot(snapshot).expect("valid project state");

        save_new(&path, &project).expect("save project");
        let loaded = load(&path).expect("load project");
        assert_eq!(loaded, project);
        let device = loaded
            .tracks()
            .iter()
            .find(|candidate| candidate.id().value() == track.value())
            .and_then(|candidate| candidate.devices().first())
            .expect("saved Compressor");
        assert_eq!(device.state().type_id().as_str(), Ok("hex.compressor"));
        assert_eq!(device.state().state_version(), 3);
        assert_eq!(&device.state().payload()[28..32], &1_u32.to_le_bytes());
        assert_eq!(&device.state().payload()[24..28], &0.375_f32.to_le_bytes());
        assert_eq!(
            device.state().payload(),
            project.tracks()[0].devices()[0].state().payload()
        );
    }

    #[test]
    fn opaque_gate_state_survives_project_save_and_load() {
        use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
        use hex_project::{Command, Created, DeviceSnapshot, Editor};

        let dir = TestDir::new();
        let path = dir.0.join("gate.hex");
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let track = match editor
            .execute(Command::AddMidiTrack {
                name: "gate persistence".into(),
            })
            .expect("track")
        {
            Created::Track(track) => track,
            _ => panic!("add midi track returned another entity"),
        };
        let mut snapshot = editor.project().snapshot();
        let device_id = snapshot.next_id;
        let mut payload = Vec::with_capacity(24);
        for value in [-24.0_f32, -36.0, -48.0, 2.5, 17.0, 300.0] {
            payload.extend_from_slice(&value.to_le_bytes());
        }
        let track_snapshot = snapshot
            .tracks
            .iter_mut()
            .find(|candidate| candidate.id == track.value())
            .expect("new track");
        track_snapshot.devices.push(DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: device_id,
            bypassed: false,
            state: NativeDeviceState::new(DeviceTypeId::from_static("hex.gate"), 1, payload)
                .expect("bounded opaque native state"),
        });
        snapshot.next_id = device_id + 1;
        let project = Project::from_snapshot(snapshot).expect("valid project state");

        save_new(&path, &project).expect("save project");
        let loaded = load(&path).expect("load project");
        assert_eq!(loaded, project);
        let device = loaded
            .tracks()
            .iter()
            .find(|candidate| candidate.id().value() == track.value())
            .and_then(|candidate| candidate.devices().first())
            .expect("saved Gate");
        assert_eq!(device.state().type_id().as_str(), Ok("hex.gate"));
        assert_eq!(device.state().state_version(), 1);
        assert_eq!(
            device.state().payload(),
            project.tracks()[0].devices()[0].state().payload()
        );
    }

    #[test]
    fn reverb_track_and_return_identity_routing_and_settings_save_reopen() {
        use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
        use hex_project::{Command, Created, Editor};
        let dir = TestDir::new();
        let path = dir.0.join("reverb.hex");
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "room".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack {
                name: "room return".into(),
            })
            .unwrap()
        else {
            panic!("return");
        };
        let payload = [2.1_f32, 73.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let state =
            NativeDeviceState::new(DeviceTypeId::from_static("hex.reverb"), 1, payload).unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: state.clone(),
            })
            .unwrap()
        else {
            panic!("device");
        };
        editor
            .execute(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state,
            })
            .unwrap();
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.7,
            })
            .unwrap();
        editor
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
        save_new(&path, editor.project()).unwrap();
        assert_eq!(load(&path).unwrap(), *editor.project());
    }

    #[test]
    fn opaque_auto_pan_state_survives_project_save_and_load() {
        use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
        use hex_project::{Command, Created, DeviceSnapshot, Editor};

        let dir = TestDir::new();
        let path = dir.0.join("auto-pan.hex");
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let track = match editor
            .execute(Command::AddMidiTrack {
                name: "auto pan persistence".into(),
            })
            .expect("track")
        {
            Created::Track(track) => track,
            _ => panic!("add midi track returned another entity"),
        };
        let mut snapshot = editor.project().snapshot();
        let device_id = snapshot.next_id;
        let mut payload = Vec::with_capacity(16);
        for value in [0.75_f32, 3.5, 270.0] {
            payload.extend_from_slice(&value.to_le_bytes());
        }
        payload.extend_from_slice(&3_u32.to_le_bytes());
        let track_snapshot = snapshot
            .tracks
            .iter_mut()
            .find(|candidate| candidate.id == track.value())
            .expect("new track");
        track_snapshot.devices.push(DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: device_id,
            bypassed: false,
            state: NativeDeviceState::new(DeviceTypeId::from_static("hex.auto-pan"), 2, payload)
                .expect("bounded opaque native state"),
        });
        snapshot.next_id = device_id + 1;
        let project = Project::from_snapshot(snapshot).expect("valid project state");

        save_new(&path, &project).expect("save project");
        let loaded = load(&path).expect("load project");
        assert_eq!(loaded, project);
        let device = loaded
            .tracks()
            .iter()
            .find(|candidate| candidate.id().value() == track.value())
            .and_then(|candidate| candidate.devices().first())
            .expect("saved Auto Pan");
        assert_eq!(device.state().type_id().as_str(), Ok("hex.auto-pan"));
        assert_eq!(device.state().state_version(), 2);
        assert_eq!(&device.state().payload()[12..16], &3_u32.to_le_bytes());
        assert_eq!(
            device.state().payload(),
            project.tracks()[0].devices()[0].state().payload()
        );
    }

    #[test]
    fn token_checks_original_current_bytes_instead_of_reencoded_formatting() {
        let dir = TestDir::new();
        let path = dir.0.join("formatted.hex");
        let project = Project::new(48_000).unwrap();
        let encoded = encode(&project).unwrap();
        let formatted = serde_json::to_vec_pretty(
            &serde_json::from_slice::<serde_json::Value>(&encoded).unwrap(),
        )
        .unwrap();
        assert_ne!(encoded, formatted);
        fs::write(&path, &formatted).expect("formatted fixture");
        let (loaded, token) = load_with_token(&path).expect("project and original token");
        assert_eq!(loaded, project);
        token.check_unchanged(&path).expect("original untouched");
        fs::write(&path, &encoded).expect("external formatting change");
        assert!(matches!(
            token.check_unchanged(&path),
            Err(StorageError::Conflict { .. })
        ));
    }

    #[test]
    fn token_loading_refuses_invalid_documents_and_comparison_detects_large_external_changes() {
        let dir = TestDir::new();
        let path = dir.0.join("bounded.hex");
        fs::write(&path, b"invalid json").expect("invalid fixture");
        assert!(load_with_token(&path).is_err());
        fs::remove_file(&path).expect("remove owned fixture");
        save_new(&path, &Project::new(48_000).expect("project")).expect("valid fixture");
        let (_, token) = load_with_token(&path).expect("token");
        fs::write(&path, vec![b' '; crate::LEGACY_READER_DOCUMENT_BYTES + 1])
            .expect("oversized fixture");
        assert!(load_with_token(&path).is_err());
        assert!(matches!(
            token.check_unchanged(&path),
            Err(StorageError::Conflict { .. })
        ));
    }

    #[test]
    fn postpublication_failure_keeps_complete_loadable_project_and_explains_outcome() {
        let dir = TestDir::new();
        let path = dir.0.join("saved.hex");
        let project = Project::new(48_000).expect("project");
        let outcome = save_with_finish(&path, &project, |temporary, destination| {
            assert_eq!(load(destination).expect("published document"), project);
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            assert!(!temporary.exists(), "exclusive rename consumed staging");
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            let _ = temporary;
            Err((
                "synchronize the project directory",
                std::io::Error::other("injected sync failure"),
            ))
        })
        .expect("published outcome");
        let warning = outcome.warning.as_ref().expect("durability warning");
        assert_eq!(warning.operation, "synchronize the project directory");
        // The returned token adopts the published file, so a session can bind
        // it and the next replacement succeeds.
        outcome
            .token
            .check_unchanged(&path)
            .expect("adoptable token");
        let token = outcome.token.clone();
        let error = durable_token(outcome).expect_err("durability failure must be explicit");
        assert!(
            matches!(&error, StorageError::PublishedButNotDurable { path: saved, .. } if saved == &path)
        );
        assert!(error.to_string().contains("project was published at"));
        assert!(
            error
                .source()
                .expect("underlying error")
                .to_string()
                .contains("injected sync failure")
        );
        assert_eq!(load(&path).expect("retained complete document"), project);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert_eq!(fs::read_dir(&dir.0).expect("directory").count(), 1);
        assert!(
            matches!(save_new(&path, &project), Err(StorageError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
        );
        let replacement = Project::new(44_100).expect("replacement");
        crate::replace_existing(&path, &token, &replacement).expect("next save");
        assert_eq!(load(&path).expect("replaced"), replacement);
    }

    /// Regression for #3915: exFAT/FAT/SMB volumes refuse hard links, which
    /// used to fail Save As and the first overwrite's recovery sidecar.
    #[cfg(unix)]
    #[test]
    fn volumes_without_hard_links_save_overwrite_and_recover() {
        use crate::publication::simulate::{Volume, volume};
        for (capability, expects_retained_stage) in [
            (Volume::NoHardLinks(libc::ENOTSUP), false),
            (Volume::NoHardLinks(libc::EPERM), false),
            (Volume::NoAtomicCreate, true),
        ] {
            let dir = TestDir::new();
            let _volume = volume(capability);
            let path = dir.0.join("usb.hex");
            let first = Project::new(48_000).expect("project");
            let outcome = save_new_with_outcome(&path, &first).expect("save as");
            assert_eq!(outcome.retained_stage.is_some(), expects_retained_stage);
            let token = durable_token(outcome).expect("durable token");
            assert_eq!(load(&path).expect("complete project"), first);
            assert!(
                matches!(save_new(&path, &first), Err(StorageError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
            );
            assert_eq!(load(&path).expect("untouched project"), first);
            let second = Project::new(44_100).expect("replacement");
            crate::replace_existing(&path, &token, &second).expect("first overwrite");
            assert_eq!(load(&path).expect("replaced"), second);
            let recovery = crate::load_recovery(&path).expect("recovery sidecar");
            assert_eq!(recovery.project, first);
            // Exclusive rename consumes staging (2 files); copy fallback safely
            // retains its isolated stage without unsafe pathname unlinking (3 files).
            let expected_count = if expects_retained_stage { 3 } else { 2 };
            assert_eq!(
                fs::read_dir(&dir.0).expect("directory").count(),
                expected_count
            );
        }
    }

    /// Regression for #8145: reproducing the retained hard-link alias mutation
    /// and verifying that `save_new_with_outcome` on a simulated link-only
    /// volume publishes an isolated independent inode instead of a shared
    /// writable hard link.
    #[cfg(unix)]
    #[test]
    fn link_only_volume_isolates_published_project_from_retained_stage_mutation() {
        use crate::publication::simulate::{Volume, volume};

        // First reproduce why a retained raw hard-link alias corrupts destination.
        let repro_dir = TestDir::new();
        let raw_stage = repro_dir.0.join(".raw-stage");
        let raw_dest = repro_dir.0.join("aliased.hex");
        let project = Project::new(48_000).expect("project");
        fs::write(&raw_stage, encode(&project).expect("encode")).expect("stage");
        fs::hard_link(&raw_stage, &raw_dest).expect("raw hard link");
        assert!(same_existing_file(&raw_stage, &raw_dest));
        fs::write(&raw_stage, b"corrupted via alias").expect("mutate stage");
        assert!(
            load(&raw_dest).is_err(),
            "retained raw hard link shares writable inode and corrupts destination"
        );

        // Actual save caller under simulated LinkOnly volume isolates inodes.
        let dir = TestDir::new();
        let _guard = volume(Volume::LinkOnly);
        let path = dir.0.join("isolated.hex");
        let outcome = save_new_with_outcome(&path, &project).expect("isolated link-only save");
        let retained = outcome
            .retained_stage
            .as_ref()
            .expect("retained stage must be reported truthfully");
        assert_eq!(load(&path).expect("loadable published project"), project);
        assert!(
            !same_existing_file(retained, &path),
            "published project must not share an inode with retained stage"
        );
        // Mutating the retained staging file must not corrupt the published project.
        fs::write(retained, b"corrupted stage bytes").expect("mutate retained stage");
        assert_eq!(
            load(&path).expect("published project remains intact"),
            project
        );
    }

    /// Regression for #8145 on macOS APFS: `fclonefileat` publishes an atomic,
    /// source-descriptor-bound CoW inode even when the staging pathname is
    /// swapped with foreign data right before publication, and mutating the
    /// retained stage leaves the published project untouched.
    #[cfg(target_os = "macos")]
    #[test]
    fn clone_only_volume_publishes_isolated_cow_project_from_open_fd_under_stage_swap() {
        use crate::publication::simulate::{Volume, volume};

        let dir = TestDir::new();
        let _guard = volume(Volume::CloneOnly);
        let path = dir.0.join("cloned.hex");
        let displaced = dir.0.join("displaced-stage");
        let project = Project::new(48_000).expect("project");
        let mut stage_seen = None;
        let outcome = save_with_hooks(
            &path,
            &project,
            |stage| {
                stage_seen = Some(stage.to_path_buf());
                fs::rename(stage, &displaced).expect("move stage aside");
                fs::write(stage, b"foreign bytes at staging path").expect("substitute foreign");
            },
            finish_publication,
        )
        .expect("APFS fclonefileat publishes from open fd");
        let stage = stage_seen.expect("stage reserved");
        assert_eq!(outcome.retained_stage.as_deref(), Some(stage.as_path()));
        assert_eq!(load(&path).expect("published from original fd"), project);
        assert_eq!(
            fs::read(&stage).expect("foreign file preserved"),
            b"foreign bytes at staging path"
        );
        assert!(displaced.is_file(), "displaced stage preserved");
        assert!(!same_existing_file(&displaced, &path));
        fs::write(&displaced, b"mutated original stage").expect("mutate displaced");
        assert_eq!(load(&path).expect("CoW clone remains isolated"), project);
    }

    /// Regression for #8144: replacing the stage pathname right after
    /// publication preserves foreign bytes, the published project, and any
    /// displaced stage across publication modes without unsafe pathname unlink.
    #[cfg(unix)]
    #[test]
    fn post_publication_stage_substitution_preserves_foreign_bytes_and_published_project() {
        use crate::publication::simulate::{Volume, volume};

        let modes: Vec<Option<Volume>> = vec![
            None,
            #[cfg(target_os = "macos")]
            Some(Volume::CloneOnly),
            Some(Volume::LinkOnly),
            Some(Volume::NoAtomicCreate),
        ];
        let project = Project::new(48_000).expect("project");
        for mode in modes {
            let dir = TestDir::new();
            let _guard = mode.map(volume);
            let path = dir.0.join("saved.hex");
            let displaced = dir.0.join("displaced-stage");
            let mut stage_path = None;
            let mut had_stage_to_displace = false;
            let outcome = save_with_hooks(
                &path,
                &project,
                |_| {},
                |temporary, destination| {
                    stage_path = Some(temporary.to_path_buf());
                    if temporary.exists() {
                        fs::rename(temporary, &displaced).expect("move stage aside");
                        had_stage_to_displace = true;
                    }
                    fs::write(temporary, b"foreign bytes after publication")
                        .expect("place foreign file at staging pathname");
                    finish_publication(temporary, destination)
                },
            )
            .expect("save should succeed");
            let stage_path = stage_path.expect("stage observed");
            assert_eq!(
                fs::read(&stage_path).expect("foreign file must survive"),
                b"foreign bytes after publication"
            );
            assert_eq!(load(&path).expect("published project survives"), project);
            if had_stage_to_displace {
                assert!(displaced.is_file(), "displaced stage must survive");
                assert_eq!(
                    outcome.retained_stage.as_deref(),
                    Some(stage_path.as_path())
                );
            } else {
                assert!(outcome.retained_stage.is_none());
            }
        }
    }

    /// Regression for #8144: pre-publication failure preserves substituted
    /// foreign bytes at the staging pathname instead of unlinking them in Drop,
    /// preserves an occupied destination, and reports the retained stage path.
    #[cfg(unix)]
    #[test]
    fn pre_publication_stage_substitution_preserves_foreign_bytes_on_error() {
        let dir = TestDir::new();
        let path = dir.0.join("occupied.hex");
        let displaced = dir.0.join("displaced-stage");
        let project = Project::new(48_000).expect("project");
        let mut stage_path = None;
        let error = save_with_hooks(
            &path,
            &project,
            |stage| {
                stage_path = Some(stage.to_path_buf());
                fs::rename(stage, &displaced).expect("move stage aside");
                fs::write(stage, b"foreign file at stage").expect("substitute stage");
                fs::write(&path, b"existing destination").expect("occupy destination");
            },
            finish_publication,
        )
        .expect_err("occupied destination must fail");
        let stage_path = stage_path.expect("stage reserved");
        let StorageError::Io(io_error) = &error else {
            panic!("expected IO AlreadyExists error, got {error:?}");
        };
        assert_eq!(io_error.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(
            error
                .to_string()
                .contains(&stage_path.to_string_lossy().to_string()),
            "error must report retained stage path: {error}"
        );
        assert_eq!(
            fs::read(&path).expect("existing destination untouched"),
            b"existing destination"
        );
        assert_eq!(
            fs::read(&stage_path).expect("foreign bytes at stage must survive Drop"),
            b"foreign file at stage"
        );
        assert!(displaced.is_file(), "displaced staged bytes preserved");
    }

    #[test]
    fn confirmed_replacement_only_overwrites_a_valid_project_and_keeps_recovery() {
        let dir = TestDir::new();
        let path = dir.0.join("existing.hex");
        let previous = Project::new(48_000).expect("project");
        save_new(&path, &previous).expect("existing project");
        let replacement = Project::new(44_100).expect("replacement");
        let outcome = crate::replace_confirmed(&path, &replacement).expect("confirmed save as");
        assert_eq!(load(&path).expect("replaced"), replacement);
        outcome.token.check_unchanged(&path).expect("bound token");
        assert_eq!(
            crate::load_recovery(&path).expect("recovery").project,
            previous
        );

        let other = dir.0.join("notes.hex");
        fs::write(&other, b"not a hex project").expect("foreign file");
        assert!(matches!(
            crate::replace_confirmed(&other, &replacement),
            Err(StorageError::Invalid(reason)) if reason.contains("choose another name")
        ));
        assert_eq!(fs::read(&other).expect("untouched"), b"not a hex project");
        assert!(!crate::recovery_path(&other).expect("sidecar path").exists());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_destination_is_a_collision_and_remains_intact() {
        let dir = TestDir::new();
        let path = dir.0.join("saved.hex");
        std::os::unix::fs::symlink("missing.hex", &path).expect("test symlink");
        let project = Project::new(48_000).expect("project");
        assert!(
            matches!(save_new(&path, &project), Err(StorageError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
        );
        assert_eq!(
            fs::read_link(path).expect("retained link"),
            Path::new("missing.hex")
        );
        assert_eq!(fs::read_dir(&dir.0).expect("directory").count(), 1);
    }

    #[test]
    fn same_existing_file_matches_hardlinks_and_distinguishes_copies_and_missing_paths() {
        let dir = TestDir::new();
        let original = dir.0.join("Original.hex");
        let hardlink = dir.0.join("Alias.hex");
        let copy = dir.0.join("Copy.hex");
        let missing = dir.0.join("Missing.hex");
        fs::write(&original, b"identical bytes").unwrap();
        fs::hard_link(&original, &hardlink).unwrap();
        fs::write(&copy, b"identical bytes").unwrap();

        assert!(same_existing_file(&original, &original));
        assert!(same_existing_file(&original, &hardlink));
        assert!(!same_existing_file(&original, &copy));
        assert!(!same_existing_file(&original, &missing));
        assert!(!same_existing_file(&missing, &missing));
    }
}
