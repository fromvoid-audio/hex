//! Eight named native tuning presets and bounded portable Freqs file I/O.
//! Paths are supplied by the control worker; this module owns no project edits.
//! Slot writes serialize cooperating Hex writers and recheck the existing
//! preset before publishing a complete temporary file with an atomic rename.
//! A noncooperating writer can still change a destination after that check;
//! the rename is not an atomic compare-and-swap. Existing recovery files are
//! never modified. The persistent empty lock file is not removed, so the OS can
//! safely release the same advisory lock when a process exits.
use hex_tuning::{
    Scale,
    preset::{self, MAX_PRESET_BYTES, Preset},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub const SLOT_COUNT: usize = 8;
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub number: usize,
    pub name: Option<String>,
    pub error: Option<String>,
}

fn slot_path(directory: &Path, slot: usize) -> Result<PathBuf, String> {
    if !(1..=SLOT_COUNT).contains(&slot) {
        return Err("tuning slot is outside 1–8".into());
    }
    Ok(directory.join(format!("slot-{slot}.freqs")))
}
pub fn read_file(path: &Path) -> Result<Preset, String> {
    read_validated(path).map(|(preset, _)| preset)
}
fn read_validated(path: &Path) -> Result<(Preset, Vec<u8>), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| format!("cannot inspect tuning preset · {e}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("tuning preset is not a regular file".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Also refuse a last-component symlink/FIFO swapped in after inspection.
        // Only constants are used: no workspace FFI or unsafe code.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("cannot open tuning preset · {e}"))?;
    if !file
        .metadata()
        .map_err(|e| format!("cannot inspect tuning preset · {e}"))?
        .is_file()
    {
        return Err("tuning preset is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_PRESET_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read tuning preset · {e}"))?;
    if bytes.len() > MAX_PRESET_BYTES {
        return Err("tuning preset exceeds size limit".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "tuning preset is not utf-8".to_owned())?;
    let preset = preset::decode(text).map_err(|e| e.to_string())?;
    Ok((preset, bytes))
}
pub fn list(directory: &Path) -> Vec<Slot> {
    (1..=SLOT_COUNT)
        .map(|number| {
            let path = directory.join(format!("slot-{number}.freqs"));
            match fs::symlink_metadata(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Slot {
                    number,
                    name: None,
                    error: None,
                },
                Err(e) => Slot {
                    number,
                    name: None,
                    error: Some(format!("cannot inspect tuning slot · {e}")),
                },
                Ok(_) => match read_file(&path) {
                    Ok(p) => Slot {
                        number,
                        name: Some(p.name),
                        error: None,
                    },
                    Err(error) => Slot {
                        number,
                        name: None,
                        error: Some(error),
                    },
                },
            }
        })
        .collect()
}
pub fn load(directory: &Path, slot: usize) -> Result<Preset, String> {
    read_file(&slot_path(directory, slot)?)
}
/// Replace an explicitly chosen slot only after validating both the new value
/// and any existing preset. Corrupt files remain available for recovery.
pub fn save(directory: &Path, slot: usize, name: &str, scale: Scale) -> Result<(), String> {
    save_with_hook(directory, slot, name, scale, || {})
}
fn save_with_hook(
    directory: &Path,
    slot: usize,
    name: &str,
    scale: Scale,
    before_check: impl FnOnce(),
) -> Result<(), String> {
    let path = slot_path(directory, slot)?;
    let text = preset::encode(name, scale).map_err(|e| e.to_string())?;
    fs::create_dir_all(directory).map_err(|e| format!("cannot create tuning library · {e}"))?;
    let _lock = lock_slot(&path)?;
    let expected = match fs::symlink_metadata(&path) {
        Ok(_) => Some(read_validated(&path)?.1),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("cannot inspect tuning slot · {e}")),
    };
    publish_slot(&path, text.as_bytes(), expected.as_deref(), before_check)
}
// Explicitly unlock before closing: a descriptor duplicated by a concurrent
// subprocess can otherwise keep the advisory lock alive past this operation.
struct SlotLock(File);

impl Drop for SlotLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

