//! Versioned, bounded, native-path derived cache, atomically replaced on success.
//!
//! Version 3 (`hexsmp3`) adds lightweight [`AudioHeaderMetadata`] inside each
//! [`SampleRecord`] plus persistent user tags and named collections. Version 1
//! (`hexsmp1`) and version 2 (`hexsmp2`) caches migrate cleanly on load and are
//! rewritten as version 3 on save. Newer or malformed formats are strictly
//! refused without clobbering disk bytes; [`SampleCatalog::preserve_corrupt`]
//! moves refused bytes aside for diagnosis.
use super::{
    AudioCodec, AudioHeaderMetadata, Fingerprint, MAX_CATALOG_BYTES, MAX_COLLECTIONS,
    MAX_PATH_BYTES, MAX_SAMPLES, MAX_TAG_BYTES, MAX_TAGS_PER_SAMPLE, SampleCatalog, SampleRecord,
    normalize_label,
};
use crate::{EntryKind, LibraryEntry, is_wav};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
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
const MAGIC_V3: [u8; 8] = *b"hexsmp3\0";
const NO_RECORD: u8 = 0;
const RECORD_UNREADABLE: u8 = 1;
const RECORD_FINGERPRINTED: u8 = 2;
const RECORD_FINGERPRINTED_WITH_AUDIO: u8 = 3;
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
        let version = if found == magic(MAGIC_V3) {
            3
        } else if found == magic(MAGIC_V2) {
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
        let mut records = HashMap::new();
        let mut seen = BTreeSet::new();
        for _ in 0..count {
            let path = read_path(&mut reader)?;
            if !path.is_absolute() || !is_wav(path.extension()) || !seen.insert(path.clone()) {
                return Err(invalid());
            }
            let name = path
                .file_name()
                .ok_or_else(invalid)?
                .to_string_lossy()
                .into_owned();
            if version >= 2
                && let Some(record) = read_record(&mut reader, version)?
            {
                records.insert(path.clone(), record);
            }
            entries.push(LibraryEntry {
                name,
                path,
                kind: EntryKind::WaveAudio,
            });
        }
        let mut tags: HashMap<PathBuf, BTreeSet<String>> = HashMap::new();
        let mut collections: BTreeMap<String, BTreeSet<PathBuf>> = BTreeMap::new();
        if version >= 3 {
            let tag_paths = read_u32(&mut reader)? as usize;
            if tag_paths > MAX_SAMPLES {
                return Err(invalid());
            }
            for _ in 0..tag_paths {
                let path = read_path(&mut reader)?;
                if !path.is_absolute() || tags.contains_key(&path) {
                    return Err(invalid());
                }
                let tag_count = usize::from(read_u16(&mut reader)?);
                if tag_count == 0 || tag_count > MAX_TAGS_PER_SAMPLE {
                    return Err(invalid());
                }
                let mut set = BTreeSet::new();
                for _ in 0..tag_count {
                    let label = read_label(&mut reader)?;
                    if !set.insert(label) {
                        return Err(invalid());
                    }
                }
                tags.insert(path, set);
            }
            let collection_count = read_u32(&mut reader)? as usize;
            if collection_count > MAX_COLLECTIONS {
                return Err(invalid());
            }
            for _ in 0..collection_count {
                let name = read_label(&mut reader)?;
                if collections.contains_key(&name) {
                    return Err(invalid());
                }
                let member_count = read_u32(&mut reader)? as usize;
                if member_count > MAX_SAMPLES {
                    return Err(invalid());
                }
                let mut members = BTreeSet::new();
                for _ in 0..member_count {
                    let path = read_path(&mut reader)?;
                    if !path.is_absolute() || !members.insert(path) {
                        return Err(invalid());
                    }
                }
                collections.insert(name, members);
            }
        }
        if !reader.is_empty() {
            return Err(invalid());
        }
        Ok(Self {
            entries,
            records,
            tags,
            collections,
        })
    }

    /// This is a disposable index, never a project or user preferences file.
    /// A failed write leaves the previous complete cache intact.
    ///
    /// Returns `Ok(false)` without writing, syncing or touching the file's
    /// modification time when `path` already holds exactly these encoded bytes,
    /// so periodic rescans of an unchanged library cost no durable writes.
    pub fn save(&self, path: &Path) -> io::Result<bool> {
        if self.entries.len() > MAX_SAMPLES
            || self.tags.len() > MAX_SAMPLES
            || self.collections.len() > MAX_COLLECTIONS
        {
            return Err(invalid());
        }
        let mut bytes = Vec::from(magic(MAGIC_V3).as_slice());
        bytes.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            write_path(&mut bytes, &entry.path)?;
            write_record(&mut bytes, self.records.get(&entry.path));
            if bytes.len() > MAX_CATALOG_BYTES {
                return Err(invalid());
            }
        }
        // Write user tags in deterministic path order so identical state produces identical bytes.
        let mut sorted_tags: Vec<(Vec<u8>, &BTreeSet<String>)> = self
            .tags
            .iter()
            .filter(|(_, set)| !set.is_empty())
            .map(|(path, set)| (encode_path(path), set))
            .collect();
        sorted_tags.sort_by(|a, b| a.0.cmp(&b.0));
        bytes.extend_from_slice(&(sorted_tags.len() as u32).to_le_bytes());
        for (raw_path, set) in sorted_tags {
            if raw_path.is_empty()
                || raw_path.len() > MAX_PATH_BYTES
                || set.len() > MAX_TAGS_PER_SAMPLE
            {
                return Err(invalid());
            }
            bytes.extend_from_slice(&(raw_path.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&raw_path);
            bytes.extend_from_slice(&(set.len() as u16).to_le_bytes());
            for tag in set {
                write_label(&mut bytes, tag)?;
            }
            if bytes.len() > MAX_CATALOG_BYTES {
                return Err(invalid());
            }
        }
        // Write user collections (BTreeMap + BTreeSet are already deterministically sorted).
        bytes.extend_from_slice(&(self.collections.len() as u32).to_le_bytes());
        for (name, members) in &self.collections {
            if members.len() > MAX_SAMPLES {
                return Err(invalid());
            }
            write_label(&mut bytes, name)?;
            bytes.extend_from_slice(&(members.len() as u32).to_le_bytes());
            for member in members {
                write_path(&mut bytes, member)?;
            }
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
    let tag = match (record.fingerprint, record.audio) {
        (Some(_), Some(_)) => RECORD_FINGERPRINTED_WITH_AUDIO,
        (Some(_), None) => RECORD_FINGERPRINTED,
        (None, _) => RECORD_UNREADABLE,
    };
    bytes.push(tag);
    bytes.extend_from_slice(&record.size.to_le_bytes());
    bytes.extend_from_slice(&record.modified_ns.to_le_bytes());
    if let Some(fingerprint) = record.fingerprint {
        bytes.extend_from_slice(&fingerprint.value().to_le_bytes());
        if let Some(audio) = record.audio {
            bytes.extend_from_slice(&audio.sample_rate.to_le_bytes());
            bytes.extend_from_slice(&audio.channels.to_le_bytes());
            bytes.extend_from_slice(&audio.bit_depth.to_le_bytes());
            bytes.extend_from_slice(&audio.frame_count.to_le_bytes());
            bytes.push(audio.codec as u8);
        }
    }
}

fn read_record(reader: &mut &[u8], version: u8) -> io::Result<Option<SampleRecord>> {
    let mut tag = [0];
    reader.read_exact(&mut tag)?;
    if tag[0] == NO_RECORD {
        return Ok(None);
    }
    let valid_tag = match tag[0] {
        RECORD_UNREADABLE | RECORD_FINGERPRINTED => true,
        RECORD_FINGERPRINTED_WITH_AUDIO => version >= 3,
        _ => false,
    };
    if !valid_tag {
        return Err(invalid());
    }
    let size = read_u64(reader)?;
    let modified_ns = read_u64(reader)?;
    let fingerprint = if tag[0] == RECORD_FINGERPRINTED || tag[0] == RECORD_FINGERPRINTED_WITH_AUDIO
    {
        Some(Fingerprint::from_value(read_u64(reader)?))
    } else {
        None
    };
    let audio = if tag[0] == RECORD_FINGERPRINTED_WITH_AUDIO {
        let sample_rate = read_u32(reader)?;
        let channels = read_u16(reader)?;
        let bit_depth = read_u16(reader)?;
        let frame_count = read_u64(reader)?;
        let mut codec_byte = [0];
        reader.read_exact(&mut codec_byte)?;
        let codec = AudioCodec::from_u8(codec_byte[0]).ok_or_else(invalid)?;
        if sample_rate == 0 || channels == 0 || bit_depth == 0 {
            return Err(invalid());
        }
        Some(AudioHeaderMetadata {
            sample_rate,
            channels,
            bit_depth,
            frame_count,
            codec,
        })
    } else {
        None
    };
    Ok(Some(SampleRecord {
        size,
        modified_ns,
        fingerprint,
        audio,
    }))
}

fn write_path(bytes: &mut Vec<u8>, path: &Path) -> io::Result<()> {
    let raw = encode_path(path);
    if raw.is_empty()
        || raw.len() > MAX_PATH_BYTES
        || bytes.len().saturating_add(raw.len()).saturating_add(4) > MAX_CATALOG_BYTES
    {
        return Err(invalid());
    }
    bytes.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&raw);
    Ok(())
}

