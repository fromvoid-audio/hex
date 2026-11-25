//! Bounded explicit user-folder CLAP bundle discovery and static inspection.
//!
//! Graduates `hex-clap-scan` beyond controlled `HCLSCAN1` fixtures to discover
//! user-installed `.clap` bundles under explicit macOS roots
//! (`~/Library/Audio/Plug-Ins/CLAP`, `/Library/Audio/Plug-Ins/CLAP`, or an
//! explicit custom folder) without executing foreign code.
//!
//! Traversal never follows symlinks, never descends into `.clap` bundles, and
//! enforces strict root, path, depth, directory, entry, and bundle bounds.
//! Static Mach-O header and SHA-256 checks validate bundle integrity and detect
//! later tampering; this does not load foreign code or establish runtime audio
//! admission.

use crate::{MAX_BINARY, file_bytes, hash, macho_identity};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub const MAX_USER_ROOTS: usize = 3;
pub const MAX_USER_ENTRIES: usize = 4096;
pub const MAX_USER_DIRECTORIES: usize = 1024;
pub const MAX_USER_DEPTH: usize = 8;
pub const MAX_USER_BUNDLES: usize = 512;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_PATH_COMPONENTS: usize = 64;
pub const MAX_INFO_PLIST_BYTES: u64 = 512 * 1024;

/// Bounded enumeration of `.clap` bundle paths within explicit folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderEnumeration {
    pub bundles: Vec<PathBuf>,
    pub skipped_links: usize,
}

/// Static identity captured for an explicitly discovered macOS `.clap` bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClapBundleIdentity {
    pub supplied_path: PathBuf,
    pub canonical_path: PathBuf,
    pub bundle_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info_plist_sha256: Option<String>,
    pub executable_relative_path: String,
    pub executable_sha256: String,
    pub thin_arm64_module: bool,
}

/// Outcome of scanning one bounded batch of explicit CLAP plugin folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClapFolderScanReport {
    pub results: Vec<(PathBuf, Result<ClapBundleIdentity, String>)>,
    pub skipped_links: usize,
}

/// Validate a bounded, absolute, NUL-free, `..`-free filesystem path.
pub fn path_valid(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|s| s.len() <= MAX_PATH_BYTES && !s.contains('\0'))
        && path.components().count() <= MAX_PATH_COMPONENTS
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

/// Standard macOS local/user CLAP locations. No home fallback.
pub fn macos_standard_clap_roots() -> Result<Vec<PathBuf>, String> {
    macos_clap_scan_paths(true, true, None)
}

/// Resolve selected macOS CLAP folders (`/Library/Audio/Plug-Ins/CLAP`,
/// `~/Library/Audio/Plug-Ins/CLAP`, and an optional custom directory).
pub fn macos_clap_scan_paths(
    system: bool,
    user: bool,
    custom: Option<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    if system {
        roots.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
    }
    if user {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("user home unavailable")?;
        if !path_valid(&home) {
            return Err("user home path invalid".into());
        }
        roots.push(home.join("Library/Audio/Plug-Ins/CLAP"));
    }
    roots.extend(custom);
    Ok(roots)
}

