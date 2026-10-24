//! Bounded filesystem snapshots for explicitly trusted factory discovery.
//! Hashes identify bytes, not authenticity or protection from malicious races.
use super::{
    macho::{self, Image, Role},
    validation::Error as ValidationError,
};
use crate::{MAX_BINARY, hash};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_INFO_BYTES: u64 = 64 * 1024;

/// Successful selected-module revalidation uses a returned canonical path
/// and one checked relative path, or just the executable path. File payload
/// scratch is separate and only one file is read at a time. OS/allocator
/// internals and error backing are not guessed as DATA bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceRevalidationStorageBound {
    pub data_bytes: usize,
    pub file_data_bytes: usize,
    pub file_reader_lifetime_bound: usize,
    pub canonicalization_lifetime_bound: usize,
}

/// One regular-file snapshot. Its returned Vec has exactly the observed
/// payload capacity, no greater than `maximum`. Allocator overhead is opaque;
/// this is not an RSS bound or a complete native-construction resource quote.
/// Changed opened-file size, growth and truncation refuse rather than resizing.
pub fn read_file(path: &Path, maximum: u64) -> Result<Vec<u8>, ValidationError> {
    read_file_between(path, maximum, || {})
}
fn read_file_between(
    path: &Path,
    maximum: u64,
    after_metadata: impl FnOnce(),
) -> Result<Vec<u8>, ValidationError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| ValidationError::Io {
        context: "file metadata failed",
        source,
    })?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err("nonregular or oversized discovery input".into());
    }
    after_metadata();
    let file = fs::File::open(path).map_err(|source| ValidationError::Io {
        context: "discovery input open failed",
        source,
    })?;
    let opened = file.metadata().map_err(|source| ValidationError::Io {
        context: "opened discovery metadata failed",
        source,
    })?;
    if !opened.is_file() || opened.len() != metadata.len() {
        return Err("discovery input changed while opening".into());
    }
    read_snapshot(file, opened.len(), maximum)
}
fn read_snapshot(
    mut input: impl Read,
    length: u64,
    maximum: u64,
) -> Result<Vec<u8>, ValidationError> {
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
        .map_err(|source| ValidationError::Io {
            context: "discovery input read failed",
            source,
        })?;
    // Probe without appending/reserving: EOF must not trigger read_to_end's
    // geometric spare-capacity growth, even when the snapshot fills its Vec.
    let mut extra = [0];
    match input.read_exact(&mut extra) {
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(bytes),
        Ok(()) => Err("discovery input grew during snapshot".into()),
        Err(error) => Err(ValidationError::Io {
            context: "discovery input end read failed",
            source: error,
        }),
    }
}

pub fn relative_file(root: &Path, relative: &str) -> Result<PathBuf, ValidationError> {
    if relative.is_empty() || relative.len() > MAX_PATH_BYTES || relative.contains('\0') {
        return Err("discovery relative path bound refused".into());
    }
    let components = Path::new(relative).components();
    let depth = components.clone().take(33).count();
    if depth == 0 || depth > 32 {
        return Err("discovery path depth refused".into());
    }
    if components
        .clone()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("discovery path must stay inside its selected bundle".into());
    }
    let capacity =
        relative_path_capacity(root, relative).ok_or("discovery path allocation bound overflow")?;
    let mut path = PathBuf::new();
    path.try_reserve_exact(capacity)
        .map_err(|_| "discovery path allocation refused")?;
    if path.capacity() != capacity {
        return Err("discovery path allocation bound differs".into());
    }
    path.push(root);
    for (index, component) in components.enumerate() {
        let Component::Normal(name) = component else {
            return Err("discovery path must stay inside its selected bundle".into());
        };
        path.push(name);
        if path.capacity() != capacity {
            return Err("discovery path exceeded its allocation bound".into());
        }
        let metadata = fs::symlink_metadata(&path).map_err(|source| ValidationError::Io {
            context: "bundle member metadata failed",
            source,
        })?;
        if metadata.file_type().is_symlink()
            || (index + 1 != depth && !metadata.is_dir())
            || (index + 1 == depth && !metadata.is_file())
        {
            return Err("linked or nonregular bundle member refused".into());
        }
    }
    Ok(path)
}

