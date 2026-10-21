//! Bounded filesystem snapshots for explicitly trusted factory discovery.
//! Hashes identify bytes, not authenticity or protection from malicious races.
use super::macho::{self, Image, Role};
use crate::{MAX_BINARY, hash};
use serde::Serialize;
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_INFO_BYTES: u64 = 64 * 1024;

pub fn read_file(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("file metadata failed: {e}"))?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err("nonregular or oversized discovery input".into());
    }
    let file = fs::File::open(path).map_err(|e| format!("discovery input open failed: {e}"))?;
    let mut bytes = Vec::new();
    file.take(maximum.checked_add(1).ok_or("input limit overflow")?)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("discovery input read failed: {e}"))?;
    if bytes.len() as u64 > maximum {
        return Err("discovery input grew beyond its bound".into());
    }
    Ok(bytes)
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

#[derive(Clone, Debug, Serialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Executable {
    pub relative_path: String,
    pub sha256: String,
    pub image: Image,
}

#[cfg(test)]
mod tests {
    use super::*;

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
