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
#[ignore = "explicit source-launch tools and original native fixtures required"]
fn source_launch_tools_scan_and_publish_catalog_metadata() {
    let fixture = Fixture::new();
    let tools = worker::available_app().unwrap();
    let cancellation = Cancellation::default();
    let update = worker::scan(
        &fixture.catalog(),
        &tools,
        module("normal"),
        cancellation.token(),
    );
    assert!(update.status.contains("discovered"), "{}", update.status);
    assert!(
        !Catalog::load(&fixture.catalog())
            .unwrap()
            .rows("")
            .is_empty()
    );
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
        assert!(
            update.status.contains("discovered"),
            "{}; {:?}",
            update.status,
            update.diagnostics
        );
        let observed = update
            .diagnostics
            .as_ref()
            .expect("actual scan observations");
        assert_eq!(observed.module, module(name));
        assert!(observed.scanner_started);
        assert!(observed.scanner_exit.is_some_and(|s| s.success()));
        assert!(
            observed
                .native_cleanup
                .as_ref()
                .is_some_and(|c| c.may_publish())
        );
        let report: hex_plugin_scan::macos::report::Report =
            serde_json::from_slice(&observed.report).unwrap();
        assert!(report.discovery.is_some());
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
    for name in [
        "entry-crash",
        "malformed-count",
        "factory2-mismatch",
        "factory2-malformed",
        "factory2-invalid-utf8",
    ] {
        copy(&module(name), &selected);
        let owner = Cancellation::default();
        let failed = worker::scan(&fixture.catalog(), &app(), selected.clone(), owner.token());
        assert!(failed.status.contains("failed"), "{}", failed.status);
        let observed = failed
            .diagnostics
            .as_ref()
            .expect("failed scan observations");
        assert_eq!(observed.module, selected);
        assert!(observed.scanner_started, "{observed:?}");
        assert!(observed.scanner_exit.is_some(), "{observed:?}");
        let report: hex_plugin_scan::macos::report::Report =
            serde_json::from_slice(&observed.report).expect("actual refused native report");
        assert_eq!(report.bundle.canonical_path, selected);
        assert!(report.cleanup.direct_reaped, "{observed:?}");
        assert!(
            !report.cleanup.pending && !report.cleanup.custody_lost,
            "{observed:?}"
        );
        assert!(
            report.error.is_some() || report.interruption.is_some(),
            "{observed:?}"
        );
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
    let started_deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !owner.has_started_scanner() {
        if let Ok(early) = receiver.try_recv() {
            thread.join().unwrap();
            panic!(
                "scan ended before actual scanner spawn: {}; {:?}",
                early.status, early.diagnostics
            );
        }
        if std::time::Instant::now() >= started_deadline {
            fixture.1.store(true, Ordering::Release);
            let _ = owner.cancel();
            panic!(
                "scanner spawn not observed; cancellation requested; fixture retained at {:?}",
                fixture.0
            );
        }
        std::thread::yield_now();
    }
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
        cancelled
            .diagnostics
            .as_ref()
            .is_some_and(|d| d.scanner_started && d.scanner_exit.is_some()),
        "{}; {:?}",
        cancelled.status,
        cancelled.diagnostics
    );
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

#[test]
#[ignore = "explicit original signed package and native fixtures required"]
fn native_deadline_report_is_retained_without_promoting_metadata() {
    let fixture = Fixture::new();
    let owner = Cancellation::default();
    let update = worker::scan(
        &fixture.catalog(),
        &app(),
        module("entry-hang"),
        owner.token(),
    );
    let observed = update
        .diagnostics
        .as_ref()
        .expect("actual deadline observations");
    let report: hex_plugin_scan::macos::report::Report = serde_json::from_slice(&observed.report)
        .unwrap_or_else(|e| panic!("{e}; {} ; {observed:?}", update.status));
    assert!(
        observed.scanner_started && observed.scanner_exit.is_some(),
        "{observed:?}"
    );
    assert_eq!(
        report.interruption.as_deref(),
        Some("deadline exceeded"),
        "{observed:?}"
    );
    assert!(
        report.cleanup.direct_reaped && !report.cleanup.pending && !report.cleanup.custody_lost,
        "{observed:?}"
    );
    assert!(
        report.discovery.is_none(),
        "deadline never grants accepted discovery"
    );
    let catalog = Catalog::load(&fixture.catalog()).unwrap();
    assert!(catalog.entries[0].last_good.is_none());
    assert!(catalog.entries[0].failure.is_some());
}

#[test]
#[ignore = "explicit original signed package and native fixtures required"]
fn actual_optional_factory_types_persist_and_search_without_foreign_restart_work() {
    let fixture = Fixture::new();
    for (name, kind) in [
        ("factory2-instrument", "instrument"),
        ("factory2-effect", "effect"),
        ("factory2-dual", "instrument and effect"),
        ("factory2-unknown", "processor · type unknown"),
    ] {
        let owner = Cancellation::default();
        let update = worker::scan(&fixture.catalog(), &app(), module(name), owner.token());
        assert!(update.status.contains("discovered"), "{}", update.status);
        let restored = Catalog::load(&fixture.catalog()).unwrap();
        let rows = restored.rows("");
        let row = rows.iter().find(|r| r.module == module(name)).unwrap();
        let action = if kind != "processor · type unknown" {
            let expected = restored
                .select_plugin(&row.module, row.cid.unwrap())
                .unwrap();
            assert_eq!(row.selection.as_deref(), Some(&expected));
            "drag or enter to insert"
        } else {
            assert!(row.selection.is_none());
            "hosting unavailable"
        };
        assert_eq!(row.status, format!("vst3 {kind} · {action}"));
        assert_eq!(row.vendor, "original extended vendor");
    }
    let bytes = fs::read(fixture.catalog()).unwrap();
    let restored = Catalog::load(&fixture.catalog()).unwrap();
    assert_eq!(restored.rows("instrument").len(), 2);
    assert_eq!(restored.rows("effect").len(), 2);
    assert_eq!(restored.rows("type unknown").len(), 1);
    assert_eq!(fs::read(fixture.catalog()).unwrap(), bytes);
}

#[test]
#[ignore = "explicit original signed package and native fixtures required"]
fn folder_batch_publishes_known_failures_and_actual_types_then_preserves_restart() {
    let f = Fixture::new();
    let root = f.0.join("root");
    fs::create_dir(&root).unwrap();
    copy(&module("malformed-count"), &root.join("A-bad.vst3"));
    copy(&module("factory2-instrument"), &root.join("vendor/B.vst3"));
    copy(&module("factory2-effect"), &root.join("C.vst3"));
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("C.vst3"), root.join("linked.vst3")).unwrap();
    // Both fixture entries were remembered as failures before discovery tools
    // were tracked. The signed folder scan must retry them without file changes.
    let bad = root.join("A-bad.vst3");
    let effect = root.join("C.vst3");
    let old_entries = [&bad, &effect].map(|path| {
        serde_json::json!({
            "path": path, "last_good": null, "source": null, "failure": "historical refusal",
            "failed_bundle": hex_plugin_scan::macos::identity::Bundle::inspect(path).unwrap(),
        })
    });
    let old_bytes = serde_json::to_vec(&serde_json::json!({
        "version": 3, "entries": old_entries,
    }))
    .unwrap();
    fs::write(f.catalog(), &old_bytes).unwrap();
    let migrated = Catalog::load(&f.catalog()).unwrap();
    assert_eq!(migrated.entries.len(), 2);
    assert!(
        migrated
            .entries
            .iter()
            .all(|entry| entry.failure.as_deref() == Some("historical refusal"))
    );
    assert_eq!(fs::read(f.catalog()).unwrap(), old_bytes);

    let owner = Cancellation::default();
    let result = worker::scan_folders(
        &f.catalog(),
        &app(),
        &[root.clone(), f.0.join("missing")],
        owner.token(),
    );
    assert!(result.catalog.is_some(), "{}", result.status);
    assert!(
        result.status.contains("2 discovered · 1 failed"),
        "{}; records: {:?}",
        result.status,
        result
            .catalog
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| (&e.path, &e.failure))
            .collect::<Vec<_>>()
    );
    assert!(result.status.contains("1 linked entries skipped"));
    assert!(result.status.contains("0 unchanged failures skipped"));
    assert!(!result.status.contains("hosting unavailable"));
    let bytes = fs::read(f.catalog()).unwrap();
    let saved = Catalog::load(&f.catalog()).unwrap();
    assert_eq!(saved.entries.len(), 3);
    assert_eq!(saved.rows("instrument").len(), 1);
    assert_eq!(saved.rows("effect").len(), 1);
    assert!(saved.rows("").iter().any(|r| r.cid.is_none()));
    assert_eq!(fs::read(f.catalog()).unwrap(), bytes);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
        4
    );
    let next_scan = Cancellation::default();
    let second = worker::scan_folders(&f.catalog(), &app(), &[root], next_scan.token());
    assert!(
        second
            .status
            .contains("2 discovered · 0 failed · 1 unchanged failures skipped"),
        "{}",
        second.status
    );
}
#[test]
#[ignore = "explicit original signed package and native fixtures required"]
fn cancelled_folder_batch_retires_current_chain_without_publishing_pending_results() {
    let f = Fixture::new();
    let root = f.0.join("root");
    fs::create_dir(&root).unwrap();
    let first = root.join("A-hang.vst3");
    copy(&module("normal"), &first);
    let owner = Cancellation::default();
    assert!(
        worker::scan(&f.catalog(), &app(), first.clone(), owner.token())
            .catalog
            .is_some()
    );
    let before = fs::read(f.catalog()).unwrap();
    copy(&module("entry-hang"), &first);
    copy(&module("factory2-effect"), &root.join("B-not-started.vst3"));
    // The original fixture only writes this sibling marker when explicitly
    // requested; the signed bundle itself remains untouched.
    let request = root.join("A-hang.vst3.entry-wait");
    let entered = root.join("A-hang.vst3.entry-ready");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(request)
        .unwrap();
    let owner = Cancellation::default();
    let token = owner.token();
    let catalog = f.catalog();
    let package = app();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        let result = worker::scan_folders(&catalog, &package, &[root], token);
        let _ = tx.send(result);
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        match fs::read(&entered) {
            Ok(bytes) if bytes == b"entry-hang\n" => break,
            Ok(_) => {} // The fixture may still be writing the marker.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                f.1.store(true, Ordering::Release);
                let _ = owner.cancel();
                panic!(
                    "entry marker read failed: {error}; fixture retained at {:?}",
                    f.0
                );
            }
        }
        match rx.try_recv() {
            Ok(early) => {
                f.1.store(true, Ordering::Release);
                thread.join().unwrap();
                panic!("folder scan ended before plugin entry: {}", early.status);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                f.1.store(true, Ordering::Release);
                thread.join().unwrap();
                panic!("folder scan disconnected before plugin entry");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if std::time::Instant::now() >= deadline {
            f.1.store(true, Ordering::Release);
            let _ = owner.cancel();
            panic!(
                "plugin entry not observed; cancellation requested; fixture retained at {:?}",
                f.0
            );
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    if !owner.has_started_scanner() || !owner.cancel() {
        f.1.store(true, Ordering::Release);
        let _ = owner.cancel();
        panic!(
            "scanner cancellation was not admitted; fixture retained at {:?}",
            f.0
        );
    }
    let result = match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(value) => value,
        Err(error) => {
            f.1.store(true, Ordering::Release);
            panic!(
                "owned folder scanner unresolved: {error}; fixture retained at {:?}",
                f.0
            );
        }
    };
    thread.join().unwrap();
    // Preserve this isolated fixture if the scanner's final custody cannot be
    // proven, even when the worker itself has returned.
    f.1.store(true, Ordering::Release);
    assert!(result.catalog.is_none());
    assert!(result.status.contains("cancelled"), "{}", result.status);
    let diagnostics = result.diagnostics.as_ref().expect("scanner diagnostics");
    assert_eq!(diagnostics.module, first);
    assert!(diagnostics.scanner_started && diagnostics.scanner_exit.is_some());
    let retired = diagnostics.native_cleanup.as_ref().is_some_and(|cleanup| {
        cleanup.direct_reaped
            && !cleanup.pending
            && !cleanup.custody_lost
            && !cleanup.cleanup_unconfirmed
    });
    if !retired {
        panic!(
            "scanner retirement not established; fixture retained at {:?}; {diagnostics:?}",
            f.0
        );
    }
    f.1.store(false, Ordering::Release);
    assert_eq!(
        fs::read(f.catalog()).unwrap(),
        before,
        "cancelled batch never promotes a partial catalog"
    );
}
