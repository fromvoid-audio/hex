//! Shared lifetime custody for installer-owned development bundles.
//!
//! The installer creates `.hex-test-build.lock` before first publication and
//! never removes it. The desktop holds a shared lock from before any worker
//! starts until shutdown; publication uses the existing exclusive lock. A
//! missing receipt in an owned location refuses rather than bypassing custody.
//! Read-only distribution bundles without this installer lock remain separate.
//! This cooperative boundary cannot stop the OS mapping code before `main`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read};
use std::path::Path;

const MAX_RECEIPT_BYTES: u64 = 8 * 1024 * 1024;

// Unlock the shared open-file description explicitly before closing it. A
// transient descriptor duplicate (for example during process creation) must
// not extend this process's custody beyond the lifetime of its lease.
pub(super) struct UpdateLease(File);

impl Drop for UpdateLease {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

pub(super) fn acquire_for_current_executable() -> Result<Option<UpdateLease>, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not identify executable for update custody: {error}"))?;
    acquire_for_reported_executable(&executable, hex_ui::diagnostic_build_identity()).map_err(
        |error| {
            format!("could not acquire update custody; retry after the update finishes: {error}")
        },
    )
}

fn acquire_for_reported_executable(
    executable: &Path,
    build: &str,
) -> io::Result<Option<UpdateLease>> {
    // macOS can report an invocation symlink. Resolve before deciding whether
    // this is a bundle; an unavailable or removed executable fails closed.
    acquire_for_executable(&fs::canonicalize(executable)?, build)
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}