fn lock_slot(path: &Path) -> Result<SlotLock, String> {
    let lock_path = path.with_extension("freqs.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options
        .open(&lock_path)
        .map_err(|e| format!("cannot open tuning slot lock · {e}"))?;
    let opened = file
        .metadata()
        .map_err(|e| format!("cannot inspect tuning slot lock · {e}"))?;
    if !opened.is_file() || opened.len() != 0 {
        return Err("tuning slot lock is not an empty regular file".into());
    }
    fs2::FileExt::try_lock_exclusive(&file).map_err(|e| {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            "tuning slot is in use · try again".to_owned()
        } else {
            format!("cannot lock tuning slot · {e}")
        }
    })?;
    let lock = SlotLock(file);
    let current = fs::symlink_metadata(&lock_path)
        .map_err(|e| format!("cannot inspect tuning slot lock · {e}"))?;
    if current.file_type().is_symlink() || !current.is_file() || current.len() != 0 {
        return Err("tuning slot lock changed · try again".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != current.dev() || opened.ino() != current.ino() {
            return Err("tuning slot lock changed · try again".into());
        }
    }
    Ok(lock)
}
fn rename_with_hook(
    directory: &Path,
    slot: usize,
    name: &str,
    before_check: impl FnOnce(),
) -> Result<(), String> {
    let path = slot_path(directory, slot)?;
    let _lock = lock_slot(&path)?;
    let (old, expected) = read_validated(&path)?;
    let text = preset::encode(name, old.scale).map_err(|e| e.to_string())?;
    publish_slot(&path, text.as_bytes(), Some(&expected), before_check)
}
pub fn rename(directory: &Path, slot: usize, name: &str) -> Result<(), String> {
    rename_with_hook(directory, slot, name, || {})
}
pub fn duplicate(directory: &Path, slot: usize) -> Result<usize, String> {
    let old = load(directory, slot)?;
    let target = (1..=SLOT_COUNT)
        .find(|number| {
            matches!(fs::symlink_metadata(directory.join(format!("slot-{number}.freqs"))),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        })
        .ok_or("all tuning slots are full")?;
    let suffix = " copy";
    let max = 48 - suffix.len();
    let mut end = old.name.len().min(max);
    while !old.name.is_char_boundary(end) {
        end -= 1;
    }
    let name = format!("{}{suffix}", &old.name[..end]);
    let text = preset::encode(&name, old.scale).map_err(|e| e.to_string())?;
    publish(&slot_path(directory, target)?, text.as_bytes())?;
    Ok(target)
}
/// Export is create-new; an existing file is never silently replaced.
/// Publication is atomic, including on supported volumes without hard links.
/// Volumes without atomic create-new publication return an error.
pub fn export_file(path: &Path, name: &str, scale: Scale) -> Result<(), String> {
    let text = preset::encode(name, scale).map_err(|e| e.to_string())?;
    publish(path, text.as_bytes())
}
fn publish(destination: &Path, bytes: &[u8]) -> Result<(), String> {
    publish_with_guard(destination, bytes, || Ok(()))
}
fn publish_slot(
    destination: &Path,
    bytes: &[u8],
    expected: Option<&[u8]>,
    before_check: impl FnOnce(),
) -> Result<(), String> {
    if let Some(expected) = expected {
        return replace_existing(destination, bytes, expected, before_check);
    }
    publish_with_guard(destination, bytes, || {
        before_check();
        Ok(())
    })
}

fn replace_existing(
    destination: &Path,
    bytes: &[u8],
    expected: &[u8],
    before_check: impl FnOnce(),
) -> Result<(), String> {
    publish_staged_with(
        destination,
        bytes,
        Some(expected),
        || {
            before_check();
            Ok(())
        },
        |source, destination| fs::rename(source, destination),
    )
}

fn publish_with_guard(
    destination: &Path,
    bytes: &[u8],
    before_publish: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    publish_staged_with(
        destination,
        bytes,
        None,
        before_publish,
        |source, destination| fs::rename(source, destination),
    )
}

fn publish_staged_with(
    destination: &Path,
    bytes: &[u8],
    expected: Option<&[u8]>,
    before_publish: impl FnOnce() -> Result<(), String>,
    replace: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let parent = destination
        .parent()
        .map(|parent| {
            if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            }
        })
        .ok_or("tuning preset path has no parent")?;
    if expected.is_none() {
        match fs::symlink_metadata(destination) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err("tuning destination is not a regular file".into());
                }
                return Err("tuning destination already exists".into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect tuning destination · {error}")),
        }
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut temporary = None;
    for _ in 0..16 {
        let path = parent.join(format!(
            ".tuning-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                temporary = Some((path, file));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("cannot create tuning temporary file · {e}")),
        }
    }
    let (temporary, mut file) = temporary.ok_or("cannot reserve tuning temporary file")?;
    let result = (|| {
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("cannot write tuning preset · {e}"))?;
        drop(file);
        before_publish()?;
        if let Some(expected) = expected {
            let actual = read_validated(destination)
                .map_err(|_| "tuning slot changed · try again".to_owned())?
                .1;
            if actual != expected {
                return Err("tuning slot changed · try again".into());
            }
            replace(&temporary, destination)
                .map_err(|e| format!("cannot replace tuning preset atomically · {e}"))?;
        } else {
            let published = crate::publication::publish_new_atomic(&temporary, destination)
                .map_err(|e| format!("cannot publish new tuning preset atomically · {e}"))?;
            if published.staging_remains() {
                fs::remove_file(&temporary)
                    .map_err(|e| format!("tuning saved but temporary cleanup failed · {e}"))?;
            }
        }
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .or_else(|error| {
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput
                ) {
                    Ok(())
                } else {
                    Err(error)
                }
            })
            .map_err(|e| format!("tuning saved but directory sync failed · {e}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Directory(PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn directory() -> Directory {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "hex-tuning-library-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Directory(p)
    }
    #[test]
    fn repeated_slot_saves_keep_working() {
        let d = directory();
        for number in 1..=32 {
            fs::write(
                d.0.join(format!("slot-1-recovery-{number:02}.freqs")),
                b"preserve",
            )
            .unwrap();
        }
        for index in 0..34 {
            save(&d.0, 1, "tuning", Scale::equal(12))
                .unwrap_or_else(|error| panic!("save {index} failed: {error}"));
            rename(&d.0, 1, "renamed").unwrap();
        }
        assert_eq!(load(&d.0, 1).unwrap().name, "renamed");
        for number in 1..=32 {
            assert_eq!(
                fs::read(d.0.join(format!("slot-1-recovery-{number:02}.freqs"))).unwrap(),
                b"preserve"
            );
        }
        assert!(!d.0.join("slot-1-recovery-33.freqs").exists());
        assert!(!fs::read_dir(&d.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tuning-")
        }));
    }
    #[test]
    fn export_refuses_without_atomic_publication_and_cleans_up() {
        let d = directory();
        let _volume = crate::publication::simulate::volume(
            crate::publication::simulate::Volume::NoAtomicCreate,
        );
        let destination = d.0.join("export.freqs");
        assert!(export_file(&destination, "scale", Scale::equal(19)).is_err());
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 0);
    }
    #[test]
    fn destination_created_before_publication_is_preserved() {
        let d = directory();
        let destination = d.0.join("export.freqs");
        let result = publish_with_guard(&destination, b"new preset", || {
            fs::write(&destination, b"existing file").unwrap();
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"existing file");
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn slot_lock_releases_when_an_inherited_descriptor_remains_open() {
        let d = directory();
        let path = d.0.join("slot-1.freqs");
        let lock = lock_slot(&path).unwrap();
        let inherited = lock.0.try_clone().unwrap();
        assert!(lock_slot(&path).is_err(), "another writer must be excluded");
        drop(lock);
        let next = lock_slot(&path).expect("operation must release the slot lock");
        drop(next);
        drop(inherited);
    }
    #[test]
    fn save_preserves_a_slot_created_during_preparation() {
        let d = directory();
        let competing = b"another writer's tuning data";
        let result = save_with_hook(&d.0, 1, "new", Scale::default(), || {
            fs::write(d.0.join("slot-1.freqs"), competing).unwrap();
        });
        assert!(result.is_err());
        assert_eq!(fs::read(d.0.join("slot-1.freqs")).unwrap(), competing);
    }
    #[test]
    fn existing_slot_change_is_preserved_during_save_and_rename() {
        let d = directory();
        let path = d.0.join("slot-1.freqs");
        for rename in [false, true] {
            save(&d.0, 1, "original", Scale::equal(12)).unwrap();
            let competing = preset::encode("competing", Scale::equal(19)).unwrap();
            let change = || fs::write(&path, competing.as_bytes()).unwrap();
            let result = if rename {
                rename_with_hook(&d.0, 1, "renamed", change)
            } else {
                save_with_hook(&d.0, 1, "saved", Scale::equal(24), change)
            };
            assert!(result.is_err());
            assert_eq!(fs::read(&path).unwrap(), competing.as_bytes());
        }
    }
    #[test]
    fn failed_atomic_replacement_preserves_slot_and_cleans_temporary() {
        let d = directory();
        save(&d.0, 1, "old", Scale::equal(12)).unwrap();
        let path = d.0.join("slot-1.freqs");
        let original = fs::read(&path).unwrap();
        let new = preset::encode("new", Scale::equal(19)).unwrap();
        let error = publish_staged_with(
            &path,
            new.as_bytes(),
            Some(&original),
            || Ok(()),
            |_, _| Err(std::io::Error::other("injected publication failure")),
        )
        .unwrap_err();
        assert!(error.contains("cannot replace tuning preset atomically"));
        assert_eq!(fs::read(path).unwrap(), original);
        assert!(!fs::read_dir(&d.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tuning-")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn failed_staged_write_preserves_slot_and_cleans_temporary() {
        const CHILD: &str = "HEX_TUNING_WRITE_FAILURE_CHILD";
        if let Some(directory) = std::env::var_os(CHILD) {
            let error = save(Path::new(&directory), 1, "new", Scale::equal(19)).unwrap_err();
            assert!(error.contains("cannot write tuning preset"));
            return;
        }
        let d = directory();
        save(&d.0, 1, "old", Scale::equal(12)).unwrap();
        let path = d.0.join("slot-1.freqs");
        let original = fs::read(&path).unwrap();
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", "ulimit -f 0; trap '' XFSZ; exec \"$@\"", "hex-test"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tuning_library::tests::failed_staged_write_preserves_slot_and_cleans_temporary",
            ])
            .env(CHILD, &d.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(path).unwrap(), original);
        assert!(!fs::read_dir(&d.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tuning-")
        }));
    }

    #[test]
    fn interrupted_prepublication_keeps_original_and_complete_staged_candidate() {
        const CHILD: &str = "HEX_TUNING_PREPARE_CHILD";
        if let Some(directory) = std::env::var_os(CHILD) {
            let _ = save_with_hook(Path::new(&directory), 1, "new", Scale::equal(19), || {
                std::process::exit(0)
            });
            std::process::exit(1);
        }
        let d = directory();
        save(&d.0, 1, "old", Scale::equal(12)).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tuning_library::tests::interrupted_prepublication_keeps_original_and_complete_staged_candidate"])
            .env(CHILD, &d.0).status().unwrap();
        assert!(status.success());
        assert_eq!(load(&d.0, 1).unwrap().name, "old");
        let staged: Vec<_> = fs::read_dir(&d.0)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".tuning-")
            })
            .collect();
        assert_eq!(staged.len(), 1);
        assert_eq!(read_file(&staged[0]).unwrap().name, "new");
    }

    #[test]
    fn concurrent_hex_writer_is_refused_without_changing_slot() {
        const CHILD: &str = "HEX_TUNING_SLOT_LOCK_CHILD";
        if let Some(directory) = std::env::var_os(CHILD) {
            let directory = Path::new(&directory);
            assert!(save(directory, 1, "competing", Scale::equal(19)).is_err());
            assert!(rename(directory, 1, "competing").is_err());
            return;
        }
        let d = directory();
        save(&d.0, 1, "original", Scale::equal(12)).unwrap();
        let result = rename_with_hook(&d.0, 1, "renamed", || {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tuning_library::tests::concurrent_hex_writer_is_refused_without_changing_slot",
                ])
                .env(CHILD, &d.0)
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(750);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break Some(status);
                }
                if std::time::Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            };
            assert!(status.is_some_and(|status| status.success()));
        });
        result.unwrap();
        assert_eq!(load(&d.0, 1).unwrap().name, "renamed");
        assert_eq!(load(&d.0, 1).unwrap().scale, Scale::equal(12));
    }
    #[cfg(unix)]
    #[test]
    fn symlink_slot_lock_is_refused_without_touching_target() {
        let d = directory();
        let target = d.0.join("other");
        fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, d.0.join("slot-1.freqs.lock")).unwrap();
        assert!(save(&d.0, 1, "new", Scale::default()).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        assert!(!d.0.join("slot-1.freqs").exists());
    }
    #[test]
    fn export_to_bare_relative_filename() {
        const CHILD: &str = "HEX_TUNING_RELATIVE_EXPORT_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let path = Path::new("export.freqs");
            export_file(path, "relative", Scale::equal(19)).unwrap();
            let original = fs::read(path).unwrap();
            assert_eq!(read_file(path).unwrap().scale, Scale::equal(19));
            assert!(export_file(path, "replacement", Scale::equal(12)).is_err());
            assert_eq!(fs::read(path).unwrap(), original);
            return;
        }
        let d = directory();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tuning_library::tests::export_to_bare_relative_filename",
            ])
            .env(CHILD, "1")
            .current_dir(&d.0)
            .status()
            .unwrap();
        assert!(status.success());
    }
    #[cfg(unix)]
    #[test]
    fn review_followup_nonregular_preset_child() {
        let Some(directory) = std::env::var_os("HEX_TUNING_NONREGULAR_FIXTURE") else {
            return;
        };
        let slots = list(Path::new(&directory));
        assert!(slots[0].error.is_some());
        fs::write(Path::new(&directory).join("refused"), b"refused").unwrap();
        assert!(fs::symlink_metadata(Path::new(&directory).join("slot-1.freqs")).is_ok());
    }
    #[cfg(unix)]
    fn check_nonregular_preset(kind: &str) {
        use std::{
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        {
            let d = directory();
            let path = d.0.join("slot-1.freqs");
            if kind == "fifo" {
                assert!(
                    Command::new("mkfifo")
                        .arg(&path)
                        .status()
                        .unwrap()
                        .success()
                );
            } else {
                let source = d.0.join("original.freqs");
                fs::write(
                    &source,
                    preset::encode("original", Scale::equal(12)).unwrap(),
                )
                .unwrap();
                std::os::unix::fs::symlink(&source, &path).unwrap();
            }
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tuning_library::tests::review_followup_nonregular_preset_child",
                ])
                .env("HEX_TUNING_NONREGULAR_FIXTURE", &d.0)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            let result = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break Some(status);
                }
                if Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(
                result.is_some_and(|status| status.success()),
                "{kind} must refuse promptly without reading the target"
            );
            assert!(d.0.join("refused").exists());
        }
    }
    #[cfg(unix)]
    #[test]
    fn review_followup_fifo_preset_never_blocks_the_worker() {
        check_nonregular_preset("fifo");
    }
    #[cfg(unix)]
    #[test]
    fn review_followup_symlink_preset_preserves_the_target() {
        check_nonregular_preset("symlink");
    }
    #[cfg(unix)]
    #[test]
    fn review_final_duplicate_skips_preserved_dangling_symlink_slots() {
        let d = directory();
        save(&d.0, 1, "source", Scale::equal(19)).unwrap();
        let link = d.0.join("slot-2.freqs");
        let target = d.0.join("missing-original");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(duplicate(&d.0, 1).unwrap(), 3);
        assert_eq!(load(&d.0, 3).unwrap().scale, Scale::equal(19));
        assert_eq!(fs::read_link(&link).unwrap(), target);
        assert!(!target.exists());
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn export_and_new_slot_use_atomic_rename_without_hard_links() {
        let d = directory();
        let _volume = crate::publication::simulate::volume(
            crate::publication::simulate::Volume::NoHardLinks(libc::ENOTSUP),
        );
        let path = d.0.join("export.freqs");
        export_file(&path, "portable", Scale::equal(19)).unwrap();
        let original = fs::read(&path).unwrap();
        assert!(export_file(&path, "replacement", Scale::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1);
        save(&d.0, 1, "slot", Scale::equal(24)).unwrap();
        assert_eq!(load(&d.0, 1).unwrap().scale, Scale::equal(24));
        assert!(fs::read_dir(&d.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tuning-")
        }));
    }
    #[test]
    fn library_roundtrip_rename_duplicate_and_full_refusal() {
        let d = directory();
        let s = Scale::default();
        assert!(list(&d.0).iter().all(|s| s.name.is_none()));
        save(&d.0, 1, "major", s).unwrap();
        rename(&d.0, 1, "renamed").unwrap();
        assert_eq!(
            load(&d.0, 1).unwrap(),
            Preset {
                name: "renamed".into(),
                scale: s
            }
        );
        assert_eq!(duplicate(&d.0, 1).unwrap(), 2);
        assert_eq!(load(&d.0, 2).unwrap().name, "renamed copy");
        for slot in 3..=8 {
            save(&d.0, slot, "other", s).unwrap();
        }
        assert!(duplicate(&d.0, 1).is_err());
        assert!(load(&d.0, 0).is_err());
    }
    #[test]
    fn preserves_corrupt_slots_and_existing_exports_and_bounds_reads() {
        let d = directory();
        let p = d.0.join("slot-1.freqs");
        fs::write(&p, b"corrupt source").unwrap();
        assert!(save(&d.0, 1, "new", Scale::default()).is_err());
        assert_eq!(fs::read(&p).unwrap(), b"corrupt source");
        assert!(list(&d.0)[0].error.is_some());
        assert!(export_file(&p, "new", Scale::default()).is_err());
        assert_eq!(fs::read(&p).unwrap(), b"corrupt source");
        fs::write(d.0.join("too-large"), vec![b' '; MAX_PRESET_BYTES + 1]).unwrap();
        assert!(read_file(&d.0.join("too-large")).is_err());
        let output = d.0.join("export.freqs");
        export_file(&output, "export", Scale::equal(24)).unwrap();
        assert_eq!(read_file(&output).unwrap().scale, Scale::equal(24));
    }
}