fn read_path(reader: &mut &[u8]) -> io::Result<PathBuf> {
    let length = read_u32(reader)? as usize;
    if length == 0 || length > MAX_PATH_BYTES || length > reader.len() {
        return Err(invalid());
    }
    let (raw, remaining) = reader.split_at(length);
    *reader = remaining;
    decode_path(raw)
}

fn write_label(bytes: &mut Vec<u8>, label: &str) -> io::Result<()> {
    let raw = label.as_bytes();
    if raw.is_empty()
        || raw.len() > MAX_TAG_BYTES
        || bytes.len().saturating_add(raw.len()).saturating_add(2) > MAX_CATALOG_BYTES
    {
        return Err(invalid());
    }
    bytes.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    bytes.extend_from_slice(raw);
    Ok(())
}

fn read_label(reader: &mut &[u8]) -> io::Result<String> {
    let length = usize::from(read_u16(reader)?);
    if length == 0 || length > MAX_TAG_BYTES || length > reader.len() {
        return Err(invalid());
    }
    let (raw, remaining) = reader.split_at(length);
    *reader = remaining;
    let text = std::str::from_utf8(raw).map_err(|_| invalid())?;
    let normalized = normalize_label(text).map_err(|_| invalid())?;
    if normalized != text {
        return Err(invalid());
    }
    Ok(normalized)
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

fn read_u16(reader: &mut &[u8]) -> io::Result<u16> {
    let mut bytes = [0; 2];
    reader.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
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
        if !bytes.len().is_multiple_of(2) {
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
