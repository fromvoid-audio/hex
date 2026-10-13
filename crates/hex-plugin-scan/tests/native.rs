#![cfg(target_os = "linux")]
//! Explicit original SDK Linux fixtures. Always ignored by default: run this
//! named suite with --ignored and the independently built receipt for evidence.
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn receipt() -> PathBuf {
    PathBuf::from(
        std::env::var_os("HEX_SCAN_FIXTURE_RECEIPT")
            .expect("explicit controlled SDK receipt required"),
    )
}
fn scan(name: &str, deadline: u64, cancel: bool) -> (bool, serde_json::Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hex-plugin-scan"))
        .arg(receipt())
        .arg(name)
        .arg(deadline.to_string())
        .stdin(if cancel {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if cancel {
        std::thread::sleep(Duration::from_millis(40));
        child.stdin.take().unwrap().write_all(b"cancel\n").unwrap();
    }
    let output = child.wait_with_output().unwrap();
    let report = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "report {e}; stderr {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), report)
}
fn clean(report: &serde_json::Value) {
    assert_eq!(report["output"]["cleanup"]["direct_reaped"], true);
    assert_eq!(report["output"]["cleanup"]["pending"], false);
    assert_eq!(report["output"]["cleanup"]["group_signals_scoped"], true);
}
#[test]
#[ignore = "requires explicit pinned SDK controlled receipt"]
fn actual_factory_enumeration_unload_and_diagnostic_separation() {
    let (ok, r) = scan("good", 2000, false);
    assert!(ok, "{r}");
    clean(&r);
    assert_eq!(
        r["catalog"]["last_valid"]["discovery"]["classes"][0]["name"],
        "factory only \"fixture\""
    );
    assert!(
        r["output"]["stdout"]
            .as_str()
            .unwrap()
            .contains("never metadata")
    );
    assert!(
        r["output"]["stderr"]
            .as_str()
            .unwrap()
            .contains("module exit refs=0")
    );
    assert!(
        !r["output"]["stderr"]
            .as_str()
            .unwrap()
            .contains("FORBIDDEN")
    );
    assert_eq!(r["output"]["packet"], serde_json::json!([]));
}
#[test]
#[ignore = "requires explicit pinned SDK controlled receipt"]
fn actual_module_entry_crash_is_contained_and_reaped() {
    let (ok, r) = scan("crash", 2000, false);
    assert!(!ok);
    clean(&r);
    assert_eq!(
        r["catalog"]["last_error"],
        "native helper failed or crashed"
    );
}
#[test]
#[ignore = "requires explicit pinned SDK controlled receipt"]
fn actual_module_entry_hang_deadline_is_terminated_and_reaped() {
    let start = Instant::now();
    let (ok, r) = scan("hang", 80, false);
    assert!(!ok);
    clean(&r);
    assert_eq!(r["catalog"]["last_error"], "deadline exceeded");
    assert!(start.elapsed() < Duration::from_secs(3));
}
#[test]
#[ignore = "requires explicit pinned SDK controlled receipt"]
fn actual_hang_cancellation_is_terminated_and_reaped() {
    let (ok, r) = scan("hang", 5000, true);
    assert!(!ok);
    clean(&r);
    assert_eq!(r["catalog"]["last_error"], "cancelled");
}
#[test]
#[ignore = "requires explicit pinned SDK controlled receipt"]
fn actual_descendant_holding_pipes_is_killed_and_subreaped() {
    let (ok, r) = scan("descendant", 2000, false);
    assert!(ok, "{r}");
    clean(&r);
    assert_eq!(r["output"]["cleanup"]["descendants_reaped"], 1);
    let pid = r["output"]["stderr"]
        .as_str()
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("fixture descendant="))
        .unwrap();
    let pid: i32 = pid.parse().unwrap();
    assert_eq!(
        r["output"]["cleanup"]["descendant_pids"],
        serde_json::json!([pid])
    );
    assert_eq!(
        rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap()),
        Err(rustix::io::Errno::SRCH)
    );
}
#[test]
#[ignore = "requires explicit pinned SDK controlled receipt"]
fn actual_discardable_classes_are_explicitly_noncacheable() {
    let (ok, r) = scan("discardable", 2000, false);
    assert!(ok);
    clean(&r);
    let discovery: hex_plugin_scan::Discovery =
        serde_json::from_value(r["catalog"]["last_valid"]["discovery"].clone()).unwrap();
    assert!(!discovery.cacheable());
    assert_eq!(discovery.classes.len(), 1);
}
#[test]
#[ignore = "requires explicit pinned SDK controlled receipt"]
fn arbitrary_installed_directory_names_are_refused_without_a_child() {
    let (ok, r) = scan("/Library/Audio/Plug-Ins/VST3", 2000, false);
    assert!(!ok);
    assert!(r["output"].is_null());
    assert_eq!(
        r["catalog"]["last_error"],
        "only original controlled fixture names allowed"
    );
}
fn copy_fixtures(label: &str) -> (PathBuf, PathBuf) {
    let original = receipt();
    let root = original.parent().unwrap();
    let dir = std::env::temp_dir().join(format!("hex-scan-{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let parsed = hex_plugin_scan::Receipt::load(&original).unwrap();
    std::fs::copy(&original, dir.join("controlled-fixtures.json")).unwrap();
    std::fs::copy(root.join("hex-vst3-probe"), dir.join("hex-vst3-probe")).unwrap();
    for m in parsed.modules {
        let path = dir.join(&m.binary);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(root.join(m.binary), path).unwrap();
    }
    let receipt = dir.join("controlled-fixtures.json");
    (dir, receipt)
}
#[test]
#[ignore = "requires explicit pinned SDK controlled receipt"]
fn actual_failed_scan_preserves_previous_valid_report() {
    let (_, good) = scan("good", 2000, false);
    let (dir, receipt) = copy_fixtures("prior");
    let previous = dir.join("previous.json");
    std::fs::write(&previous, serde_json::to_vec(&good).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hex-plugin-scan"))
        .args([
            receipt.as_os_str(),
            std::ffi::OsStr::new("crash"),
            std::ffi::OsStr::new("2000"),
            previous.as_os_str(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let failed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    clean(&failed);
    assert_eq!(
        failed["catalog"]["last_valid"],
        good["catalog"]["last_valid"]
    );
    assert!(!failed["catalog"]["last_error"].is_null());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
#[ignore = "requires explicit pinned SDK controlled receipt"]
fn changed_receipt_after_real_native_spawn_refuses_after_cleanup() {
    let (dir, receipt) = copy_fixtures("changed");
    let mut child = Command::new(env!("CARGO_BIN_EXE_hex-plugin-scan"))
        .arg(&receipt)
        .args(["hang", "1000"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut notice = String::new();
    let mut reader = BufReader::new(child.stderr.take().unwrap());
    assert!(
        reader.read_line(&mut notice).unwrap() > 0,
        "supervisor did not admit child"
    );
    assert_eq!(notice.trim(), "scanner: controlled helper started");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&receipt)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let r: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    clean(&r);
    assert_eq!(
        r["catalog"]["last_error"],
        "controlled receipt changed during probe"
    );
    assert!(r["catalog"]["last_valid"].is_null());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
#[ignore = "requires explicit pinned SDK controlled receipt"]
fn inherited_auto_reap_negative_control_and_waitable_child_repair() {
    let path = receipt();
    let root = path.parent().unwrap();
    let manifest = hex_plugin_scan::Receipt::load(&path).unwrap();
    let launcher = hex_plugin_scan::controlled_path(root, "launch-ignored").unwrap();
    hex_plugin_scan::binary_identity(&launcher, &manifest.launcher_sha256).unwrap();
    let negative = Command::new(&launcher)
        .arg("--negative-control")
        .output()
        .unwrap();
    assert!(negative.status.success());
    assert!(
        String::from_utf8_lossy(&negative.stdout).contains("auto-reap negative control confirmed")
    );
    let output = Command::new(&launcher)
        .arg(env!("CARGO_BIN_EXE_hex-plugin-scan"))
        .arg(&path)
        .args(["descendant", "2000"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{report}");
    clean(&report);
    assert_eq!(report["output"]["cleanup"]["descendants_reaped"], 1);
}
