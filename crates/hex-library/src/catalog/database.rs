//! Versioned, bounded, native-path derived cache, atomically replaced on success.
//!
//! Version 2 adds an optional [`SampleRecord`] (size, mtime, fingerprint) after
//! each path. Version 1 caches still load, as entries without records, and are
//! rewritten as version 2 after the next scan. Unknown versions are refused and
//! rebuilt; [`SampleCatalog::preserve_corrupt`] keeps the refused bytes aside.
use super::{
    Fingerprint, MAX_CATALOG_BYTES, MAX_PATH_BYTES, MAX_SAMPLES, SampleCatalog, SampleRecord,
};
use crate::{EntryKind, LibraryEntry, is_wav};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(unix)]
const PLATFORM: u8 = b'u';
#[cfg(windows)]
const PLATFORM: u8 = b'w';
#[cfg(not(any(unix, windows)))]
const PLATFORM: u8 = b't';
const MAGIC_V1: [u8; 8] = *b"hexsmp1\0";
const MAGIC_V2: [u8; 8] = *b"hexsmp2\0";
const NO_RECORD: u8 = 0;
const RECORD_UNREADABLE: u8 = 1;
const RECORD_FINGERPRINTED: u8 = 2;
const TEMPORARY_ATTEMPTS: usize = 32;

fn magic(base: [u8; 8]) -> [u8; 8] {
    let mut magic = base;
    magic[7] = PLATFORM;
    magic
}
static NEXT: AtomicU64 = AtomicU64::new(0);
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid sample database; refresh to rebuild",
    )
}

impl SampleCatalog {
    pub fn load(path: &Path) -> io::Result<Self> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() > MAX_CATALOG_BYTES as u64 {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        file.take(MAX_CATALOG_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_CATALOG_BYTES {
            return Err(invalid());
        }
        let mut reader = bytes.as_slice();
        let mut found = [0; 8];
        reader.read_exact(&mut found)?;
        let version = if found == magic(MAGIC_V2) {
            2
        } else if found == magic(MAGIC_V1) {
            1
        } else {
            return Err(invalid());
        };
        let count = read_u32(&mut reader)? as usize;
        if count > MAX_SAMPLES {
            return Err(invalid());
        }
        let mut entries = Vec::with_capacity(count);
        let mut records = std::collections::HashMap::new();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..count {
            let length = read_u32(&mut reader)? as usize;
            if length == 0 || length > MAX_PATH_BYTES || length > reader.len() {
                return Err(invalid());
            }
            let (path, remaining) = reader.split_at(length);
            reader = remaining;
            let path = decode_path(path)?;
            if !path.is_absolute() || !is_wav(path.extension()) || !seen.insert(path.clone()) {
                return Err(invalid());
            }
            let name = path
                .file_name()
                .ok_or_else(invalid)?
                .to_string_lossy()
                .into_owned();
            if version >= 2
                && let Some(record) = read_record(&mut reader)?
            {
                records.insert(path.clone(), record);
            }
            entries.push(LibraryEntry {
                name,
                path,
                kind: EntryKind::WaveAudio,
            });
        }
        if !reader.is_empty() {
            return Err(invalid());
        }
        Ok(Self { entries, records })
    }

    /// This is a disposable index, never a project or user preferences file.
    /// A failed write leaves the previous complete cache intact.
    ///
    /// Returns `Ok(false)` without writing, syncing or touching the file's
    /// modification time when `path` already holds exactly these encoded bytes,
    /// so periodic rescans of an unchanged library cost no durable writes.
    pub fn save(&self, path: &Path) -> io::Result<bool> {
        if self.entries.len() > MAX_SAMPLES {
            return Err(invalid());
        }
        let mut bytes = Vec::from(magic(MAGIC_V2).as_slice());
        bytes.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            let raw = encode_path(&entry.path);
            if raw.is_empty()
                || raw.len() > MAX_PATH_BYTES
                || bytes.len() + raw.len() + 4 > MAX_CATALOG_BYTES
            {
                return Err(invalid());
            }
            bytes.extend_from_slice(&(raw.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&raw);
            write_record(&mut bytes, self.records.get(&entry.path));
            if bytes.len() > MAX_CATALOG_BYTES {
                return Err(invalid());
            }
        }
        if holds_exactly(path, &bytes) {
            return Ok(false);
        }
        let name = path.file_name().ok_or_else(invalid)?;
        let parent = normalized_parent(path);
        fs::create_dir_all(parent)?;
        let (temporary, mut file) = create_temporary(parent, name, &NEXT)?;
        let result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)?;
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result.map(|()| true)
    }
}

fn normalized_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn create_temporary(
    parent: &Path,
    name: &std::ffi::OsStr,
    counter: &AtomicU64,
) -> io::Result<(std::path::PathBuf, File)> {
    for _ in 0..TEMPORARY_ATTEMPTS {
        let mut pending = OsString::from(".");
        pending.push(name);
        pending.push(format!(
            ".{}-{}.pending",
            std::process::id(),
            counter.fetch_add(1, Ordering::Relaxed)
        ));
        let temporary = parent.join(pending);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate temporary sample database file",
    ))
}

