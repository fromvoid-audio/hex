//! Eight named native tuning presets and bounded portable Freqs file I/O.
//! Paths are supplied by the control worker; this module owns no project edits.
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
    let file = File::open(path).map_err(|e| format!("cannot open tuning preset · {e}"))?;
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
    preset::decode(text).map_err(|e| e.to_string())
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
    let path = slot_path(directory, slot)?;
    let text = preset::encode(name, scale).map_err(|e| e.to_string())?;
    if path
        .try_exists()
        .map_err(|e| format!("cannot inspect tuning slot · {e}"))?
    {
        read_file(&path)?;
    }
    fs::create_dir_all(directory).map_err(|e| format!("cannot create tuning library · {e}"))?;
    publish(&path, text.as_bytes(), true)
}
pub fn rename(directory: &Path, slot: usize, name: &str) -> Result<(), String> {
    let old = load(directory, slot)?;
    save(directory, slot, name, old.scale)
}
pub fn duplicate(directory: &Path, slot: usize) -> Result<usize, String> {
    let old = load(directory, slot)?;
    let target = (1..=SLOT_COUNT)
        .find(|number| {
            !directory
                .join(format!("slot-{number}.freqs"))
                .try_exists()
                .unwrap_or(true)
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
pub fn export_file(path: &Path, name: &str, scale: Scale) -> Result<(), String> {
    let text = preset::encode(name, scale).map_err(|e| e.to_string())?;
    publish(path, text.as_bytes(), false)
}
fn publish(destination: &Path, bytes: &[u8], replace: bool) -> Result<(), String> {
    let parent = destination
        .parent()
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
        if replace {
            fs::rename(&temporary, destination)
                .map_err(|e| format!("cannot publish tuning slot · {e}"))?;
        } else {
            fs::hard_link(&temporary, destination)
                .map_err(|e| format!("cannot publish new tuning preset · {e}"))?;
            fs::remove_file(&temporary)
                .map_err(|e| format!("tuning saved but temporary cleanup failed · {e}"))?;
        }
        #[cfg(unix)]
        File::open(parent)
            .and_then(|f| f.sync_all())
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