fn existing_plain_dir(path: &Path) -> Result<bool, String> {
    if !path_valid(path) {
        return Err("invalid clap folder path".into());
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&prefix) {
            Ok(info) if info.is_dir() && !info.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(format!(
                    "clap folder has linked or non-directory component: {}",
                    prefix.display()
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(format!("clap folder unavailable: {e}")),
        }
    }
    Ok(true)
}

fn plain_file_path(path: &Path) -> Result<(), String> {
    if !path_valid(path) {
        return Err("invalid clap member path".into());
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let info =
            fs::symlink_metadata(&prefix).map_err(|e| format!("clap member unavailable: {e}"))?;
        if info.file_type().is_symlink() {
            return Err("linked clap bundle component refused".into());
        }
    }
    let meta = fs::symlink_metadata(path).map_err(|e| format!("clap file unavailable: {e}"))?;
    if !meta.is_file() {
        return Err("clap executable is not a regular file".into());
    }
    Ok(())
}

/// Enumerate `.clap` bundles under one to three explicit roots.
///
/// Missing roots yield an empty result; symlinks inside an existing root are
/// counted and skipped; linked root components and bound violations fail fast.
pub fn enumerate_clap_folders(
    roots: &[PathBuf],
    mut cancelled: impl FnMut() -> bool,
) -> Result<FolderEnumeration, String> {
    if roots.is_empty() || roots.len() > MAX_USER_ROOTS {
        return Err("one to three clap folders required".into());
    }
    let mut pending = Vec::new();
    for root in roots {
        if cancelled() {
            return Err("clap folder scan cancelled".into());
        }
        if root.extension().and_then(|s| s.to_str()) == Some("clap") {
            return Err("select folders containing clap bundles".into());
        }
        if existing_plain_dir(root)? && !pending.iter().any(|(path, _)| path == root) {
            pending.push((root.clone(), 0));
        }
    }
    let mut result = FolderEnumeration {
        bundles: Vec::new(),
        skipped_links: 0,
    };
    let mut entries = 0_usize;
    let mut directories = 0_usize;
    while let Some((directory, depth)) = pending.pop() {
        if cancelled() {
            return Err("clap folder scan cancelled".into());
        }
        if !existing_plain_dir(&directory)? {
            return Err("clap folder disappeared during enumeration".into());
        }
        directories += 1;
        if directories > MAX_USER_DIRECTORIES {
            return Err("clap directory bound exceeded".into());
        }
        let mut children = Vec::new();
        for entry in fs::read_dir(&directory)
            .map_err(|e| format!("clap folder listing failed at {}: {e}", directory.display()))?
        {
            if cancelled() {
                return Err("clap folder scan cancelled".into());
            }
            entries += 1;
            if entries > MAX_USER_ENTRIES {
                return Err("clap folder entry bound exceeded".into());
            }
            let entry = entry.map_err(|e| format!("clap folder entry failed: {e}"))?;
            let path = entry.path();
            if !path_valid(&path) {
                return Err("clap folder member path invalid or oversized".into());
            }
            let kind = entry
                .file_type()
                .map_err(|e| format!("clap folder member unavailable: {e}"))?;
            if kind.is_symlink() {
                result.skipped_links += 1;
                continue;
            }
            if !kind.is_dir() {
                continue;
            }
            if path.extension().and_then(|v| v.to_str()) == Some("clap") {
                if !result.bundles.contains(&path) {
                    result.bundles.push(path);
                }
                if result.bundles.len() > MAX_USER_BUNDLES {
                    return Err("clap bundle capacity exceeded".into());
                }
            } else {
                if depth == MAX_USER_DEPTH {
                    return Err("clap folder depth bound exceeded".into());
                }
                children.push(path);
            }
        }
        children.sort();
        pending.extend(children.into_iter().rev().map(|path| (path, depth + 1)));
    }
    result.bundles.sort();
    Ok(result)
}

/// Extract a `<key>...</key><string>...</string>` value from an XML Info.plist.
fn xml_plist_string(plist: &str, key: &str) -> Option<String> {
    let key_tag = format!("<key>{key}</key>");
    let after_key = plist.split_once(&key_tag)?.1;
    let after_open = after_key.split_once("<string>")?.1;
    let raw = after_open.split_once("</string>")?.0.trim();
    if raw.is_empty() || raw.len() > 256 || raw.contains('\0') {
        return None;
    }
    Some(raw.to_string())
}

fn is_supported_macho_magic(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
    matches!(
        magic,
        // Thin 64-bit little-endian Mach-O (arm64 / x86_64)
        [0xcf, 0xfa, 0xed, 0xfe]
        // Universal / Fat Mach-O (32-bit or 64-bit big-endian header)
        | [0xca, 0xfe, 0xba, 0xbe]
        | [0xca, 0xfe, 0xba, 0xbf]
    )
}

/// Inspect one macOS `.clap` bundle without executing foreign code.
pub fn inspect_clap_bundle(path: &Path) -> Result<ClapBundleIdentity, String> {
    if !path_valid(path) || path.extension().and_then(|s| s.to_str()) != Some("clap") {
        return Err("absolute bounded .clap bundle path required".into());
    }
    if !existing_plain_dir(path)? {
        return Err("clap bundle not found".into());
    }
    let canonical_path =
        fs::canonicalize(path).map_err(|e| format!("clap bundle canonicalization failed: {e}"))?;
    if !path_valid(&canonical_path) {
        return Err("clap bundle canonical path invalid".into());
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.contains('\0'))
        .ok_or("clap bundle name invalid")?
        .to_string();

    let info_path = canonical_path.join("Contents/Info.plist");
    let (info_plist_sha256, bundle_identifier, executable_name) =
        match fs::symlink_metadata(&info_path) {
            Ok(_) => {
                plain_file_path(&info_path)?;
                let info_bytes = file_bytes(&info_path, MAX_INFO_PLIST_BYTES)?;
                let info_sha = hash(&info_bytes);
                let text = std::str::from_utf8(&info_bytes).ok();
                let bundle_id = text.and_then(|t| xml_plist_string(t, "CFBundleIdentifier"));
                let exec_name = text
                    .and_then(|t| xml_plist_string(t, "CFBundleExecutable"))
                    .unwrap_or_else(|| stem.clone());
                (Some(info_sha), bundle_id, exec_name)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (None, None, stem.clone()),
            Err(e) => return Err(format!("clap Info.plist unavailable: {e}")),
        };

    if executable_name.contains('/')
        || executable_name.contains('\\')
        || executable_name == "."
        || executable_name == ".."
    {
        return Err("invalid clap CFBundleExecutable".into());
    }

    let executable_relative_path = format!("Contents/MacOS/{executable_name}");
    let executable_path = canonical_path.join(&executable_relative_path);
    plain_file_path(&executable_path)?;
    let executable_bytes = file_bytes(&executable_path, MAX_BINARY)?;
    if !is_supported_macho_magic(&executable_bytes) {
        return Err("clap bundle executable is not a Mach-O image".into());
    }
    let executable_sha256 = hash(&executable_bytes);
    let thin_arm64_module = macho_identity::validate(
        &executable_bytes,
        &executable_sha256,
        macho_identity::ImageRole::Module,
    )
    .is_ok();

    Ok(ClapBundleIdentity {
        supplied_path: path.to_path_buf(),
        canonical_path,
        bundle_name: stem,
        bundle_identifier,
        info_plist_sha256,
        executable_relative_path,
        executable_sha256,
        thin_arm64_module,
    })
}

impl ClapBundleIdentity {
    /// Revalidate this bundle on disk without executing foreign code.
    pub fn unchanged(&self) -> Result<(), String> {
        let current = inspect_clap_bundle(&self.supplied_path)?;
        if current.canonical_path != self.canonical_path
            || current.info_plist_sha256 != self.info_plist_sha256
            || current.executable_relative_path != self.executable_relative_path
            || current.executable_sha256 != self.executable_sha256
        {
            return Err("clap bundle changed on disk; rescan required".into());
        }
        Ok(())
    }
}

/// Enumerate and statically inspect all `.clap` bundles in explicit roots.
pub fn scan_clap_folders(
    roots: &[PathBuf],
    mut cancelled: impl FnMut() -> bool,
) -> Result<ClapFolderScanReport, String> {
    let enumeration = enumerate_clap_folders(roots, &mut cancelled)?;
    let mut results = Vec::with_capacity(enumeration.bundles.len());
    for bundle in enumeration.bundles {
        if cancelled() {
            return Err("clap folder scan cancelled".into());
        }
        let outcome = inspect_clap_bundle(&bundle);
        results.push((bundle, outcome));
    }
    Ok(ClapFolderScanReport {
        results,
        skipped_links: enumeration.skipped_links,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-clap-user-folder-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn thin_arm64_module_bytes() -> Vec<u8> {
        let mut b = vec![0u8; 56];
        for (at, n) in [
            (0, 0xfeedfacf_u32),
            (4, 0x0100000c),
            (12, 6), // MH_BUNDLE
            (16, 1),
            (20, 24),
            (32, 0x1b), // LC_UUID
            (36, 24),
        ] {
            b[at..at + 4].copy_from_slice(&n.to_le_bytes());
        }
        b[40..56].fill(7);
        b
    }

    fn write_clap_bundle(root: &Path, name: &str, bundle_id: &str, exec_bytes: &[u8]) -> PathBuf {
        let bundle = root.join(format!("{name}.clap"));
        fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>{bundle_id}</string>
    <key>CFBundleExecutable</key>
    <string>{name}</string>
</dict>
</plist>"#
        );
        fs::write(bundle.join("Contents/Info.plist"), plist).unwrap();
        fs::write(bundle.join(format!("Contents/MacOS/{name}")), exec_bytes).unwrap();
        bundle
    }

    #[test]
    fn macos_clap_scan_paths_resolves_system_user_and_custom_roots() {
        let custom = PathBuf::from("/tmp/custom-clap");
        let roots = macos_clap_scan_paths(true, true, Some(custom.clone())).unwrap();
        assert_eq!(roots.len(), 3);
        assert_eq!(roots[0], PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        assert!(roots[1].ends_with("Library/Audio/Plug-Ins/CLAP"));
        assert_eq!(roots[2], custom);
    }

    #[test]
    fn enumeration_stops_at_clap_bundles_skips_links_and_enforces_bounds() {
        let f = Fixture::new();
        let first = f.0.join("vendor/Synth.clap");
        let second = f.0.join("Delay.clap");
        fs::create_dir_all(first.join("Contents/Inner.clap")).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(f.0.join("not-a-bundle.clap"), b"plain file").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&first, f.0.join("alias.clap")).unwrap();

        let found = enumerate_clap_folders(&[f.0.clone(), f.0.join("absent")], || false).unwrap();
        assert_eq!(found.bundles, vec![second, first]);
        #[cfg(unix)]
        assert_eq!(found.skipped_links, 1);

        assert!(
            enumerate_clap_folders(std::slice::from_ref(&f.0), || true)
                .unwrap_err()
                .contains("cancelled")
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&f.0, f.0.join("linked-root")).unwrap();
            assert!(enumerate_clap_folders(&[f.0.join("linked-root")], || false).is_err());
        }
        assert!(enumerate_clap_folders(&[PathBuf::from("relative/clap")], || false).is_err());
    }

    #[test]
    fn inspect_and_scan_validate_macho_bundles_and_detect_modifications() {
        let f = Fixture::new();
        let good = write_clap_bundle(
            &f.0,
            "HexClapSynth",
            "org.portalsurfer.hex.clap.synth",
            &thin_arm64_module_bytes(),
        );
        let bad = write_clap_bundle(&f.0, "CorruptClap", "org.hex.bad", b"not macho bytes");

        let report = scan_clap_folders(std::slice::from_ref(&f.0), || false).unwrap();
        assert_eq!(report.results.len(), 2);

        let good_res = report
            .results
            .iter()
            .find(|(p, _)| p == &good)
            .unwrap()
            .1
            .as_ref()
            .expect("valid clap bundle");
        assert_eq!(good_res.bundle_name, "HexClapSynth");
        assert_eq!(
            good_res.bundle_identifier.as_deref(),
            Some("org.portalsurfer.hex.clap.synth")
        );
        assert!(good_res.thin_arm64_module);
        assert!(good_res.info_plist_sha256.is_some());
        assert_eq!(good_res.unchanged(), Ok(()));

        // Mutating the executable invalidates unchanged().
        let mut modified = thin_arm64_module_bytes();
        modified[40] = 99;
        fs::write(good.join("Contents/MacOS/HexClapSynth"), modified).unwrap();
        assert!(good_res.unchanged().is_err());

        let bad_res = &report.results.iter().find(|(p, _)| p == &bad).unwrap().1;
        assert!(bad_res.as_ref().unwrap_err().contains("Mach-O"));
    }
}
