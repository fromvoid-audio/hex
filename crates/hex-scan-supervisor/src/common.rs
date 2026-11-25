//! Shared controlled-file validation, SHA-256 digests, and scanner record types.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const MAX_BINARY: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct License {
    pub sha256: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Module {
    pub name: String,
    pub binary: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog<R> {
    pub last_valid: Option<R>,
    pub last_error: Option<String>,
}

impl<R> Default for Catalog<R> {
    fn default() -> Self {
        Self {
            last_valid: None,
            last_error: None,
        }
    }
}

impl<R> Catalog<R> {
    pub fn record(&mut self, result: Result<R, String>) {
        match result {
            Ok(record) => {
                self.last_valid = Some(record);
                self.last_error = None;
            }
            Err(error) => self.last_error = Some(error),
        }
    }
}

// SHA output arrays no longer implement LowerHex; keep durable 64-byte text.
pub fn digest_hex(digest: [u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    output
}

pub fn hash(bytes: &[u8]) -> String {
    digest_hex(Sha256::digest(bytes).into())
}

pub fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub fn file_bytes(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    if !fs::symlink_metadata(path)
        .map_err(|_| "file metadata unavailable")?
        .file_type()
        .is_file()
    {
        return Err("non-regular or symlink file refused".into());
    }
    let mut file = fs::File::open(path).map_err(|_| "file unavailable".to_owned())?;
    if !file
        .metadata()
        .map_err(|_| "file metadata unavailable")?
        .is_file()
    {
        return Err("regular file required".into());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(limit.checked_add(1).ok_or("file bound overflow")?)
        .read_to_end(&mut bytes)
        .map_err(|_| "file read failed")?;
    if bytes.len() as u64 > limit {
        return Err("file bound exceeded".into());
    }
    Ok(bytes)
}

pub fn controlled_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("path containment refused".into());
    }
    let mut path = root.to_path_buf();
    if fs::symlink_metadata(root)
        .map_err(|_| "fixture root unavailable")?
        .file_type()
        .is_symlink()
    {
        return Err("symlink fixture root refused".into());
    }
    for component in relative.components() {
        path.push(component.as_os_str());
        if fs::symlink_metadata(&path)
            .map_err(|_| "controlled input missing")?
            .file_type()
            .is_symlink()
        {
            return Err("symlink controlled input refused".into());
        }
    }
    if !path
        .canonicalize()
        .map_err(|_| "controlled input unavailable")?
        .starts_with(
            root.canonicalize()
                .map_err(|_| "fixture root unavailable")?,
        )
    {
        return Err("path containment refused".into());
    }
    Ok(path)
}

/// Verify a controlled Linux x86_64 ELF64 little-endian `ET_DYN` fixture binary
/// against its expected SHA-256 digest.
///
/// This helper is intentionally scoped to Linux x86_64 ELF64 test/scan fixtures
/// and never loads native host binaries or parses macOS Mach-O executables.
pub fn linux_elf64_binary_identity(path: &Path, expected: &str) -> Result<String, String> {
    let bytes = file_bytes(path, MAX_BINARY)?;
    // Deliberately no native loader. Only the claimed Linux x86_64 ELF64 LE
    // fixture architecture/ET_DYN helper or shared-module ABI is accepted.
    if bytes.len() < 64
        || &bytes[..4] != b"\x7fELF"
        || bytes[4] != 2
        || bytes[5] != 1
        || bytes[6] != 1
        || !matches!(bytes[7], 0 | 3)
        || u32::from_le_bytes(bytes[20..24].try_into().map_err(|_| "ELF version width")?) != 1
        || u16::from_le_bytes([bytes[18], bytes[19]]) != 62
        || u16::from_le_bytes([bytes[16], bytes[17]]) != 3
    {
        return Err("unsupported binary architecture".into());
    }
    let digest = hash(&bytes);
    if digest != expected {
        return Err("controlled binary identity changed".into());
    }
    Ok(digest)
}

/// Compatibility alias for [`linux_elf64_binary_identity`] used by offline
/// protocol fixture harnesses that run on both Linux and macOS.
pub use linux_elf64_binary_identity as binary_identity;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "hex-scan-supervisor-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hash_text_retains_lowercase_padding_and_sha256_vectors() {
        for (input, expected) in [
            (
                b"".as_slice(),
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc".as_slice(),
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"\x03".as_slice(),
                "084fed08b978af4d7d196a7446a86b58009e636b611db16211b65a9aadff29c5",
            ),
        ] {
            assert_eq!(hash(input), expected);
            assert!(hex_digest(expected));
        }
        for byte in 0..=u8::MAX {
            assert_eq!(digest_hex([byte; 32]), format!("{byte:02x}").repeat(32));
        }
        assert!(!hex_digest(&"A".repeat(64)));
        assert!(!hex_digest(&"0".repeat(63)));
        assert!(!hex_digest(&format!("{}z", "0".repeat(63))));
    }

    #[test]
    fn controlled_path_file_bytes_and_binary_identity_enforce_bounds() {
        let dir = temp_dir("controlled");
        let file = dir.join("fixture.bin");
        let mut elf = vec![0_u8; 64];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        elf[6] = 1;
        elf[16..18].copy_from_slice(&3_u16.to_le_bytes());
        elf[18..20].copy_from_slice(&62_u16.to_le_bytes());
        elf[20..24].copy_from_slice(&1_u32.to_le_bytes());
        fs::write(&file, &elf).unwrap();

        assert_eq!(controlled_path(&dir, "fixture.bin").unwrap(), file);
        assert!(controlled_path(&dir, "../fixture.bin").is_err());
        assert_eq!(file_bytes(&file, 64).unwrap(), elf);
        assert!(file_bytes(&file, 63).is_err());
        assert!(binary_identity(&file, &hash(&elf)).is_ok());
        assert!(binary_identity(&file, &"0".repeat(64)).is_err());

        #[cfg(unix)]
        {
            let link = dir.join("link.bin");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert!(controlled_path(&dir, "link.bin").is_err());
            assert!(file_bytes(&link, 64).is_err());
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn catalog_record_preserves_last_valid_on_error() {
        let mut catalog: Catalog<u32> = Catalog::default();
        catalog.record(Ok(7));
        assert_eq!(catalog.last_valid, Some(7));
        assert_eq!(catalog.last_error, None);
        catalog.record(Err("failed".into()));
        assert_eq!(catalog.last_valid, Some(7));
        assert_eq!(catalog.last_error.as_deref(), Some("failed"));
        catalog.record(Ok(9));
        assert_eq!(catalog.last_valid, Some(9));
        assert_eq!(catalog.last_error, None);
    }
}
