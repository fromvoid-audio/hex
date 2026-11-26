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
    io::{self, BufWriter, Read, Write},
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
const MAGIC_V4: [u8; 8] = *b"hexsmp4\0";
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
        let version = if found == magic(MAGIC_V4) {
            4
        } else if found == magic(MAGIC_V3) {
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
        let mut tag_taxonomy: BTreeMap<String, Option<String>> = BTreeMap::new();
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
        if version >= 4 {
            let taxonomy_count = read_u32(&mut reader)? as usize;
            if taxonomy_count > crate::boundary::MAX_TAXONOMY_TAGS {
                return Err(invalid());
            }
            for _ in 0..taxonomy_count {
                let name = read_label(&mut reader)?;
                if tag_taxonomy.contains_key(&name) {
                    return Err(invalid());
                }
                let mut has_parent = [0u8; 1];
                reader.read_exact(&mut has_parent)?;
                let parent = match has_parent[0] {
                    0 => None,
                    1 => Some(read_label(&mut reader)?),
                    _ => return Err(invalid()),
                };
                tag_taxonomy.insert(name, parent);
            }
            validate_taxonomy_map(&tag_taxonomy).map_err(|_| invalid())?;
        }
        if !reader.is_empty() {
            return Err(invalid());
        }
        let mut catalog = Self {
            entries,
            folded_paths: Vec::new(),
            records,
            tags,
            tag_taxonomy,
            collections,
        };
        catalog.rebuild_search_index();
        Ok(catalog)
    }

    /// This is a disposable index, never a project or user preferences file.
    /// A failed write leaves the previous complete cache intact.
    ///
    /// Returns `Ok(false)` without writing, syncing or touching the file's
    /// modification time when `path` already holds exactly these encoded bytes,
    /// so periodic rescans of an unchanged library cost no durable writes.
    /// Comparison streams against a fixed-size stack buffer without allocating
    /// full-catalog encode/read buffers in memory.
    pub fn save(&self, path: &Path) -> io::Result<bool> {
        if self.entries.len() > MAX_SAMPLES
            || self.tags.len() > MAX_SAMPLES
            || self.collections.len() > MAX_COLLECTIONS
        {
            return Err(invalid());
        }
        // Write user tags in deterministic path order so identical state produces identical bytes.
        let mut sorted_tags: Vec<(Vec<u8>, &Path, &BTreeSet<String>)> = self
            .tags
            .iter()
            .filter(|(_, set)| !set.is_empty())
            .map(|(path, set)| (encode_path(path), path.as_path(), set))
            .collect();
        sorted_tags.sort_by(|a, b| a.0.cmp(&b.0));

        if let Ok(file) = File::open(path)
            && let Ok(metadata) = file.metadata()
            && metadata.is_file()
            && metadata.len() <= MAX_CATALOG_BYTES as u64
        {
            let mut compare = CompareSink::new(file, metadata.len());
            self.encode_catalog(&sorted_tags, &mut compare)?;
            if compare.is_exact_match() {
                return Ok(false);
            }
        } else {
            let mut validator = CountingSink::default();
            self.encode_catalog(&sorted_tags, &mut validator)?;
        }

        let name = path.file_name().ok_or_else(invalid)?;
        let parent = normalized_parent(path);
        fs::create_dir_all(parent)?;
        let (temporary, file) = create_temporary(parent, name, &NEXT)?;
        let result = (|| {
            let mut sink = BufferedSink::new(file);
            self.encode_catalog(&sorted_tags, &mut sink)?;
            let file = sink.into_inner()?;
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

    fn encode_catalog<S: CatalogSink>(
        &self,
        sorted_tags: &[(Vec<u8>, &Path, &BTreeSet<String>)],
        sink: &mut S,
    ) -> io::Result<()> {
        let has_taxonomy = !self.tag_taxonomy.is_empty();
        if has_taxonomy {
            validate_taxonomy_map(&self.tag_taxonomy).map_err(|_| invalid())?;
            sink.write_bytes(&magic(MAGIC_V4))?;
        } else {
            sink.write_bytes(&magic(MAGIC_V3))?;
        }
        sink.write_bytes(&(self.entries.len() as u32).to_le_bytes())?;
        for entry in &self.entries {
            write_path(sink, &entry.path)?;
            write_record(sink, self.records.get(&entry.path))?;
        }
        sink.write_bytes(&(sorted_tags.len() as u32).to_le_bytes())?;
        for (raw_path, path, set) in sorted_tags {
            if !path.is_absolute()
                || raw_path.is_empty()
                || raw_path.len() > MAX_PATH_BYTES
                || set.len() > MAX_TAGS_PER_SAMPLE
            {
                return Err(invalid());
            }
            sink.write_bytes(&(raw_path.len() as u32).to_le_bytes())?;
            sink.write_bytes(raw_path)?;
            sink.write_bytes(&(set.len() as u16).to_le_bytes())?;
            for tag in *set {
                write_label(sink, tag)?;
            }
        }
        // Write user collections (BTreeMap + BTreeSet are already deterministically sorted).
        sink.write_bytes(&(self.collections.len() as u32).to_le_bytes())?;
        for (name, members) in &self.collections {
            if members.len() > MAX_SAMPLES {
                return Err(invalid());
            }
            write_label(sink, name)?;
            sink.write_bytes(&(members.len() as u32).to_le_bytes())?;
            for member in members {
                write_path(sink, member)?;
            }
        }
        if has_taxonomy {
            sink.write_bytes(&(self.tag_taxonomy.len() as u32).to_le_bytes())?;
            for (name, parent) in &self.tag_taxonomy {
                write_label(sink, name)?;
                match parent {
                    None => sink.write_bytes(&[0])?,
                    Some(parent_name) => {
                        sink.write_bytes(&[1])?;
                        write_label(sink, parent_name)?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn validate_taxonomy_map(map: &BTreeMap<String, Option<String>>) -> Result<(), &'static str> {
    if map.len() > crate::boundary::MAX_TAXONOMY_TAGS {
        return Err("taxonomy tag limit reached");
    }
    for (name, parent) in map {
        let mut cursor = parent.as_deref();
        let mut depth = 1_usize;
        let mut seen = BTreeSet::from([name.as_str()]);
        while let Some(node) = cursor {
            if !map.contains_key(node) {
                return Err("parent tag does not exist");
            }
            if !seen.insert(node) {
                return Err("tag hierarchy cannot contain a cycle");
            }
            if depth > crate::boundary::MAX_TAXONOMY_DEPTH {
                return Err("tag hierarchy depth limit reached");
            }
            cursor = map.get(node).and_then(|p| p.as_deref());
            depth += 1;
        }
    }
    Ok(())
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

const COMPARE_CHUNK_BYTES: usize = 16 * 1024;
const WRITE_BUFFER_BYTES: usize = 64 * 1024;

trait CatalogSink {
    fn write_bytes(&mut self, data: &[u8]) -> io::Result<()>;
}

#[derive(Default)]
struct CountingSink {
    written: usize,
}

impl CatalogSink for CountingSink {
    fn write_bytes(&mut self, data: &[u8]) -> io::Result<()> {
        self.written = self.written.saturating_add(data.len());
        if self.written > MAX_CATALOG_BYTES {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Streams encoded catalog bytes directly against an existing cache file using
/// a fixed stack buffer so unchanged rescans allocate zero full-catalog buffers.
struct CompareSink {
    file: File,
    buf: [u8; COMPARE_CHUNK_BYTES],
    pos: usize,
    filled: usize,
    written: usize,
    expected_len: u64,
    matched: bool,
}

impl CompareSink {
    fn new(file: File, expected_len: u64) -> Self {
        Self {
            file,
            buf: [0; COMPARE_CHUNK_BYTES],
            pos: 0,
            filled: 0,
            written: 0,
            expected_len,
            matched: true,
        }
    }

    fn is_exact_match(&mut self) -> bool {
        if !self.matched || self.written as u64 != self.expected_len || self.pos != self.filled {
            return false;
        }
        let mut trailing = [0_u8; 1];
        loop {
            match self.file.read(&mut trailing) {
                Ok(0) => return true,
                Ok(_) => return false,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
        }
    }
}

impl CatalogSink for CompareSink {
    fn write_bytes(&mut self, mut data: &[u8]) -> io::Result<()> {
        self.written = self.written.saturating_add(data.len());
        if self.written > MAX_CATALOG_BYTES {
            return Err(invalid());
        }
        if !self.matched {
            return Ok(());
        }
        if self.written as u64 > self.expected_len {
            self.matched = false;
            return Ok(());
        }
        while !data.is_empty() {
            if self.pos == self.filled {
                let read = loop {
                    match self.file.read(&mut self.buf) {
                        Ok(n) => break n,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => {
                            self.matched = false;
                            return Ok(());
                        }
                    }
                };
                if read == 0 {
                    self.matched = false;
                    return Ok(());
                }
                self.pos = 0;
                self.filled = read;
            }
            let avail = (self.filled - self.pos).min(data.len());
            if self.buf[self.pos..self.pos + avail] != data[..avail] {
                self.matched = false;
                return Ok(());
            }
            self.pos += avail;
            data = &data[avail..];
        }
        Ok(())
    }
}

struct BufferedSink {
    writer: BufWriter<File>,
    written: usize,
}

impl BufferedSink {
    fn new(file: File) -> Self {
        Self {
            writer: BufWriter::with_capacity(WRITE_BUFFER_BYTES, file),
            written: 0,
        }
    }

    fn into_inner(self) -> io::Result<File> {
        self.writer
            .into_inner()
            .map_err(io::IntoInnerError::into_error)
    }
}

impl CatalogSink for BufferedSink {
    fn write_bytes(&mut self, data: &[u8]) -> io::Result<()> {
        self.written = self.written.saturating_add(data.len());
        if self.written > MAX_CATALOG_BYTES {
            return Err(invalid());
        }
        self.writer.write_all(data)
    }
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

fn write_record<S: CatalogSink>(sink: &mut S, record: Option<&SampleRecord>) -> io::Result<()> {
    let Some(record) = record else {
        return sink.write_bytes(&[NO_RECORD]);
    };
    let tag = match (record.fingerprint, record.audio) {
        (Some(_), Some(_)) => RECORD_FINGERPRINTED_WITH_AUDIO,
        (Some(_), None) => RECORD_FINGERPRINTED,
        (None, _) => RECORD_UNREADABLE,
    };
    let mut buf = [0_u8; 42];
    buf[0] = tag;
    buf[1..9].copy_from_slice(&record.size.to_le_bytes());
    buf[9..17].copy_from_slice(&record.modified_ns.to_le_bytes());
    let mut len = 17;
    if let Some(fingerprint) = record.fingerprint {
        buf[17..25].copy_from_slice(&fingerprint.value().to_le_bytes());
        len = 25;
        if let Some(audio) = record.audio {
            buf[25..29].copy_from_slice(&audio.sample_rate.to_le_bytes());
            buf[29..31].copy_from_slice(&audio.channels.to_le_bytes());
            buf[31..33].copy_from_slice(&audio.bit_depth.to_le_bytes());
            buf[33..41].copy_from_slice(&audio.frame_count.to_le_bytes());
            buf[41] = audio.codec as u8;
            len = 42;
        }
    }
    sink.write_bytes(&buf[..len])
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
    let audio_inspected = version >= 3 && tag[0] != RECORD_UNREADABLE;
    Ok(Some(SampleRecord {
        size,
        modified_ns,
        fingerprint,
        audio,
        audio_inspected,
    }))
}

fn write_path<S: CatalogSink>(sink: &mut S, path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(invalid());
    }
    let raw = encode_path(path);
    if raw.is_empty() || raw.len() > MAX_PATH_BYTES {
        return Err(invalid());
    }
    sink.write_bytes(&(raw.len() as u32).to_le_bytes())?;
    sink.write_bytes(&raw)
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

fn write_label<S: CatalogSink>(sink: &mut S, label: &str) -> io::Result<()> {
    let raw = label.as_bytes();
    if raw.is_empty() || raw.len() > MAX_TAG_BYTES {
        return Err(invalid());
    }
    sink.write_bytes(&(raw.len() as u16).to_le_bytes())?;
    sink.write_bytes(raw)
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
