//! Filesystem layout and publication for desktop project folders. The marker
//! distinguishes a project root from a general directory of standalone sets.
//! Project documents keep their existing encoding and relative asset paths.

use crate::{ReplaceWarning, SaveNewOutcome, StorageError, files, publication, replacement};
use hex_project::Project;
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering::Relaxed},
};

const MARKER: &str = ".hex-project";
pub const SAMPLES: &str = "samples";
static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct StageRetained {
    path: PathBuf,
    source: StorageError,
}

impl fmt::Display for StageRetained {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} · staging path {} was not removed by hex · inspect it before using or removing it",
            self.source,
            crate::text::verbatim(self.path.display())
        )
    }
}

impl std::error::Error for StageRetained {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn stage_retained(path: PathBuf, source: StorageError) -> StorageError {
    StorageError::Io(io::Error::other(StageRetained { path, source }))
}

/// Return the marked parent of a project document, if it has one.
pub fn root_for_document(path: &Path) -> io::Result<Option<&Path>> {
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    Ok(is_root(parent)?.then_some(parent))
}

pub fn is_root(root: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(root.join(MARKER)) {
        Ok(meta) if meta.is_file() && meta.len() == 0 => Ok(true),
        Ok(_) => Err(io::Error::other("project folder marker is invalid")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub enum Destination {
    File(PathBuf),
    Folder(NewFolder),
}

impl Destination {
    /// An exact existing file is the path the desktop confirmed replacing.
    /// All other names outside a marked folder reserve a new project folder.
    pub fn prepare(chosen: PathBuf, replace_existing: bool) -> Result<Self, StorageError> {
        Self::prepare_with_hook(chosen, replace_existing, |_| {})
    }

    fn prepare_with_hook(
        chosen: PathBuf,
        replace_existing: bool,
        after_reserve: impl FnOnce(&Path),
    ) -> Result<Self, StorageError> {
        if replace_existing || root_for_document(&chosen)?.is_some() {
            return Ok(Self::File(chosen));
        }
        match fs::symlink_metadata(&chosen) {
            Ok(_) => return Err(StorageError::Invalid("project destination already exists")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let parent = chosen.parent().ok_or(StorageError::Invalid(
            "project target needs a parent directory",
        ))?;
        let name = chosen
            .file_name()
            .ok_or(StorageError::Invalid("project target needs a filename"))?;
        let stem = chosen
            .file_stem()
            .ok_or(StorageError::Invalid("project target needs a name"))?;
        let root = parent.join(stem);
        match fs::symlink_metadata(&root) {
            Ok(_) => {
                return Err(StorageError::Invalid(
                    "project folder name already exists; open the folder in the save dialog or choose a new name",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let path = root.join(name);
        let path_bytes = path.as_os_str().len();
        if path_bytes > crate::project_name::MAX_PROJECT_PATH_BYTES {
            return Err(StorageError::Invalid(
                "project folder path exceeds the 4096-byte limit",
            ));
        }
        #[cfg(unix)]
        if path_bytes >= libc::PATH_MAX as usize {
            return Err(StorageError::Invalid(
                "project folder would make the saved path too long; choose a shorter name or location",
            ));
        }
        let folder = NewFolder {
            staging: reserve_stage(parent)?,
            path,
        };
        let stage = folder.root();
        after_reserve(stage);
        let initialized = (|| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(stage.join(MARKER))?
                .sync_all()?;
            fs::create_dir(stage.join(SAMPLES))
        })();
        if let Err(error) = initialized {
            return Err(stage_retained(folder.staging, error.into()));
        }
        Ok(Self::Folder(folder))
    }
}

/// A private sibling staging folder, published only after all files are ready.
/// Unpublished staging paths are retained because their contents or identity
/// may have changed; callers must report the path on pre-publication errors.
pub struct NewFolder {
    staging: PathBuf,
    path: PathBuf,
}

impl NewFolder {
    pub fn root(&self) -> &Path {
        &self.staging
    }

    pub fn publish(
        self,
        project: &Project,
        cancelled: impl FnOnce() -> bool,
    ) -> Result<SaveNewOutcome, StorageError> {
        let result = (|| {
            let name = self
                .path
                .file_name()
                .ok_or(StorageError::Invalid("project target needs a filename"))?;
            let root = self
                .path
                .parent()
                .ok_or(StorageError::Invalid("project target needs a folder"))?;
            let stage = self.root();
            let mut outcome = crate::save_new_with_outcome(stage.join(name), project)?;
            // At this point the document is private. If its stage could not be
            // synchronized, leave the chosen destination absent.
            if let Some(warning) = outcome.warning.take() {
                return Err(warning.source.into());
            }
            if cancelled() {
                return Err(StorageError::Invalid("project save cancelled"));
            }
            publication::rename_exclusive(stage, root)?;
            outcome.path = self.path.clone();
            outcome.warning = root
                .parent()
                .and_then(|parent| sync_directory(parent).err())
                .map(|source| ReplaceWarning {
                    operation: "synchronize the parent of the project folder",
                    source,
                });
            Ok(outcome)
        })();
        result.map_err(|source| stage_retained(self.staging, source))
    }
}

fn reserve_stage(parent: &Path) -> io::Result<PathBuf> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    for _ in 0..32 {
        let serial = NEXT_STAGE.fetch_add(1, Relaxed);
        let stage = parent.join(format!(".hex-project-{}-{serial}", std::process::id()));
        match builder.create(&stage) {
            Ok(()) => return Ok(stage),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other(
        "could not reserve a project staging folder",
    ))
}

/// Create the real subdirectories of a project sample path. Reject symlinks
/// and traversal so collecting or generating samples cannot escape the root.
pub fn sample_directory(root: &Path, subpath: &Path) -> io::Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in Path::new(SAMPLES).components().chain(subpath.components()) {
        let Component::Normal(component) = component else {
            return Err(io::Error::other("invalid project sample path"));
        };
        let parent = path.clone();
        path.push(component);
        match fs::create_dir(&path) {
            Ok(()) => sync_directory(&parent)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if !fs::symlink_metadata(&path)?.is_dir() {
                    return Err(io::Error::other(
                        "project sample folder is not a real directory",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(path)
}

/// Place immutable generated audio in the project without replacing another
/// sample. A repeated save can reuse an exact copy. Stream through a sibling
/// staging file so failure leaves any existing sample intact.
pub fn collect_sample(
    root: &Path,
    relative: &Path,
    source: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<(), StorageError> {
    let name = relative
        .file_name()
        .ok_or(StorageError::Invalid("project sample needs a filename"))?;
    let parent = relative
        .parent()
        .ok_or(StorageError::Invalid("project sample needs a parent"))?;
    let directory = sample_directory(root, parent)?;
    let target = directory.join(name);
    if target == source {
        return Ok(());
    }
    if !fs::symlink_metadata(source)?.is_file() {
        return Err(StorageError::Invalid(
            "generated sample is not a regular file",
        ));
    }
    let mut input = File::open(source)?;
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_file() && meta.len() == input.metadata()?.len() => {
            if same_bytes(&mut input, File::open(&target)?, &cancelled)? {
                return Ok(());
            }
            return Err(StorageError::Invalid(
                "project sample name is already in use",
            ));
        }
        Ok(_) => {
            return Err(StorageError::Invalid(
                "project sample name is already in use",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let len = input.metadata()?.len();
    let temporary = replacement::prepare_temp_with(&directory, None, |file| {
        let mut input = BufReader::new(input.take(len));
        loop {
            check_cancelled(&cancelled)?;
            let bytes = input.fill_buf()?;
            if bytes.is_empty() {
                break;
            }
            file.write_all(bytes)?;
            let count = bytes.len();
            input.consume(count);
        }
        if input.into_inner().limit() != 0 {
            return Err(io::Error::other("generated sample changed while saving"));
        }
        Ok(())
    })?;
    check_cancelled(&cancelled)?;
    publication::publish_new(&temporary.path, &target)?;
    files::finish_publication(&temporary.path, &target).map_err(|(_, error)| error.into())
}

fn same_bytes(a: &mut File, b: File, cancelled: &impl Fn() -> bool) -> io::Result<bool> {
    let mut a = BufReader::new(a);
    let mut b = BufReader::new(b);
    loop {
        check_cancelled(cancelled)?;
        let a_buf = a.fill_buf()?;
        let b_buf = b.fill_buf()?;
        let len = a_buf.len().min(b_buf.len());
        if len == 0 {
            return Ok(a_buf.is_empty() && b_buf.is_empty());
        }
        if a_buf[..len] != b_buf[..len] {
            return Ok(false);
        }
        a.consume(len);
        b.consume(len);
    }
}

fn check_cancelled(cancelled: &impl Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(io::Error::other("project save cancelled"))
    } else {
        Ok(())
    }
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    struct Files(PathBuf);
    impl Files {
        fn new() -> Self {
            let path = reserve_stage(&std::env::temp_dir()).unwrap();
            Self(path)
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn new_folder_publishes_complete_set_and_accepts_sibling_sets() {
        let files = Files::new();
        let chosen = files.0.join("My Set.hex");
        let Destination::Folder(folder) = Destination::prepare(chosen.clone(), false).unwrap()
        else {
            panic!("a new set should reserve a folder");
        };
        let original = files.0.join("original.wav");
        fs::write(&original, b"sample bytes").unwrap();
        collect_sample(
            folder.root(),
            Path::new("bounces/audio.wav"),
            &original,
            || false,
        )
        .unwrap();
        assert!(!chosen.exists());
        assert!(!files.0.join("My Set").exists());
        let project = Project::new(48_000).unwrap();
        let saved = folder.publish(&project, || false).unwrap();
        let root = files.0.join("My Set");
        assert_eq!(saved.path, root.join("My Set.hex"));
        assert_eq!(crate::load(&saved.path).unwrap(), project);
        assert_eq!(
            fs::read(root.join("samples/bounces/audio.wav")).unwrap(),
            b"sample bytes"
        );
        let version = root.join("Version 2.hex");
        assert!(matches!(
            Destination::prepare(version.clone(), false).unwrap(),
            Destination::File(path) if path == version
        ));
        assert!(Destination::prepare(chosen, false).is_err());
    }

    #[test]
    fn replaced_staging_path_does_not_delete_other_files_on_drop() {
        let files = Files::new();
        let Destination::Folder(folder) =
            Destination::prepare(files.0.join("Song.hex"), false).unwrap()
        else {
            panic!("expected a new folder");
        };
        let stage = folder.root().to_path_buf();
        let original_stage = files.0.join("original-stage");
        fs::rename(&stage, &original_stage).unwrap();
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("other-work"), b"preserve me").unwrap();
        drop(folder);
        assert_eq!(fs::read(stage.join("other-work")).unwrap(), b"preserve me");
        assert!(original_stage.join(MARKER).exists());
    }

    #[test]
    fn failed_stage_initialization_preserves_existing_marker_and_reports_path() {
        let files = Files::new();
        let mut stage = None;
        let result = Destination::prepare_with_hook(files.0.join("Song.hex"), false, |path| {
            stage = Some(path.to_path_buf());
            fs::write(path.join(MARKER), b"other work").unwrap();
        });
        let error = result.err().expect("occupied marker must fail");
        let stage = stage.expect("staging folder reserved");
        assert_eq!(fs::read(stage.join(MARKER)).unwrap(), b"other work");
        assert!(
            error
                .to_string()
                .contains(&stage.to_string_lossy().to_string())
        );
        assert!(!files.0.join("Song").exists());
    }

    #[test]
    fn a_folder_race_never_replaces_the_existing_destination() {
        let files = Files::new();
        let chosen = files.0.join("Song.hex");
        let Destination::Folder(folder) = Destination::prepare(chosen, false).unwrap() else {
            panic!("expected a new folder");
        };
        let stage = folder.root().to_path_buf();
        let occupied = files.0.join("Song");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("existing"), b"keep").unwrap();
        let error = folder
            .publish(&Project::new(48_000).unwrap(), || false)
            .expect_err("occupied destination must fail");
        assert!(
            error
                .to_string()
                .contains(&stage.to_string_lossy().to_string())
        );
        assert!(stage.join("Song.hex").is_file());
        assert_eq!(fs::read(occupied.join("existing")).unwrap(), b"keep");
        assert!(!occupied.join("Song.hex").exists());

        let chosen = files.0.join("Cancelled.hex");
        let Destination::Folder(folder) = Destination::prepare(chosen, false).unwrap() else {
            panic!("expected a new folder");
        };
        let stage = folder.root().to_path_buf();
        let error = folder
            .publish(&Project::new(48_000).unwrap(), || true)
            .expect_err("cancelled save must fail");
        assert!(
            error
                .to_string()
                .contains(&stage.to_string_lossy().to_string())
        );
        assert!(stage.join("Cancelled.hex").is_file());
        assert!(!files.0.join("Cancelled").exists());
    }

    #[test]
    fn derived_path_must_fit_even_when_the_chosen_filename_fits() {
        let files = Files::new();
        let mut parent = files.0.clone();
        let parent_length = libc::PATH_MAX as usize - 200;
        while parent.as_os_str().len() < parent_length {
            let count = (parent_length - parent.as_os_str().len() - 1).min(100);
            parent.push("p".repeat(count.max(1)));
            fs::create_dir(&parent).unwrap();
        }
        let chosen = parent.join(format!("{}.hex", "x".repeat(120)));
        assert!(chosen.as_os_str().len() < libc::PATH_MAX as usize);
        let error = Destination::prepare(chosen, false).err().unwrap();
        assert!(error.to_string().contains("project folder"), "{error}");
        assert!(fs::read_dir(parent).unwrap().next().is_none());
    }

    #[test]
    fn collection_reuses_exact_bytes_and_refuses_collisions_and_cancelled_copies() {
        let files = Files::new();
        let source = files.0.join("original.wav");
        let name = Path::new("recorded/take.wav");
        let root = files.0.join("project");
        fs::create_dir(&root).unwrap();
        fs::write(&source, b"same").unwrap();
        assert!(collect_sample(&root, name, &source, || true).is_err());
        assert!(!root.join(SAMPLES).join(name).exists());
        for _ in 0..2 {
            collect_sample(&root, name, &source, || false).unwrap();
        }
        fs::write(&source, b"new!").unwrap();
        assert!(collect_sample(&root, name, &source, || false).is_err());
        assert_eq!(fs::read(root.join(SAMPLES).join(name)).unwrap(), b"same");
        assert!(collect_sample(&root, Path::new("../escape.wav"), &source, || false).is_err());
    }
}