fn acquire_for_executable(executable: &Path, build: &str) -> io::Result<Option<UpdateLease>> {
    if executable.file_name().is_none_or(|name| name != "hex") {
        return Ok(None);
    }
    let Some(macos) = executable.parent() else {
        return Ok(None);
    };
    let Some(contents) = macos.parent() else {
        return Ok(None);
    };
    let Some(bundle) = contents.parent() else {
        return Ok(None);
    };
    if macos.file_name().is_none_or(|name| name != "MacOS")
        || contents.file_name().is_none_or(|name| name != "Contents")
    {
        return Ok(None);
    }
    let Some(parent) = bundle.parent() else {
        return Err(invalid("invalid bundle path"));
    };
    if parent
        .file_name()
        .is_some_and(|name| name == ".hex-install")
    {
        return Err(invalid("temporary installer custody cannot run"));
    }
    let path = parent.join(".hex-test-build.lock");
    let receipt = contents.join("Resources/test-build.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            // An installer-owned app never loses its permanent lock. This also
            // refuses an old executable mapped from temporary previous custody.
            return match fs::symlink_metadata(&receipt) {
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
                Ok(_) => Err(invalid("test bundle has no permanent installer lock")),
            };
        }
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("invalid update lock"));
    }
    let file = OpenOptions::new().read(true).write(true).open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        let after = fs::symlink_metadata(&path)?;
        if !after.is_file()
            || after.file_type().is_symlink()
            || (metadata.dev(), metadata.ino()) != (opened.dev(), opened.ino())
            || (opened.dev(), opened.ino()) != (after.dev(), after.ino())
        {
            return Err(invalid("update lock changed"));
        }
    }
    fs2::FileExt::try_lock_shared(&file)?;
    let lease = UpdateLease(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let locked = lease.0.metadata()?;
        let current = fs::symlink_metadata(&path)?;
        if !current.is_file()
            || current.file_type().is_symlink()
            || (locked.dev(), locked.ino()) != (current.dev(), current.ino())
        {
            return Err(invalid("update lock changed after acquisition"));
        }
    }
    if bundle
        .file_name()
        .and_then(|name| name.to_str())
        .is_none_or(|name| !name.eq_ignore_ascii_case("hex.app"))
    {
        return Err(invalid("development bundle was moved"));
    }
    let metadata = fs::symlink_metadata(&receipt)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_RECEIPT_BYTES
    {
        return Err(invalid("invalid test build receipt"));
    }
    let mut bytes = Vec::new();
    File::open(&receipt)?
        .take(MAX_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return Err(invalid("test build receipt is too large"));
    }
    let receipt: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| invalid("invalid test build receipt"))?;
    let Some((_, provenance)) = build.rsplit_once(" source=") else {
        return Err(invalid("running build provenance is unavailable"));
    };
    let mut fields = provenance.split(':');
    let (Some(source), Some(tree), Some("clean"), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(invalid("running build provenance is unavailable"));
    };
    let installed_bundle = receipt
        .get("app_path")
        .and_then(|value| value.as_str())
        .and_then(|path| fs::canonicalize(path).ok());
    let actual_bundle = fs::canonicalize(bundle)?;
    if receipt.get("schema").and_then(|v| v.as_u64()) != Some(2)
        || receipt.get("profile").and_then(|v| v.as_str()) != Some("release")
        || source.len() != 40
        || tree.len() != 64
        || !source.bytes().all(|b| b.is_ascii_hexdigit())
        || !tree.bytes().all(|b| b.is_ascii_hexdigit())
        || receipt.get("source").and_then(|v| v.as_str()) != Some(source)
        || receipt.get("source_sha256").and_then(|v| v.as_str()) != Some(tree)
        || installed_bundle.as_deref() != Some(actual_bundle.as_path())
    {
        return Err(invalid(
            "running build does not match the installed test bundle",
        ));
    }
    Ok(Some(lease))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    const SOURCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TREE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "hex-update-lease-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("create private fixture directory");
            Self(root)
        }
        fn executable(&self) -> std::path::PathBuf {
            let bundle = self.0.join("hex.app");
            let contents = bundle.join("Contents");
            fs::create_dir_all(contents.join("MacOS")).unwrap();
            fs::create_dir_all(contents.join("Resources")).unwrap();
            fs::write(
                contents.join("Resources/test-build.json"),
                serde_json::json!({"schema":2,"profile":"release","source":SOURCE,"source_sha256":TREE,"app_path":bundle})
                    .to_string(),
            )
            .unwrap();
            fs::write(self.0.join(".hex-test-build.lock"), b"").unwrap();
            let executable = contents.join("MacOS/hex");
            fs::write(&executable, b"fixture").unwrap();
            executable
        }
        fn build() -> String {
            format!("0.1 source={SOURCE}:{TREE}:clean")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn running_bundles_hold_shared_custody_until_all_instances_exit() {
        let fixture = Fixture::new();
        let executable = fixture.executable();
        let first = acquire_for_executable(&executable, &Fixture::build())
            .unwrap()
            .unwrap();
        let second = acquire_for_executable(&executable, &Fixture::build())
            .unwrap()
            .unwrap();
        let installer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(fixture.0.join(".hex-test-build.lock"))
            .unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&installer).is_err());
        drop(first);
        assert!(fs2::FileExt::try_lock_exclusive(&installer).is_err());
        drop(second);
        fs2::FileExt::try_lock_exclusive(&installer).unwrap();
        assert!(acquire_for_executable(&executable, &Fixture::build()).is_err());
        fs2::FileExt::unlock(&installer).unwrap();
        assert!(
            acquire_for_executable(&executable, &Fixture::build())
                .unwrap()
                .is_some()
        );
    }

    #[cfg(unix)]
    #[test]
    fn dropped_lease_releases_custody_even_if_its_descriptor_was_duplicated() {
        let fixture = Fixture::new();
        let executable = fixture.executable();
        let lease = acquire_for_executable(&executable, &Fixture::build())
            .unwrap()
            .unwrap();
        let second = acquire_for_executable(&executable, &Fixture::build())
            .unwrap()
            .unwrap();
        let inherited = lease.0.try_clone().unwrap();
        let installer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(fixture.0.join(".hex-test-build.lock"))
            .unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&installer).is_err());
        drop(lease);
        assert!(fs2::FileExt::try_lock_exclusive(&installer).is_err());
        drop(second);
        fs2::FileExt::try_lock_exclusive(&installer).unwrap();
        drop(inherited);
    }

    #[cfg(unix)]
    #[test]
    fn dropped_lease_releases_custody_while_child_holds_duplicate() {
        use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, posix_spawn};
        use nix::sys::signal::{Signal, kill};
        use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
        use std::os::fd::AsRawFd;

        let fixture = Fixture::new();
        let executable = fixture.executable();
        let lease = acquire_for_executable(&executable, &Fixture::build())
            .unwrap()
            .unwrap();
        let installer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(fixture.0.join(".hex-test-build.lock"))
            .unwrap();
        let mut actions = PosixSpawnFileActions::init().unwrap();
        actions.add_dup2(lease.0.as_raw_fd(), 198).unwrap();
        let attributes = PosixSpawnAttr::init().unwrap();
        let child = posix_spawn(
            c"/bin/sleep",
            &actions,
            &attributes,
            &[c"sleep", c"5"],
            &[] as &[&std::ffi::CStr],
        )
        .unwrap();
        let held_while_live = fs2::FileExt::try_lock_exclusive(&installer).is_err();
        drop(lease);
        let released_while_child_lives = fs2::FileExt::try_lock_exclusive(&installer);
        let child_alive = matches!(
            waitpid(child, Some(WaitPidFlag::WNOHANG)),
            Ok(WaitStatus::StillAlive)
        );
        let _ = kill(child, Signal::SIGKILL);
        let _ = waitpid(child, None);
        assert!(held_while_live);
        assert!(child_alive);
        assert!(
            released_while_child_lives.is_ok(),
            "child duplicate prolonged custody"
        );
    }

    #[cfg(unix)]
    #[test]
    fn helper_process_does_not_inherit_shared_custody() {
        let fixture = Fixture::new();
        let executable = fixture.executable();
        let lease = acquire_for_executable(&executable, &Fixture::build())
            .unwrap()
            .unwrap();
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("5")
            .spawn()
            .unwrap();
        drop(lease);
        let installer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(fixture.0.join(".hex-test-build.lock"))
            .unwrap();
        let result = fs2::FileExt::try_lock_exclusive(&installer);
        let _ = child.kill();
        let _ = child.wait();
        assert!(result.is_ok(), "spawned helper retained shared custody");
    }

    #[test]
    fn missing_or_changed_receipt_never_bypasses_installer_custody() {
        let fixture = Fixture::new();
        let executable = fixture.executable();
        let changed_source = format!("0.1 source={}:{}:clean", "c".repeat(40), TREE);
        assert!(acquire_for_executable(&executable, &changed_source).is_err());
        fs::remove_file(fixture.0.join("hex.app/Contents/Resources/test-build.json")).unwrap();
        assert!(acquire_for_executable(&executable, &Fixture::build()).is_err());
        fs::remove_file(fixture.0.join(".hex-test-build.lock")).unwrap();
        assert!(
            acquire_for_executable(&executable, &Fixture::build())
                .unwrap()
                .is_none()
        );
        assert!(
            acquire_for_executable(&fixture.0.join("hex-desktop"), &Fixture::build())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn non_test_bundle_without_custody_does_not_create_a_lock() {
        let fixture = Fixture::new();
        let executable = fixture.executable();
        fs::remove_file(fixture.0.join("hex.app/Contents/Resources/test-build.json")).unwrap();
        fs::remove_file(fixture.0.join(".hex-test-build.lock")).unwrap();
        assert!(
            acquire_for_reported_executable(&executable, &Fixture::build())
                .unwrap()
                .is_none()
        );
        assert!(!fixture.0.join(".hex-test-build.lock").exists());
    }

    #[test]
    fn copied_test_bundle_without_installer_lock_refuses() {
        let fixture = Fixture::new();
        let executable = fixture.executable();
        fs::remove_file(fixture.0.join(".hex-test-build.lock")).unwrap();
        assert!(acquire_for_executable(&executable, &Fixture::build()).is_err());
    }

    #[test]
    fn executable_mapped_from_temporary_installer_custody_refuses() {
        let fixture = Fixture::new();
        let executable = fixture.executable();
        let workspace = fixture.0.join(".hex-install");
        fs::create_dir(&workspace).unwrap();
        fs::rename(fixture.0.join("hex.app"), workspace.join("previous.app")).unwrap();
        fs::remove_file(workspace.join("previous.app/Contents/Resources/test-build.json")).unwrap();
        assert!(
            acquire_for_executable(
                &workspace.join("previous.app/Contents/MacOS/hex"),
                &Fixture::build()
            )
            .is_err()
        );
        assert!(!executable.exists());
    }

    #[cfg(unix)]
    #[test]
    fn executable_symlink_alias_cannot_bypass_bundle_custody() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let executable = fixture.executable();
        let alias = fixture.0.join("hex-alias");
        symlink(&executable, &alias).unwrap();
        let lease = acquire_for_reported_executable(&alias, &Fixture::build())
            .unwrap()
            .unwrap();
        let installer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(fixture.0.join(".hex-test-build.lock"))
            .unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&installer).is_err());
        drop(lease);
        fs2::FileExt::try_lock_exclusive(&installer).unwrap();
        fs::remove_file(executable).unwrap();
        assert!(acquire_for_reported_executable(&alias, &Fixture::build()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn linked_lock_refuses_without_touching_its_target() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let executable = fixture.executable();
        let unrelated = fixture.0.join("user-file");
        fs::write(&unrelated, b"preserve").unwrap();
        fs::remove_file(fixture.0.join(".hex-test-build.lock")).unwrap();
        symlink(&unrelated, fixture.0.join(".hex-test-build.lock")).unwrap();
        assert!(acquire_for_executable(&executable, &Fixture::build()).is_err());
        assert_eq!(fs::read(unrelated).unwrap(), b"preserve");
    }
}
