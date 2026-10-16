#![cfg(target_os = "linux")]
//! Explicit ORIGINAL CLAP native fixtures; ignored unless independently built receipt supplied.
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn receipt() -> PathBuf {
    PathBuf::from(
        std::env::var_os("HEX_CLAP_FIXTURE_RECEIPT")
            .expect("explicit pinned original CLAP receipt required"),
    )
}
fn scan(
    name: &str,
    deadline: u64,
    cancel: bool,
    previous: Option<&std::path::Path>,
) -> (bool, serde_json::Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hex-clap-scan"));
    command.arg(receipt()).arg(name).arg(deadline.to_string());
    if let Some(p) = previous {
        command.arg(p);
    }
    let mut child = command
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
    let out = child.wait_with_output().unwrap();
    let r = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("report{e}; stderr{}", String::from_utf8_lossy(&out.stderr)));
    (out.status.success(), r)
}
fn clean(r: &serde_json::Value) {
    assert_eq!(r["output"]["cleanup"]["direct_reaped"], true);
    assert_eq!(r["output"]["cleanup"]["pending"], false);
    assert_eq!(r["output"]["cleanup"]["group_signals_scoped"], true);
}
#[test]
#[ignore = "requires explicit pinned original CLAP receipt"]
fn actual_descriptor_enum_deinit_unload_without_instance() {
    let (ok, r) = scan("good", 2000, false, None);
    assert!(ok, "{r}");
    clean(&r);
    let d = &r["catalog"]["last_valid"]["discovery"];
    assert_eq!(d["api"], serde_json::json!([1, 2, 10]));
    assert_eq!(d["descriptors"][0]["id"], "org.hex.controlled.gain");
    assert_eq!(
        d["descriptors"][0]["name"],
        "original \"descriptor\" fixture"
    );
    assert_eq!(d["descriptors"][0]["features"][2], "hex-original-unknown");
    let stderr = r["output"]["stderr"].as_str().unwrap();
    assert!(stderr.contains("fixture init count=1"));
    assert!(stderr.contains("fixture deinit count=0"));
    assert!(stderr.contains("fixture unload initialized=0"));
    assert!(!stderr.contains("FORBIDDEN create_plugin"));
    assert!(
        r["output"]["stdout"]
            .as_str()
            .unwrap()
            .contains("never IPC")
    );
}
#[test]
#[ignore = "requires explicit pinned original CLAP receipt"]
fn actual_failed_init_missing_factory_and_api_refuse_correct_pairs() {
    for name in ["init-fail", "no-factory", "api-bad"] {
        let (ok, r) = scan(name, 2000, false, None);
        assert!(!ok, "{r}");
        clean(&r);
        let logs = r["output"]["stderr"].as_str().unwrap();
        assert!(logs.contains("fixture unload initialized=0"));
        assert_eq!(
            logs.contains("fixture deinit count=0"),
            name == "no-factory"
        );
        assert!(r["catalog"]["last_valid"].is_null());
    }
}
#[test]
#[ignore = "requires explicit pinned original CLAP receipt"]
fn actual_duplicate_malformed_utf8_count_and_feature_refuse_after_cleanup() {
    for name in [
        "duplicate",
        "malformed",
        "bad-utf8",
        "too-many",
        "unclosed-features",
    ] {
        let (ok, r) = scan(name, 2000, false, None);
        assert!(!ok, "{name}{r}");
        clean(&r);
        let logs = r["output"]["stderr"].as_str().unwrap();
        assert!(logs.contains("fixture deinit count=0"), "{logs}");
        assert!(logs.contains("fixture unload initialized=0"));
        assert!(r["catalog"]["last_valid"].is_null());
    }
}
#[test]
#[ignore = "requires explicit pinned original CLAP receipt"]
fn actual_entry_crash_deadline_cancel_and_deinit_hang_are_reaped() {
    for (name, cancel) in [
        ("crash", false),
        ("hang", false),
        ("hang", true),
        ("deinit-hang", false),
    ] {
        let start = Instant::now();
        let (ok, r) = scan(name, 120, cancel, None);
        assert!(!ok, "{r}");
        clean(&r);
        assert!(r["catalog"]["last_valid"].is_null());
        assert!(start.elapsed() < Duration::from_secs(3));
        if cancel {
            assert_eq!(r["catalog"]["last_error"], "cancelled");
        }
    }
}
#[test]
#[ignore = "requires explicit pinned original CLAP receipt"]
fn actual_descendant_reaped_kernel_ledger() {
    let (ok, r) = scan("descendant", 2000, false, None);
    assert!(ok, "{r}");
    clean(&r);
    let ids = r["output"]["cleanup"]["descendant_pids"]
        .as_array()
        .unwrap();
    assert_eq!(ids.len(), 1);
    for p in ids {
        let pid = rustix::process::Pid::from_raw(p.as_i64().unwrap() as i32).unwrap();
        assert_eq!(
            rustix::process::test_kill_process(pid),
            Err(rustix::io::Errno::SRCH)
        );
    }
}
#[test]
#[ignore = "requires explicit pinned original CLAP receipt"]
fn actual_failure_retains_previous_valid_discovery() {
    let (ok, old) = scan("good", 2000, false, None);
    assert!(ok);
    let p = std::env::temp_dir().join(format!("hex-clap-prior-{}.json", std::process::id()));
    std::fs::write(&p, serde_json::to_vec(&old).unwrap()).unwrap();
    let (ok, r) = scan("crash", 1000, false, Some(&p));
    assert!(!ok);
    clean(&r);
    assert_eq!(r["catalog"]["last_valid"], old["catalog"]["last_valid"]);
    assert!(r["catalog"]["last_error"].is_string());
    std::fs::remove_file(p).unwrap();
}
#[test]
#[ignore = "requires explicit pinned original CLAP receipt"]
fn arbitrary_installed_module_names_refuse_before_child() {
    let (ok, r) = scan("installed-third-party", 1000, false, None);
    assert!(!ok);
    assert!(r["output"].is_null());
    assert!(
        r["catalog"]["last_error"]
            .as_str()
            .unwrap()
            .contains("original controlled fixture")
    );
}
#[test]
#[ignore = "requires explicit pinned original CLAP receipt"]
fn changed_receipt_after_real_spawn_refuses_after_cleanup() {
    let source = receipt();
    let root = std::env::temp_dir().join(format!("hex-clap-stale-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("controlled-clap-fixtures.json");
    std::fs::copy(&source, &target).unwrap();
    for name in ["hex-clap-probe", "hang.clap"] {
        std::fs::copy(source.parent().unwrap().join(name), root.join(name)).unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_hex-clap-scan"))
        .arg(&target)
        .arg("hang")
        .arg("200")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(line.contains("controlled helper started"));
    std::fs::OpenOptions::new()
        .append(true)
        .open(&target)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    clean(&r);
    assert!(
        r["catalog"]["last_error"]
            .as_str()
            .unwrap()
            .contains("receipt changed")
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
#[ignore = "requires explicit pinned original CLAP receipt"]
fn inherited_auto_reap_negative_control_and_owned_cleanup_repair() {
    let launcher = receipt().parent().unwrap().join("launch-ignored");
    let negative = Command::new(&launcher)
        .arg("--negative-control")
        .output()
        .unwrap();
    assert!(negative.status.success());
    assert!(
        String::from_utf8_lossy(&negative.stdout)
            .contains("inherited auto-reap negative control confirmed")
    );
    let out = Command::new(&launcher)
        .arg(env!("CARGO_BIN_EXE_hex-clap-scan"))
        .arg(receipt())
        .arg("descendant")
        .arg("2000")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    clean(&r);
    assert_eq!(r["output"]["cleanup"]["descendants_reaped"], 1);
}
#[test]
#[ignore = "requires explicit pinned original CLAP receipt"]
fn real_module_digest_change_refuses_before_any_child() {
    let source = receipt();
    let root = std::env::temp_dir().join(format!("hex-clap-digest-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("controlled-clap-fixtures.json");
    std::fs::copy(&source, &target).unwrap();
    for name in ["hex-clap-probe", "good.clap"] {
        std::fs::copy(source.parent().unwrap().join(name), root.join(name)).unwrap();
    }
    std::fs::OpenOptions::new()
        .append(true)
        .open(root.join("good.clap"))
        .unwrap()
        .write_all(b"changed")
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_hex-clap-scan"))
        .arg(&target)
        .arg("good")
        .arg("2000")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(r["output"].is_null());
    assert!(
        r["catalog"]["last_error"]
            .as_str()
            .unwrap()
            .contains("identity changed")
    );
    std::fs::remove_dir_all(root).unwrap();
}
