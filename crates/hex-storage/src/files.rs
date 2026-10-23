use crate::{MAX_DOCUMENT_BYTES, StorageError, decode, encode};
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

/// Opaque bounded exact-byte content snapshot, shared cheaply between owners.
/// This is a content check, not a filesystem lock, identity or overwrite permit.
#[derive(Clone)]
pub struct FileToken(pub(super) Arc<[u8]>);

impl std::fmt::Debug for FileToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileToken")
            .field("byte_count", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl FileToken {
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

/// Decode and capture the same single bounded read, preserving original current-format
/// bytes including whitespace rather than re-encoding the decoded project.
pub fn load_with_token(path: impl AsRef<Path>) -> Result<(Project, FileToken), StorageError> {
    let bytes = read_document(path.as_ref())?;
    let project = decode(&bytes)?;
    Ok((project, FileToken(bytes.into())))
}

pub(super) fn read_document(path: &Path) -> Result<Vec<u8>, StorageError> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(StorageError::Invalid("project document exceeds byte limit"));
    }
    Ok(bytes)
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Create a complete new project without overwriting another file. Failed writes
/// cannot expose partial JSON at the target path; old targets always remain intact.
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
    save_with_finish(path.as_ref(), project, finish_publication)
}

type FinishError = (&'static str, std::io::Error);

fn save_with_finish(
    path: &Path,
    project: &Project,
    finish: impl FnOnce(&Path, &Path) -> Result<(), FinishError>,
) -> Result<FileToken, StorageError> {
    if path.file_name().is_none() {
        return Err(StorageError::Invalid("project target needs a filename"));
    }
    // Refuse known collisions before serialization or temporary-file work. The
    // final hard link still enforces create-new atomically against races.
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
    fs::hard_link(&temporary.0, path)?;
    finish(&temporary.0, path).map_err(|(operation, source)| {
        StorageError::PublishedButNotDurable {
            path: path.to_path_buf(),
            operation,
            source,
        }
    })?;
    Ok(FileToken(Arc::from(bytes)))
}

fn finish_publication(temporary: &Path, path: &Path) -> Result<(), FinishError> {
    // Attempt directory sync even when temporary cleanup failed. Neither step
    // may roll back a destination that has already been published.
    let cleanup_error = fs::remove_file(temporary).err();
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
    fn token_loading_and_comparison_refuse_invalid_or_oversized_documents() {
        let dir = TestDir::new();
        let path = dir.0.join("bounded.hex");
        fs::write(&path, b"invalid json").expect("invalid fixture");
        assert!(load_with_token(&path).is_err());
        fs::remove_file(&path).expect("remove owned fixture");
        save_new(&path, &Project::new(48_000).expect("project")).expect("valid fixture");
        let (_, token) = load_with_token(&path).expect("token");
        fs::write(&path, vec![b' '; MAX_DOCUMENT_BYTES + 1]).expect("oversized fixture");
        assert!(matches!(
            load_with_token(&path),
            Err(StorageError::Invalid("project document exceeds byte limit"))
        ));
        assert!(matches!(
            token.check_unchanged(&path),
            Err(StorageError::Invalid("project document exceeds byte limit"))
        ));
    }

    #[test]
    fn postpublication_failure_keeps_complete_loadable_project_and_explains_outcome() {
        let dir = TestDir::new();
        let path = dir.0.join("saved.hex");
        let project = Project::new(48_000).expect("project");
        let error = save_with_finish(&path, &project, |temporary, destination| {
            assert_eq!(load(destination).expect("published document"), project);
            fs::remove_file(temporary).expect("cleanup succeeds");
            Err((
                "synchronize the project directory",
                std::io::Error::other("injected sync failure"),
            ))
        })
        .expect_err("durability failure must be explicit");
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
