//! Explicit original packaged-tool evidence, never starts the Hex application.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use hex_plugin_catalog::{
    Catalog,
    worker::{self, Cancellation},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf, AtomicBool);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hex-plugin-worker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap(), AtomicBool::new(false))
    }
    fn catalog(&self) -> PathBuf {
        self.0.join("catalog.json")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.1.load(Ordering::Acquire) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn app() -> PathBuf {
    PathBuf::from(
        std::env::var_os("HEX_VST3_CATALOG_TEST_APP")
            .expect("explicit original signed package fixture required"),
    )
}
fn module(name: &str) -> PathBuf {
    PathBuf::from(
        std::env::var_os("HEX_VST3_CATALOG_FIXTURES")
            .expect("explicit original native fixtures required"),
    )
    .join("hex-vst3-host.app/Contents/Resources/factory-fixtures")
    .join(format!("{name}.vst3"))
}
fn copy(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        assert!(!entry.file_type().unwrap().is_symlink());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &destination);
        } else {
            // Signed fixture replacement gets a fresh inode; in-place writes
            // were independently observed to produce SIGKILL on this Mac.
            if destination.exists() {
                fs::remove_file(&destination).unwrap();
            }
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

#[test]
#[ignore = "explicit original signed package and native fixtures required"]
fn selected_packaged_thin_and_universal_scan_persist_actual_uids_and_restart_without_rescan() {
    let fixture = Fixture::new();
    for name in ["normal", "universal"] {
        let cancellation = Cancellation::default();
        let update = worker::scan(
            &fixture.catalog(),
            &app(),
            module(name),
            cancellation.token(),
        );
        assert!(update.status.contains("discovered"), "{}", update.status);
        assert!(update.catalog.is_some());
    }
    let restored = Catalog::load(&fixture.catalog()).unwrap();
    let rows = restored.rows("factory processor");
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|row| row.cid.is_some_and(|id| id[..4] == *b"HEXF" && id[15] == 1))
    );
    assert!(restored.entries.iter().all(|entry| {
        entry
            .last_good
            .as_ref()
            .is_some_and(|r| r.cleanup.direct_reaped && r.cleanup.direct_exit_code == Some(0))
    }));
    let bytes = fs::read(fixture.catalog()).unwrap();
    let again = Catalog::load(&fixture.catalog()).unwrap();
    assert_eq!(again.rows(""), restored.rows(""));
    assert_eq!(fs::read(fixture.catalog()).unwrap(), bytes);
}

#[test]
#[ignore = "explicit original signed package and native fixtures required"]
fn failed_rescans_and_cancelled_owned_worker_preserve_last_good_without_late_promotion() {
    let fixture = Fixture::new();
    let selected = fixture.0.join("Selected.vst3");
    copy(&module("normal"), &selected);
    let owner = Cancellation::default();
    let first = worker::scan(&fixture.catalog(), &app(), selected.clone(), owner.token());
    assert!(first.status.contains("discovered"), "{}", first.status);
    let original = Catalog::load(&fixture.catalog()).unwrap().entries[0]
        .last_good
        .as_ref()
        .unwrap()
        .executable
        .clone();
    for name in ["entry-crash", "malformed-count"] {
        copy(&module(name), &selected);
        let owner = Cancellation::default();
        let failed = worker::scan(&fixture.catalog(), &app(), selected.clone(), owner.token());
        assert!(failed.status.contains("failed"), "{}", failed.status);
        let saved = Catalog::load(&fixture.catalog()).unwrap();
        assert_eq!(
            saved.entries[0].last_good.as_ref().unwrap().executable,
            original
        );
        assert!(saved.entries[0].failure.is_some());
    }
    copy(&module("entry-hang"), &selected);
    let owner = Cancellation::default();
    let token = owner.token();
    let catalog = fixture.catalog();
    let package = app();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        let result = worker::scan(&catalog, &package, selected, token);
        let _ = sender.send(result);
    });
    std::thread::sleep(Duration::from_millis(200));
    if !owner.cancel() {
        let early = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("early worker result");
        thread.join().unwrap();
        panic!(
            "original hanging scan ended before cancellation: {}",
            early.status
        );
    }
    let cancelled = match receiver.recv_timeout(Duration::from_secs(15)) {
        Ok(value) => value,
        Err(error) => {
            fixture.1.store(true, Ordering::Release);
            panic!(
                "owned scanner retirement unresolved: {error}; fixture retained at {:?}; no forced termination",
                fixture.0
            );
        }
    };
    thread.join().unwrap();
    assert!(
        cancelled.status.contains("cancelled"),
        "{}",
        cancelled.status
    );
    let restored = Catalog::load(&fixture.catalog()).unwrap();
    assert_eq!(
        restored.entries[0].last_good.as_ref().unwrap().executable,
        original
    );
    assert!(
        restored.entries[0]
            .failure
            .as_ref()
            .unwrap()
            .contains("cancelled")
    );
}
