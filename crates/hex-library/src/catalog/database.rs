//! Versioned, bounded, native-path derived cache, atomically replaced on success.
use super::{MAX_CATALOG_BYTES, MAX_PATH_BYTES, MAX_SAMPLES, SampleCatalog};
use crate::{EntryKind, LibraryEntry, is_wav};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(unix)]
const MAGIC: &[u8; 8] = b"hexsmp1u";
#[cfg(windows)]
const MAGIC: &[u8; 8] = b"hexsmp1w";
#[cfg(not(any(unix, windows)))]
const MAGIC: &[u8; 8] = b"hexsmp1t";
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
        let mut magic = [0; 8];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(invalid());
        }
        let count = read_u32(&mut reader)? as usize;
        if count > MAX_SAMPLES {
            return Err(invalid());
        }
        let mut entries = Vec::with_capacity(count);
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
            entries.push(LibraryEntry {
                name,
                path,
                kind: EntryKind::WaveAudio,
            });
        }
        if !reader.is_empty() {
            return Err(invalid());
        }
        Ok(Self { entries })
    }

    /// This is a disposable index, never a project or user preferences file.
    /// A failed write leaves the previous complete cache intact.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if self.entries.len() > MAX_SAMPLES {
            return Err(invalid());
        }
        let mut bytes = Vec::from(MAGIC.as_slice());
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
        }
        let parent = path.parent().ok_or_else(invalid)?;
        fs::create_dir_all(parent)?;
        let name = path.file_name().ok_or_else(invalid)?;
        let mut pending = OsString::from(".");
        pending.push(name);
        pending.push(format!(
            ".{}-{}.pending",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let temporary = parent.join(pending);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
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
        result
    }
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
