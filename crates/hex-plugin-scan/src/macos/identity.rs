//! Bounded filesystem snapshots for explicitly trusted factory discovery.
//! Hashes identify bytes, not authenticity or protection from malicious races.
use super::macho::{self, Image, Role};
use crate::{MAX_BINARY, hash};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_INFO_BYTES: u64 = 64 * 1024;

/// One regular-file snapshot. Its returned Vec has exactly the observed
/// payload capacity, no greater than `maximum`. Allocator overhead is opaque;
/// this is not an RSS bound or a complete native-construction resource quote.
/// Changed opened-file size, growth and truncation refuse rather than resizing.
pub fn read_file(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    read_file_between(path, maximum, || {})
}
fn read_file_between(
    path: &Path,
    maximum: u64,
    after_metadata: impl FnOnce(),
) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("file metadata failed: {e}"))?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err("nonregular or oversized discovery input".into());
    }
    after_metadata();
    let file = fs::File::open(path).map_err(|e| format!("discovery input open failed: {e}"))?;
    let opened = file
        .metadata()
        .map_err(|e| format!("opened discovery metadata failed: {e}"))?;
    if !opened.is_file() || opened.len() != metadata.len() {
        return Err("discovery input changed while opening".into());
    }
    read_snapshot(file, opened.len(), maximum)
}
fn read_snapshot(mut input: impl Read, length: u64, maximum: u64) -> Result<Vec<u8>, String> {
    if length > maximum {
        return Err("oversized discovery input".into());
    }
    let length = usize::try_from(length).map_err(|_| "discovery input length overflow")?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| "discovery input allocation refused")?;
    if bytes.capacity() != length {
        return Err("discovery input allocation bound differs".into());
    }
    bytes.resize(length, 0);
    input
        .read_exact(&mut bytes)
        .map_err(|e| format!("discovery input read failed: {e}"))?;
    // Probe without appending/reserving: EOF must not trigger read_to_end's
    // geometric spare-capacity growth, even when the snapshot fills its Vec.
    let mut extra = [0];
    match input.read_exact(&mut extra) {
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(bytes),
        Ok(()) => Err("discovery input grew during snapshot".into()),
        Err(error) => Err(format!("discovery input end read failed: {error}")),
    }
}

pub fn relative_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty() || relative.len() > MAX_PATH_BYTES || relative.contains('\0') {
        return Err("discovery relative path bound refused".into());
    }
    let components = Path::new(relative).components().collect::<Vec<_>>();
    if components.is_empty() || components.len() > 32 {
        return Err("discovery path depth refused".into());
    }
    let mut path = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err("discovery path must stay inside its selected bundle".into());
        };
        path.push(name);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|e| format!("bundle member metadata failed: {e}"))?;
        if metadata.file_type().is_symlink()
            || (index + 1 != components.len() && !metadata.is_dir())
            || (index + 1 == components.len() && !metadata.is_file())
        {
            return Err("linked or nonregular bundle member refused".into());
        }
    }
    Ok(path)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub supplied_path: PathBuf,
    pub canonical_path: PathBuf,
    pub info_sha256: String,
}

