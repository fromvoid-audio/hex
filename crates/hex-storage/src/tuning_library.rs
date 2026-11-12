//! Eight named native tuning presets and bounded portable Freqs file I/O.
//! Paths are supplied by the control worker; this module owns no project edits.
//! Slot writes hold a persistent per-slot advisory lock and recheck the bytes
//! read before replacement. Older Hex versions and external writers do not
//! honor that lock; they can still race the final check and rename. This is not
//! an atomic compare-and-swap, and such a writer's changes can be lost.
//! The empty lock file is never removed: its open handle is held until the
//! operation finishes, and the OS releases the lock if the process exits.
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
fn lock_slot(path: &Path) -> Result<File, String> {
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
    Ok(file)
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
    publish(&slot_path(directory, target)?, text.as_bytes(), false)?;
    Ok(target)
}
/// Export is create-new; an existing file is never silently replaced.
/// Filesystems without hardlinks use an exclusively created destination. A failed
/// write may leave that new file incomplete; destinations are never deleted.
pub fn export_file(path: &Path, name: &str, scale: Scale) -> Result<(), String> {
    let text = preset::encode(name, scale).map_err(|e| e.to_string())?;
    publish(path, text.as_bytes(), false)
}
fn publish(destination: &Path, bytes: &[u8], replace: bool) -> Result<(), String> {
    publish_with_link(destination, bytes, replace, |source, target| {
        fs::hard_link(source, target)
    })
}
fn publish_slot(
    destination: &Path,
    bytes: &[u8],
    expected: Option<&[u8]>,
    before_check: impl FnOnce(),
) -> Result<(), String> {
    publish_with_guard(
        destination,
        bytes,
        expected.is_some(),
        |source, target| fs::hard_link(source, target),
        || {
            before_check();
            if let Some(expected) = expected {
                let actual = read_validated(destination)
                    .map_err(|_| "tuning slot changed · try again".to_owned())?
                    .1;
                if actual != expected {
                    return Err("tuning slot changed · try again".into());
                }
            }
            Ok(())
        },
    )
}
fn publish_with_link(
    destination: &Path,
    bytes: &[u8],
    replace: bool,
    link: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), String> {
    publish_with_guard(destination, bytes, replace, link, || Ok(()))
}
fn publish_with_guard(
    destination: &Path,
    bytes: &[u8],
    replace: bool,
    link: impl Fn(&Path, &Path) -> std::io::Result<()>,
    before_publish: impl FnOnce() -> Result<(), String>,
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
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("tuning destination is not a regular file".into());
        }
        if !replace {
            return Err("tuning destination already exists".into());
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
        if replace {
            fs::rename(&temporary, destination)
                .map_err(|e| format!("cannot publish tuning slot · {e}"))?;
        } else {
            if link(&temporary, destination).is_err() {
                let mut destination = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(destination)
                    .map_err(|e| format!("cannot publish new tuning preset · {e}"))?;
                destination.write_all(bytes).and_then(|()| destination.sync_all())
                    .map_err(|e| format!("new tuning preset could not be completed · incomplete file retained · {e}"))?;
            }
            fs::remove_file(&temporary)
                .map_err(|e| format!("tuning saved but temporary cleanup failed · {e}"))?;
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
    #[test]
    fn review_create_new_export_does_not_require_hardlinks_or_replace_existing_files() {
        let d = directory();
        let path = d.0.join("removable.freqs");
        let text = preset::encode("portable", Scale::equal(19)).unwrap();
        let no_links =
            |_: &Path, _: &Path| Err(std::io::Error::from(std::io::ErrorKind::Unsupported));
        publish_with_link(&path, text.as_bytes(), false, no_links).unwrap();
        assert_eq!(read_file(&path).unwrap().scale, Scale::equal(19));
        assert!(publish_with_link(&path, b"replacement", false, no_links).is_err());
        assert_eq!(fs::read(&path).unwrap(), text.as_bytes());
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1);
        let raced = d.0.join("raced.freqs");
        let competing_writer = |_: &Path, target: &Path| {
            fs::write(target, b"another writer's file")?;
            Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
        };
        assert!(publish_with_link(&raced, text.as_bytes(), false, competing_writer).is_err());
        assert_eq!(fs::read(&raced).unwrap(), b"another writer's file");
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
