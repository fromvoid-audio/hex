//! Bounded installed-location enumeration. Never loads code or enters bundles.
use crate::{MAX_MODULES, path_valid};
use std::{
    fs,
    path::{Path, PathBuf},
};
const MAX_ENTRIES: usize = 4096;
const MAX_DIRECTORIES: usize = 1024;
const MAX_DEPTH: usize = 8;

#[derive(Debug)]
pub struct Enumeration {
    pub bundles: Vec<PathBuf>,
    pub skipped_links: usize,
}

/// Standard macOS local/user locations. No home or checkout fallback.
pub fn macos_standard_paths() -> Result<Vec<PathBuf>, String> {
    macos_scan_paths(true, true, None)
}

/// Resolve only the selected roots, before the bounded batch starts.
pub fn macos_scan_paths(
    system: bool,
    user: bool,
    custom: Option<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    if system {
        roots.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
    }
    if user {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("user home unavailable")?;
        if !path_valid(&home) {
            return Err("user home path invalid".into());
        }
        roots.push(home.join("Library/Audio/Plug-Ins/VST3"));
    }
    roots.extend(custom);
    Ok(roots)
}

fn existing_root(path: &Path) -> Result<bool, String> {
    if !path_valid(path) {
        return Err("invalid vst3 folder path".into());
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        match fs::symlink_metadata(&prefix) {
            Ok(info) if info.is_dir() && !info.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(format!(
                    "vst3 folder has linked or non-directory component: {}",
                    prefix.display()
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(format!("vst3 folder unavailable: {e}")),
        }
    }
    Ok(true)
}

/// At most three roots; stop before any foreign execution on traversal/cap failure.
/// Symlinks encountered within an existing root are counted and never followed.
pub fn enumerate(
    roots: &[PathBuf],
    mut cancelled: impl FnMut() -> bool,
) -> Result<Enumeration, String> {
    if roots.is_empty() || roots.len() > 3 {
        return Err("one to three vst3 folders required".into());
    }
    let mut pending = Vec::new();
    for root in roots {
        if cancelled() {
            return Err("folder scan cancelled".into());
        }
        if root.extension().and_then(|s| s.to_str()) == Some("vst3") {
            return Err("select folders containing vst3 bundles".into());
        }
        if existing_root(root)? && !pending.iter().any(|(path, _)| path == root) {
            pending.push((root.clone(), 0));
        }
    }
    let mut result = Enumeration {
        bundles: Vec::new(),
        skipped_links: 0,
    };
    let mut entries = 0;
    let mut directories = 0;
    while let Some((directory, depth)) = pending.pop() {
        if cancelled() {
            return Err("folder scan cancelled".into());
        }
        // Recheck before traversal; native module admission also independently
        // rejects linked/changed paths immediately before loading selected code.
        if !existing_root(&directory)? {
            return Err("vst3 folder disappeared during enumeration".into());
        }
        directories += 1;
        if directories > MAX_DIRECTORIES {
            return Err("vst3 directory bound exceeded".into());
        }
        let mut children = Vec::new();
        for entry in fs::read_dir(&directory)
            .map_err(|e| format!("vst3 folder listing failed at {}: {e}", directory.display()))?
        {
            if cancelled() {
                return Err("folder scan cancelled".into());
            }
            entries += 1;
            if entries > MAX_ENTRIES {
                return Err("vst3 folder entry bound exceeded".into());
            }
            let entry = entry.map_err(|e| format!("vst3 folder entry failed: {e}"))?;
            let path = entry.path();
            if !path_valid(&path) {
                return Err("vst3 folder member path invalid or oversized".into());
            }
            let kind = entry
                .file_type()
                .map_err(|e| format!("vst3 folder member unavailable: {e}"))?;
            if kind.is_symlink() {
                result.skipped_links += 1;
                continue;
            }
            if !kind.is_dir() {
                continue;
            }
            if path.extension().and_then(|v| v.to_str()) == Some("vst3") {
                if !result.bundles.contains(&path) {
                    result.bundles.push(path);
                }
                if result.bundles.len() > MAX_MODULES {
                    return Err("vst3 bundle capacity exceeded".into());
                }
            } else {
                if depth == MAX_DEPTH {
                    return Err("vst3 folder depth bound exceeded".into());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hex-plugin-roots-{}-{}",
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
    #[test]
    fn deterministic_nested_discovery_stops_at_bundles_and_never_follows_links() {
        let f = Fixture::new();
        let first = f.0.join("vendor/B.vst3");
        let second = f.0.join("A.vst3");
        fs::create_dir_all(first.join("Contents/NotAPlugin.vst3")).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(f.0.join("file.vst3"), b"not a bundle directory").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&first, f.0.join("alias.vst3")).unwrap();
        let found = enumerate(&[f.0.clone(), f.0.join("missing")], || false).unwrap();
        assert_eq!(found.bundles, vec![second, first]);
        #[cfg(unix)]
        assert_eq!(found.skipped_links, 1);
        assert!(
            enumerate(std::slice::from_ref(&f.0), || true)
                .unwrap_err()
                .contains("cancelled")
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&f.0, f.0.join("linked-root")).unwrap();
            assert!(enumerate(&[f.0.join("linked-root")], || false).is_err());
        }
    }
    #[test]
    fn capacity_depth_entry_and_invalid_root_fail_before_discovery() {
        let f = Fixture::new();
        let modules = f.0.join("modules");
        fs::create_dir(&modules).unwrap();
        for n in 0..=MAX_MODULES {
            fs::create_dir(modules.join(format!("{n}.vst3"))).unwrap();
        }
        assert!(
            enumerate(&[modules], || false)
                .unwrap_err()
                .contains("capacity")
        );
        let depth = f.0.join("depth");
        let mut path = depth.clone();
        fs::create_dir(&path).unwrap();
        for _ in 0..=MAX_DEPTH {
            path = path.join("nested");
            fs::create_dir(&path).unwrap();
        }
        assert!(enumerate(&[depth], || false).unwrap_err().contains("depth"));
        let directories = f.0.join("directories");
        fs::create_dir(&directories).unwrap();
        for n in 0..MAX_DIRECTORIES {
            fs::create_dir(directories.join(n.to_string())).unwrap();
        }
        assert!(
            enumerate(&[directories], || false)
                .unwrap_err()
                .contains("directory bound")
        );
        let entries = f.0.join("entries");
        fs::create_dir(&entries).unwrap();
        for n in 0..=MAX_ENTRIES {
            fs::write(entries.join(n.to_string()), []).unwrap();
        }
        assert!(
            enumerate(&[entries], || false)
                .unwrap_err()
                .contains("entry bound")
        );
        assert!(enumerate(&[PathBuf::from("relative")], || false).is_err());
        let missing = enumerate(&[f.0.join("absent")], || false).unwrap();
        assert!(missing.bundles.is_empty());
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn full_catalog_refuses_new_folder_members_before_even_inspecting_tools() {
        let f = Fixture::new();
        let folder = f.0.join("folder");
        fs::create_dir(&folder).unwrap();
        fs::create_dir(folder.join("New.vst3")).unwrap();
        let file = f.0.join("catalog.json");
        crate::storage::update(&file, |catalog| {
            for n in 0..MAX_MODULES {
                catalog.record(
                    f.0.join(format!("old{n}.vst3")),
                    Err("retained failure".into()),
                )?;
            }
            Ok(())
        })
        .unwrap();
        let before = fs::read(&file).unwrap();
        let owner = crate::worker::Cancellation::default();
        let result = crate::worker::scan_folders(
            &file,
            &f.0.join("absent-tools.app"),
            &[folder],
            owner.token(),
        );
        assert!(result.catalog.is_none());
        assert!(result.status.contains("capacity"), "{}", result.status);
        assert_eq!(fs::read(file).unwrap(), before);
    }
}
