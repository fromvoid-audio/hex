//! Build-only qualification under the same installed-toolchain trust as rustc/std.
//! Not a signature, security, physical-allocation or runtime-behavior attestation.
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn benign_args(value: Option<&std::ffi::OsStr>) -> bool {
    match value {
        None => true,
        Some(value) => matches!(
            value.to_str(),
            Some("") | Some("-D__CLIPPY_HACKERY__warnings__CLIPPY_HACKERY__")
        ),
    }
}

pub fn matching_driver(
    compiler: &str,
    driver: &str,
    sysroot: &str,
    root: &Path,
    version: &str,
) -> bool {
    driver.trim() == compiler.trim()
        && Path::new(sysroot.trim()) == root
        && version.trim() == "clippy 0.1.94 (4a4ef493e3 2026-03-02)"
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    mode: u32,
    links: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
#[cfg(unix)]
fn stamp(path: &Path) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute()
        || path
            .ancestors()
            .any(|p| fs::symlink_metadata(p).map_or(true, |m| m.file_type().is_symlink()))
    {
        return None;
    }
    let m = fs::symlink_metadata(path).ok()?;
    if !m.is_file() {
        return None;
    }
    Some(Stamp {
        dev: m.dev(),
        ino: m.ino(),
        size: m.len(),
        mode: m.mode(),
        links: m.nlink(),
        mtime: (m.mtime(), m.mtime_nsec()),
        ctime: (m.ctime(), m.ctime_nsec()),
    })
}
#[cfg(not(unix))]
#[derive(Debug, PartialEq, Eq)]
struct Stamp;
#[cfg(not(unix))]
fn stamp(_: &Path) -> Option<Stamp> {
    None
}

