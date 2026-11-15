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

/// Opaque exact-byte content snapshot, shared cheaply between owners.
/// This is a content check, not a filesystem lock, identity or overwrite permit.
#[derive(Clone)]
pub struct FileToken(pub(super) Arc<[u8]>, Option<u32>);

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

    /// True when saving `project` writes a newer schema than this file's, for
    /// example adding sinc v2 media to a schema63 file. Compares the actual
    /// saved formats, not application versions.
    pub fn requires_schema_upgrade_for(&self, project: &Project) -> bool {
        self.requires_format_upgrade()
            || self
                .1
                .is_none_or(|version| version < crate::schema_version_for(project))
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
        }
        let version = serde_json::from_slice::<Header>(&bytes)
            .ok()
            .map(|header| header.version);
        Self(bytes, version)
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

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
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
/// failed temporary cleanup or directory sync after publication; the token
/// still matches the published bytes, so callers can bind the document.
#[derive(Debug)]
pub struct SaveNewOutcome {
    pub path: PathBuf,
    pub token: FileToken,
    pub warning: Option<ReplaceWarning>,
}

/// Create a new project like [`save_new_with_token`], but return a published
/// file with its token and durability warning instead of an error.
pub fn save_new_with_outcome(
    path: impl AsRef<Path>,
    project: &Project,
) -> Result<SaveNewOutcome, StorageError> {
    save_with_finish(path.as_ref(), project, finish_publication)
}

type FinishError = (&'static str, std::io::Error);

fn save_with_finish(
    path: &Path,
    project: &Project,
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
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
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
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    crate::publication::publish_new(&temporary.0, path)?;
    let warning = finish(&temporary.0, path)
        .err()
        .map(|(operation, source)| ReplaceWarning { operation, source });
    Ok(SaveNewOutcome {
        path: path.to_path_buf(),
        token: FileToken::from_validated_bytes(Arc::from(bytes)),
        warning,
    })
}

pub(crate) fn finish_publication(temporary: &Path, path: &Path) -> Result<(), FinishError> {
    // Attempt directory sync even when temporary cleanup failed. Neither step
    // may roll back a destination that has already been published.
    // An exclusive-rename publication already consumed the staging name.
    let cleanup_error = fs::remove_file(temporary)
        .err()
        .filter(|error| error.kind() != std::io::ErrorKind::NotFound);
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
    if let Some(error) = cleanup_error {
        return Err(("remove the temporary project file", error));
    }
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
            fs::remove_file(temporary).expect("cleanup succeeds");
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
        for capability in [
            Volume::NoHardLinks(libc::ENOTSUP),
            Volume::NoHardLinks(libc::EPERM),
            Volume::NoAtomicCreate,
        ] {
            let dir = TestDir::new();
            let _volume = volume(capability);
            let path = dir.0.join("usb.hex");
            let first = Project::new(48_000).expect("project");
            let token = save_new_with_token(&path, &first).expect("save as");
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
            // Only the project and its sidecar remain: no staging leftovers.
            assert_eq!(fs::read_dir(&dir.0).expect("directory").count(), 2);
        }
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
}