impl Bundle {
    pub fn inspect(path: &Path) -> Result<Self, String> {
        let original = path.to_str().ok_or("bundle path encoding unsupported")?;
        if original.len() > MAX_PATH_BYTES
            || path.extension().and_then(|v| v.to_str()) != Some("vst3")
        {
            return Err("one explicit .vst3 bundle path required".into());
        }
        let metadata = fs::symlink_metadata(path).map_err(|e| format!("bundle missing: {e}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("selected bundle must be a nonlinked directory".into());
        }
        let canonical_path = path
            .canonicalize()
            .map_err(|e| format!("bundle path failed: {e}"))?;
        if canonical_path
            .to_str()
            .is_none_or(|v| v.len() > MAX_PATH_BYTES)
        {
            return Err("canonical bundle path encoding or size refused".into());
        }
        let info = relative_file(&canonical_path, "Contents/Info.plist")?;
        Ok(Self {
            supplied_path: path.to_path_buf(),
            canonical_path,
            info_sha256: hash(&read_file(&info, MAX_INFO_BYTES)?),
        })
    }

    pub fn unchanged(&self) -> Result<(), String> {
        let current = Self::inspect(&self.supplied_path)?;
        if current.canonical_path != self.canonical_path || current.info_sha256 != self.info_sha256
        {
            return Err("selected bundle identity changed".into());
        }
        Ok(())
    }

    pub fn admit(&self, executable: &str) -> Result<Executable, String> {
        self.unchanged()?;
        if !executable.starts_with("Contents/MacOS/") {
            return Err("native bundle executable location refused".into());
        }
        let path = relative_file(&self.canonical_path, executable)?;
        let bytes = read_file(&path, MAX_BINARY)?;
        Ok(Executable {
            relative_path: executable.to_owned(),
            sha256: hash(&bytes),
            image: macho::inspect(&bytes, Role::Module)?,
        })
    }

    pub fn executable_unchanged(&self, expected: &Executable) -> Result<(), String> {
        let current = self.admit(&expected.relative_path)?;
        if &current != expected {
            return Err("selected executable changed".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Executable {
    pub relative_path: String,
    pub sha256: String,
    pub image: Image,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "hex-native-snapshot-{}-{unique}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn file_snapshot_capacity_is_the_observed_length_without_geometric_growth() {
        let directory = Directory::new();
        let path = directory.0.join("input");
        for length in [0, 1, 4097, 16_384] {
            let expected = vec![0x5a; length];
            fs::write(&path, &expected).unwrap();
            let snapshot = read_file(&path, MAX_BINARY).unwrap();
            assert_eq!(snapshot, expected);
            assert_eq!(
                snapshot.capacity(),
                length,
                "snapshot payload capacity must be explicit"
            );
        }
    }

    #[test]
    fn snapshot_reader_refuses_growth_truncation_limits_and_preserves_read_errors() {
        use std::io::{self, Cursor};
        for (bytes, length, maximum) in [(&b"abc"[..], 2, 3), (&b"ab"[..], 3, 3)] {
            assert!(read_snapshot(Cursor::new(bytes), length, maximum).is_err());
        }
        struct NeverRead;
        impl Read for NeverRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("invalid bound must refuse before reading");
            }
        }
        assert!(read_snapshot(NeverRead, 5, 4).is_err());
        assert!(read_snapshot(NeverRead, u64::MAX, u64::MAX).is_err());
        struct ProbeFailure(Cursor<&'static [u8]>);
        impl Read for ProbeFailure {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                if self.0.position() == 3 {
                    return Err(io::Error::other("original end read refusal"));
                }
                self.0.read(output)
            }
        }
        let error = read_snapshot(ProbeFailure(Cursor::new(b"abc")), 3, 3).unwrap_err();
        assert!(error.contains("original end read refusal"));
        struct Interrupted(bool, Cursor<&'static [u8]>);
        impl Read for Interrupted {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    return Err(io::ErrorKind::Interrupted.into());
                }
                self.1.read(output)
            }
        }
        assert_eq!(
            read_snapshot(Interrupted(false, Cursor::new(b"abc")), 3, 3).unwrap(),
            b"abc"
        );
        assert_eq!(
            read_snapshot(Cursor::new(&b""[..]), 0, 0)
                .unwrap()
                .capacity(),
            0
        );
    }

    #[test]
    fn file_snapshot_refuses_nonregular_oversized_and_changed_opened_inputs() {
        let directory = Directory::new();
        assert!(read_file(&directory.0, MAX_BINARY).is_err());
        let path = directory.0.join("input");
        fs::write(&path, b"ab").unwrap();
        let result = read_file_between(&path, MAX_BINARY, || {
            fs::write(&path, b"abc").unwrap();
        });
        assert!(result.unwrap_err().contains("changed while opening"));
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_BINARY).unwrap();
        let maximum = read_file(&path, MAX_BINARY).unwrap();
        assert_eq!(maximum.len(), MAX_BINARY as usize);
        assert_eq!(maximum.capacity(), MAX_BINARY as usize);
        drop(maximum);
        file.set_len(MAX_BINARY + 1).unwrap();
        assert!(read_file(&path, MAX_BINARY).is_err());
        #[cfg(unix)]
        {
            let linked = directory.0.join("linked");
            std::os::unix::fs::symlink(&path, &linked).unwrap();
            assert!(read_file(&linked, MAX_BINARY).is_err());
        }
    }

    #[test]
    fn paths_cannot_escape_the_selected_bundle() {
        for path in [
            "../outside",
            "/tmp/outside",
            "Contents/../../outside",
            "",
            "x\0y",
        ] {
            assert!(relative_file(Path::new("."), path).is_err());
        }
    }
}