fn relative_path_capacity(root: &Path, relative: &str) -> Option<usize> {
    if root.as_os_str().len() > MAX_PATH_BYTES || relative.len() > MAX_PATH_BYTES {
        return None;
    }
    root.as_os_str()
        .len()
        .checked_add(1)?
        .checked_add(relative.len())
}

fn canonical_bundle_path(path: &Path) -> Result<PathBuf, ValidationError> {
    let original = path.to_str().ok_or("bundle path encoding unsupported")?;
    if original.len() > MAX_PATH_BYTES || path.extension().and_then(|v| v.to_str()) != Some("vst3")
    {
        return Err("one explicit .vst3 bundle path required".into());
    }
    let metadata = fs::symlink_metadata(path).map_err(|source| ValidationError::Io {
        context: "bundle missing",
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("selected bundle must be a nonlinked directory".into());
    }
    let canonical = path.canonicalize().map_err(|source| ValidationError::Io {
        context: "bundle path failed",
        source,
    })?;
    if canonical.to_str().is_none_or(|v| v.len() > MAX_PATH_BYTES)
        || canonical.capacity() > MAX_PATH_BYTES
    {
        return Err("canonical bundle path encoding or size refused".into());
    }
    Ok(canonical)
}

pub(super) fn hash_matches(bytes: &[u8], expected: &str) -> bool {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut actual = [0_u8; 64];
    for (index, byte) in Sha256::digest(bytes).iter().enumerate() {
        actual[index * 2] = HEX[usize::from(byte >> 4)];
        actual[index * 2 + 1] = HEX[usize::from(byte & 15)];
    }
    expected.as_bytes() == actual
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
        let canonical_path = canonical_bundle_path(path)?;
        let info = relative_file(&canonical_path, "Contents/Info.plist")?;
        Ok(Self {
            supplied_path: path.to_path_buf(),
            canonical_path,
            info_sha256: hash(&read_file(&info, MAX_INFO_BYTES)?),
        })
    }

    pub fn unchanged(&self) -> Result<(), ValidationError> {
        let current = canonical_bundle_path(&self.supplied_path)?;
        if current != self.canonical_path {
            return Err("selected bundle identity changed".into());
        }
        let info = relative_file(&current, "Contents/Info.plist")?;
        let bytes = read_file(&info, MAX_INFO_BYTES)?;
        if !hash_matches(&bytes, &self.info_sha256) {
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

    pub fn executable_unchanged(&self, expected: &Executable) -> Result<(), ValidationError> {
        self.unchanged()?;
        if !expected.relative_path.starts_with("Contents/MacOS/") {
            return Err("native bundle executable location refused".into());
        }
        let path = relative_file(&self.canonical_path, &expected.relative_path)?;
        let bytes = read_file(&path, MAX_BINARY)?;
        let image = macho::inspect(&bytes, Role::Module)?;
        if image != expected.image || !hash_matches(&bytes, &expected.sha256) {
            return Err("selected executable changed".into());
        }
        Ok(())
    }
    /// Bound the actual successful revalidator's path DATA before it runs.
    /// Incoming Bundle/Executable backing belongs to the caller; diagnostics,
    /// initial inspection and opaque OS/allocator internals remain separate.
    pub fn revalidation_storage_bound(
        &self,
        executable: &Executable,
    ) -> Option<SourceRevalidationStorageBound> {
        if self.supplied_path.as_os_str().len() > MAX_PATH_BYTES {
            return None;
        }
        let info = MAX_PATH_BYTES.checked_add(relative_path_capacity(
            &self.canonical_path,
            "Contents/Info.plist",
        )?)?;
        let module = relative_path_capacity(&self.canonical_path, &executable.relative_path)?;
        Some(SourceRevalidationStorageBound {
            data_bytes: info.max(module),
            file_data_bytes: usize::try_from(MAX_BINARY).ok()?,
            file_reader_lifetime_bound: 1,
            canonicalization_lifetime_bound: 1,
        })
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
        assert!(error.to_string().contains("original end read refusal"));
        assert!(matches!(
            error,
            ValidationError::Io {
                context: "discovery input end read failed",
                ..
            }
        ));
        assert!(std::error::Error::source(&error).is_some());
        let missing = read_file(&Directory::new().0.join("absent"), MAX_BINARY).unwrap_err();
        let ValidationError::Io { context, source } = missing else {
            panic!("original metadata I/O failure");
        };
        assert_eq!(context, "file metadata failed");
        assert_eq!(source.kind(), io::ErrorKind::NotFound);
        assert!(source.raw_os_error().is_some());
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
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("changed while opening")
        );
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
            file.set_len(1).unwrap(); // The target itself is valid; refusal must be for the link.
            assert!(read_file(&path, MAX_BINARY).is_ok());
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

    #[test]
    fn source_revalidation_preserves_identity_errors_and_declared_path_capacity() {
        let directory = Directory::new();
        let root = directory.0.join("original-é.vst3");
        fs::create_dir_all(root.join("Contents/MacOS")).unwrap();
        let info = root.join("Contents/Info.plist");
        fs::write(&info, b"original info").unwrap();
        // Structural Mach-O fixture only; no executable/SDK code is run.
        let image: Vec<u8> = [0xfeed_facf_u32, 0x0100_000c, 0, 8, 1, 8, 0, 0, 0, 8]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        let executable = root.join("Contents/MacOS/fixture");
        fs::write(&executable, &image).unwrap();
        let bundle = Bundle::inspect(&root).unwrap();
        let selected = bundle.admit("Contents/MacOS/fixture").unwrap();
        let bound = bundle.revalidation_storage_bound(&selected).unwrap();
        assert!(bound.data_bytes >= MAX_PATH_BYTES);
        assert_eq!(bound.file_data_bytes, MAX_BINARY as usize);
        bundle.executable_unchanged(&selected).unwrap();
        let path = relative_file(&bundle.canonical_path, &selected.relative_path).unwrap();
        assert_eq!(
            path.capacity(),
            bundle.canonical_path.as_os_str().len() + 1 + selected.relative_path.len()
        );
        fs::write(&info, b"changed info").unwrap();
        assert!(
            bundle
                .executable_unchanged(&selected)
                .unwrap_err()
                .to_string()
                .contains("bundle identity changed")
        );
        fs::write(&info, b"original info").unwrap();
        let mut changed = image.clone();
        changed.push(0);
        fs::write(&executable, changed).unwrap();
        assert!(
            bundle
                .executable_unchanged(&selected)
                .unwrap_err()
                .to_string()
                .contains("executable changed")
        );
        fs::write(&executable, b"not mach-o").unwrap();
        assert!(
            bundle
                .executable_unchanged(&selected)
                .unwrap_err()
                .to_string()
                .contains("unsupported mach-o container")
        );
    }

    #[test]
    fn relative_path_depth_boundary_and_links_keep_their_refusal() {
        let directory = Directory::new();
        let mut full = directory.0.clone();
        let mut relative = String::new();
        for _ in 0..31 {
            full.push("d");
            fs::create_dir(&full).unwrap();
            relative.push_str("d/");
        }
        full.push("leaf");
        fs::write(&full, b"original").unwrap();
        relative.push_str("leaf");
        assert_eq!(relative_file(&directory.0, &relative).unwrap(), full);
        assert!(
            relative_file(&directory.0, &format!("d/{relative}"))
                .unwrap_err()
                .to_string()
                .contains("depth refused")
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&full, directory.0.join("linked")).unwrap();
            assert!(
                relative_file(&directory.0, "linked")
                    .unwrap_err()
                    .to_string()
                    .contains("linked or nonregular")
            );
        }
    }
}