#[derive(Debug)]
pub struct InstalledFiles(Vec<(PathBuf, Stamp)>);
impl InstalledFiles {
    pub fn capture(root: &Path, compiler: &Path, wrapper: &Path, target: &str) -> Option<Self> {
        if compiler != root.join("bin/rustc") || wrapper != root.join("bin/clippy-driver") {
            return None;
        }
        let rustlib = root.join("lib/rustlib");
        let files = [
            compiler.to_path_buf(),
            wrapper.to_path_buf(),
            rustlib.join("components"),
            rustlib.join("rust-installer-version"),
            rustlib.join(format!("manifest-clippy-preview-{target}")),
            rustlib.join(format!("manifest-rust-std-{target}")),
        ];
        let snapshots = files
            .iter()
            .map(|p| Some((p.clone(), stamp(p)?)))
            .collect::<Option<Vec<_>>>()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if files[..2]
                .iter()
                .any(|p| fs::metadata(p).map_or(true, |m| m.permissions().mode() & 0o111 == 0))
            {
                return None;
            }
        }
        if !fs::read_to_string(&files[2])
            .ok()?
            .lines()
            .any(|l| l == format!("clippy-preview-{target}"))
            || !fs::read_to_string(&files[4])
                .ok()?
                .lines()
                .any(|l| l == "file:bin/clippy-driver")
        {
            return None;
        }
        Some(Self(snapshots))
    }
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.0.iter().map(|(p, _)| p.as_path())
    }
    pub fn unchanged(&self) -> bool {
        self.0.iter().all(|(p, s)| stamp(p).as_ref() == Some(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsStr,
        sync::atomic::{AtomicU64, Ordering},
    };
    #[test]
    fn exact_actual_lint_args_only() {
        for s in [
            None,
            Some(""),
            Some("-D__CLIPPY_HACKERY__warnings__CLIPPY_HACKERY__"),
        ] {
            assert!(benign_args(s.map(OsStr::new)));
        }
        for s in [
            "-Dwarnings",
            "-D__CLIPPY_HACKERY__warnings__CLIPPY_HACKERY__--sysroot=x",
            "@args",
            "--cfg__CLIPPY_HACKERY__layout",
            "__CLIPPY_HACKERY__",
            "--no-deps",
            "-Zrandomize-layout",
        ] {
            assert!(!benign_args(Some(OsStr::new(s))), "{s}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            assert!(!benign_args(Some(OsStr::from_bytes(b"\xff"))));
        }
    }
    #[test]
    fn metadata_queries_do_not_substitute_for_args_or_compiler_pin() {
        let root = Path::new("/installed");
        let v = "rustc pinned complete metadata";
        assert!(matching_driver(
            v,
            v,
            "/installed\n",
            root,
            "clippy 0.1.94 (4a4ef493e3 2026-03-02)\n"
        ));
        for (driver, sysroot, version) in [
            (
                "other",
                "/installed",
                "clippy 0.1.94 (4a4ef493e3 2026-03-02)",
            ),
            (v, "/foreign", "clippy 0.1.94 (4a4ef493e3 2026-03-02)"),
            (v, "/installed", "clippy 0.1.95 (other)"),
        ] {
            assert!(!matching_driver(v, driver, sysroot, root, version));
        }
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "hex-pcm-clippy-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            let p = p.canonicalize().unwrap();
            fs::create_dir_all(p.join("bin")).unwrap();
            fs::create_dir_all(p.join("lib/rustlib")).unwrap();
            for (name, data) in [
                ("bin/rustc", "compiler"),
                ("bin/clippy-driver", "driver"),
                ("lib/rustlib/components", "clippy-preview-test\n"),
                ("lib/rustlib/rust-installer-version", "3\n"),
                (
                    "lib/rustlib/manifest-clippy-preview-test",
                    "file:bin/clippy-driver\n",
                ),
                ("lib/rustlib/manifest-rust-std-test", "file:std\n"),
            ] {
                fs::write(p.join(name), data).unwrap();
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                for f in ["bin/rustc", "bin/clippy-driver"] {
                    fs::set_permissions(p.join(f), fs::Permissions::from_mode(0o755)).unwrap();
                }
            }
            Self(p)
        }
        fn capture(&self) -> Option<InstalledFiles> {
            InstalledFiles::capture(
                &self.0,
                &self.0.join("bin/rustc"),
                &self.0.join("bin/clippy-driver"),
                "test",
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[cfg(unix)]
    #[test]
    fn private_install_identity_manifest_and_replacement_negatives() {
        let f = Fixture::new();
        assert!(f.capture().unwrap().unchanged());
        assert_eq!(f.capture().unwrap().paths().count(), 6);
        assert!(
            InstalledFiles::capture(
                &f.0,
                &f.0.join("bin/other-rustc"),
                &f.0.join("bin/clippy-driver"),
                "test"
            )
            .is_none()
        );
        assert!(
            InstalledFiles::capture(
                &f.0,
                &f.0.join("bin/rustc"),
                &f.0.join("lookalike/clippy-driver"),
                "test"
            )
            .is_none()
        );
        assert!(
            InstalledFiles::capture(
                &f.0,
                &f.0.join("bin/rustc"),
                &f.0.join("bin/clippy-driver"),
                "foreign-target"
            )
            .is_none()
        );
        for file in [
            "lib/rustlib/components",
            "lib/rustlib/manifest-clippy-preview-test",
        ] {
            let p = f.0.join(file);
            let bytes = fs::read(&p).unwrap();
            fs::write(&p, "missing").unwrap();
            assert!(f.capture().is_none());
            fs::write(&p, bytes).unwrap();
        }
        let captured = f.capture().unwrap();
        let p = f.0.join("bin/clippy-driver");
        fs::rename(&p, f.0.join("old-driver")).unwrap();
        fs::copy(f.0.join("old-driver"), &p).unwrap();
        assert!(!captured.unchanged());
        let captured = f.capture().unwrap();
        fs::write(&p, "changed driver").unwrap();
        assert!(!captured.unchanged());
        fs::remove_file(&p).unwrap();
        std::os::unix::fs::symlink(f.0.join("old-driver"), &p).unwrap();
        assert!(f.capture().is_none());
        fs::remove_file(&p).unwrap();
        fs::rename(f.0.join("old-driver"), &p).unwrap();
        fs::rename(f.0.join("bin"), f.0.join("real-bin")).unwrap();
        std::os::unix::fs::symlink(f.0.join("real-bin"), f.0.join("bin")).unwrap();
        assert!(f.capture().is_none());
    }
}