/// Whether `path` is a regular file whose contents equal `bytes`. Reads at most
/// `bytes.len() + 1` bytes (itself bounded by `MAX_CATALOG_BYTES`); any error
/// or difference answers `false`, so the caller writes a fresh cache.
fn holds_exactly(path: &Path, bytes: &[u8]) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    if !file
        .metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() == bytes.len() as u64)
    {
        return false;
    }
    let mut existing = Vec::with_capacity(bytes.len());
    file.take(bytes.len() as u64 + 1)
        .read_to_end(&mut existing)
        .is_ok_and(|_| existing == bytes)
}

impl SampleCatalog {
    /// Move a refused cache aside to `<name>.corrupt` (replacing an older one)
    /// before it is rebuilt, so its bytes stay available for diagnosis. Returns
    /// the preserved path, or `None` when there was no file.
    pub fn preserve_corrupt(path: &Path) -> io::Result<Option<std::path::PathBuf>> {
        let name = path.file_name().ok_or_else(invalid)?;
        let mut preserved = name.to_owned();
        preserved.push(".corrupt");
        let preserved = path.with_file_name(preserved);
        match fs::rename(path, &preserved) {
            Ok(()) => Ok(Some(preserved)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn write_record(bytes: &mut Vec<u8>, record: Option<&SampleRecord>) {
    let Some(record) = record else {
        bytes.push(NO_RECORD);
        return;
    };
    bytes.push(if record.fingerprint.is_some() {
        RECORD_FINGERPRINTED
    } else {
        RECORD_UNREADABLE
    });
    bytes.extend_from_slice(&record.size.to_le_bytes());
    bytes.extend_from_slice(&record.modified_ns.to_le_bytes());
    if let Some(fingerprint) = record.fingerprint {
        bytes.extend_from_slice(&fingerprint.value().to_le_bytes());
    }
}

fn read_record(reader: &mut &[u8]) -> io::Result<Option<SampleRecord>> {
    let mut tag = [0];
    reader.read_exact(&mut tag)?;
    if tag[0] == NO_RECORD {
        return Ok(None);
    }
    if tag[0] != RECORD_UNREADABLE && tag[0] != RECORD_FINGERPRINTED {
        return Err(invalid());
    }
    let size = read_u64(reader)?;
    let modified_ns = read_u64(reader)?;
    let fingerprint = if tag[0] == RECORD_FINGERPRINTED {
        Some(Fingerprint::from_value(read_u64(reader)?))
    } else {
        None
    };
    Ok(Some(SampleRecord {
        size,
        modified_ns,
        fingerprint,
    }))
}

fn read_u64(reader: &mut &[u8]) -> io::Result<u64> {
    let mut bytes = [0; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_u32(reader: &mut &[u8]) -> io::Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn encode_path(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
    #[cfg(not(any(unix, windows)))]
    {
        path.to_string_lossy().as_bytes().to_vec()
    }
}

fn decode_path(bytes: &[u8]) -> io::Result<std::path::PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes.to_vec()).into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        if bytes.len() % 2 != 0 {
            return Err(invalid());
        }
        let units: Vec<_> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        Ok(OsString::from_wide(&units).into())
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(std::str::from_utf8(bytes).map_err(|_| invalid())?.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(label: &str) -> Self {
            static ID: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "hex-library-db-{label}-{}-{}",
                std::process::id(),
                ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct CwdGuard(PathBuf);
    impl CwdGuard {
        fn enter(dir: &Path) -> Self {
            let previous = std::env::current_dir().unwrap();
            std::env::set_current_dir(dir).unwrap();
            Self(previous)
        }
    }
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    #[test]
    fn save_normalizes_bare_relative_filename_parent() {
        assert_eq!(normalized_parent(Path::new("samples.db")), Path::new("."));
        assert_eq!(
            normalized_parent(Path::new("/tmp/hex/samples.db")),
            Path::new("/tmp/hex")
        );
        let dir = TempDir::new("bare-relative");
        let _cwd = CwdGuard::enter(&dir.0);
        let catalog = SampleCatalog::default();
        assert!(catalog.save(Path::new("samples.db")).unwrap());
        let loaded = SampleCatalog::load(Path::new("samples.db")).unwrap();
        assert_eq!(loaded.entries.len(), 0);
    }

    #[test]
    fn save_retries_past_preexisting_pending_file_collision() {
        let dir = TempDir::new("pending-collision");
        let db = dir.0.join("samples.db");
        let name = db.file_name().unwrap();
        let counter = AtomicU64::new(0);
        let colliding = dir
            .0
            .join(format!(".samples.db.{}-0.pending", std::process::id()));
        fs::write(&colliding, b"stale").unwrap();
        let (allocated, file) = create_temporary(&dir.0, name, &counter).unwrap();
        drop(file);
        assert_ne!(allocated, colliding);
        assert_eq!(fs::read(&colliding).unwrap(), b"stale");
        let _ = fs::remove_file(allocated);

        // Also exercise SampleCatalog::save directly when the next sequence slots collide.
        let base = NEXT.load(Ordering::Relaxed);
        let mut stale_files = Vec::new();
        for offset in 0..4 {
            let path = dir.0.join(format!(
                ".samples.db.{}-{}.pending",
                std::process::id(),
                base + offset
            ));
            fs::write(&path, b"preexisting").unwrap();
            stale_files.push(path);
        }
        let catalog = SampleCatalog::default();
        assert!(catalog.save(&db).unwrap());
        assert_eq!(SampleCatalog::load(&db).unwrap().entries.len(), 0);
        for path in &stale_files {
            assert_eq!(fs::read(path).unwrap(), b"preexisting");
        }
    }
}
