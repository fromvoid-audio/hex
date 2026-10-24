#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
//! Actual original Mac fixtures only. Explicit receipt required; never installed plugins.
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

fn receipt() -> PathBuf {
    PathBuf::from(
        std::env::var_os("HEX_VST3_MACOS_RECEIPT").expect("explicit native Mac receipt required"),
    )
    .canonicalize()
    .expect("existing original receipt")
}

fn fixture(receipt: &Path, name: &str) -> PathBuf {
    receipt
        .parent()
        .expect("fixture root")
        .join("hex-vst3-host.app/Contents/Resources/factory-fixtures")
        .join(format!("{name}.vst3"))
}
fn command(receipt: &Path, module: &Path, deadline: u64) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hex-vst3-scan-macos"));
    command
        .arg(receipt)
        .arg("--trust-user-code")
        .arg(module)
        .arg(deadline.to_string());
    command
}
fn report(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "report decode {error}: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
fn refused(output: &Output) -> serde_json::Value {
    assert!(!output.status.success());
    let value = report(output);
    assert!(value["discovery"].is_null(), "{value}");
    assert!(!value["error"].is_null(), "{value}");
    assert_eq!(value["cleanup"]["direct_reaped"], true, "{value}");
    assert_eq!(value["cleanup"]["pending"], false, "{value}");
    assert_eq!(value["cleanup"]["descendants"], "unknown");
    value
}

static TEMP_ID: AtomicU64 = AtomicU64::new(0);
struct OwnedCopy(PathBuf);
struct Sentinel(std::process::Child);
impl Drop for Sentinel {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
impl Drop for OwnedCopy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).expect("fresh owned directory");
    for entry in fs::read_dir(source).expect("original fixture directory") {
        let entry = entry.expect("fixture entry");
        let kind = entry.file_type().expect("fixture type");
        assert!(!kind.is_symlink(), "original fixture tree is nonlinked");
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("owned fixture copy");
        }
    }
}
fn copy_build(original: &Path) -> (OwnedCopy, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "hex-mac-factory-{}-{}",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).expect("unique owned test root");
    copy_tree(
        &original
            .parent()
            .expect("fixture root")
            .join("hex-vst3-host.app"),
        &root.join("hex-vst3-host.app"),
    );
    let target = root.join("controlled-macos.json");
    fs::copy(original, &target).expect("receipt copy");
    (OwnedCopy(root), target)
}
fn reseal_receipt(receipt: &Path) {
    let root = receipt.parent().expect("owned fixture root");
    let output = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-", "--timestamp=none"])
        .arg(root.join("hex-vst3-host.app"))
        .output()
        .expect("owned signing");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut data: serde_json::Value =
        serde_json::from_slice(&fs::read(receipt).expect("receipt")).expect("receipt JSON");
    for (relative, digest) in data["bundle_files"]
        .as_object_mut()
        .expect("bundle file map")
    {
        *digest = serde_json::Value::String(hex_plugin_scan::hash(
            &fs::read(root.join(relative)).expect("owned member"),
        ));
    }
    fs::write(
        receipt,
        serde_json::to_vec_pretty(&data).expect("bounded receipt"),
    )
    .expect("owned receipt write");
}

#[test]
#[ignore = "explicit original Mac helper/fixture build required"]
fn real_thin_and_universal_factories_preserve_processor_and_controller_uids_without_instances() {
    let receipt = receipt();
    let mut identities = None;
    for name in ["normal", "universal"] {
        let output = command(&receipt, &fixture(&receipt, name), 10000)
            .output()
            .expect("factory scan");
        let value = report(&output);
        assert!(output.status.success(), "{value}");
        let classes = value["discovery"]["classes"]
            .as_array()
            .expect("class records");
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0]["category"], "Audio Module Class");
        assert_eq!(classes[1]["category"], "Component Controller Class");
        let ids = classes.iter().map(|c| c["cid"].clone()).collect::<Vec<_>>();
        assert_eq!(
            ids[0],
            serde_json::json!([72, 69, 88, 70, 65, 67, 84, 49, 1, 2, 3, 4, 5, 6, 7, 1])
        );
        assert_eq!(
            ids[1],
            serde_json::json!([72, 69, 88, 70, 65, 67, 84, 49, 1, 2, 3, 4, 5, 6, 7, 2])
        );
        if let Some(previous) = &identities {
            assert_eq!(&ids, previous);
        } else {
            identities = Some(ids);
        }
        assert_eq!(
            value["executable"]["image"],
            if name == "normal" {
                "thin_arm64"
            } else {
                "universal_arm64"
            }
        );
        assert!(
            !value["stderr"]
                .as_str()
                .expect("diagnostic")
                .contains("forbidden ordinary")
        );
        assert_eq!(value["cleanup"]["direct_exit_code"], 0);
    }
}

#[test]
#[ignore = "explicit original Mac helper/fixture build required"]
fn load_enumeration_and_exit_crashes_hangs_and_malformed_factories_never_publish_partial_results() {
    let receipt = receipt();
    for name in [
        "load-crash",
        "entry-crash",
        "enumerate-crash",
        "exit-crash",
        "load-hang",
        "entry-hang",
        "enumerate-hang",
        "exit-hang",
        "exit-fail",
        "malformed-count",
        "malformed-string",
        "duplicate-uid",
        "flood",
        "leave-group-hang",
    ] {
        let started = Instant::now();
        let output = command(&receipt, &fixture(&receipt, name), 1000)
            .output()
            .expect("owned failure scan");
        let value = refused(&output);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{name}: {value}"
        );
        if name == "flood" {
            assert!(value["stdout"].as_str().expect("diagnostic").len() <= 65536);
        }
        if name == "leave-group-hang" {
            assert!(value["cleanup"]["direct_kill"].is_string());
        }
        assert!(
            !value["stderr"]
                .as_str()
                .expect("diagnostic")
                .contains("forbidden ordinary")
        );
    }
}

#[test]
#[ignore = "explicit original Mac helper/fixture build required"]
fn wrong_architecture_is_refused_during_admission_and_open_inherited_channels_do_not_require_eof() {
    let receipt = receipt();
    let output = command(&receipt, &fixture(&receipt, "wrong-architecture"), 10000)
        .output()
        .expect("wrong architecture scan");
    let value = refused(&output);
    assert!(value["executable"].is_null());
    assert!(value["error"].as_str().expect("refusal").contains("arm64"));
    let started = Instant::now();
    let output = command(&receipt, &fixture(&receipt, "open-channel-child"), 10000)
        .output()
        .expect("inherited channels scan");
    let value = report(&output);
    assert!(output.status.success(), "{value}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "must not await the child's five-second channel lifetime"
    );
    assert_eq!(value["cleanup"]["descendants"], "unknown");
}

#[test]
#[ignore = "explicit original Mac helper/fixture build required"]
fn inherited_auto_reap_is_repaired_in_the_dedicated_owner_only() {
    let receipt = receipt();
    let launcher = receipt
        .parent()
        .expect("fixture root")
        .join("hex-vst3-host.app/Contents/MacOS/hex-vst3-launch-ignored");
    assert!(
        Command::new(&launcher)
            .arg("--negative-control")
            .status()
            .expect("negative launcher")
            .success()
    );
    let output = Command::new(launcher)
        .arg(env!("CARGO_BIN_EXE_hex-vst3-scan-macos"))
        .arg(&receipt)
        .arg("--trust-user-code")
        .arg(fixture(&receipt, "normal"))
        .arg("10000")
        .output()
        .expect("inherited disposition scan");
    let value = report(&output);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["cleanup"]["direct_reaped"], true);
    assert_eq!(value["cleanup"]["custody_lost"], false);
}

#[test]
#[ignore = "explicit original Mac helper/fixture build required"]
fn terminal_reply_without_exit_is_not_success_and_single_byte_cancel_preserves_unrelated_processes()
{
    let original = receipt();
    let (_owned, receipt) = copy_build(&original);
    let root = receipt.parent().expect("owned fixture root");
    let helper = root.join("hex-vst3-host.app/Contents/MacOS/hex-vst3-factory-scan");
    fs::copy(
        root.join("hex-vst3-host.app/Contents/MacOS/hex-vst3-factory-scan-reply-hang"),
        &helper,
    )
    .expect("known original fault helper");
    reseal_receipt(&receipt);
    let output = command(&receipt, &fixture(&receipt, "normal"), 1000)
        .output()
        .expect("reply without exit");
    let value = refused(&output);
    assert_eq!(value["interruption"], "deadline exceeded", "{value}");

    let mut sentinel = Sentinel(
        Command::new("/bin/sleep")
            .arg("10")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("owned unrelated sentinel"),
    );
    let mut child = command(&receipt, &fixture(&receipt, "normal"), 10000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cancel scan");
    let mut reader = BufReader::new(child.stderr.take().expect("progress pipe"));
    let mut line = String::new();
    loop {
        line.clear();
        assert_ne!(
            reader.read_line(&mut line).expect("progress read"),
            0,
            "terminal progress missing"
        );
        if line.contains("terminal result received") {
            break;
        }
    }
    child
        .stdin
        .as_mut()
        .expect("cancel pipe")
        .write_all(b"c")
        .expect("single cancellation byte");
    drop(reader);
    let output = child.wait_with_output().expect("cancel result");
    let value = refused(&output);
    assert_eq!(value["interruption"], "cancelled");
    assert!(
        sentinel
            .0
            .try_wait()
            .expect("sentinel observation")
            .is_none(),
        "unrelated process survived"
    );
    sentinel.0.kill().expect("owned sentinel stop");
    sentinel.0.wait().expect("owned sentinel reap");
}

#[test]
#[ignore = "explicit original Mac helper/fixture build required"]
fn real_factory_result_requires_normal_direct_reap_with_truthful_group_outcomes() {
    let receipt = receipt();
    let module = receipt
        .parent()
        .expect("fixture root")
        .join("hex-vst3-host.app/Contents/Resources/fixtures/effect.vst3");
    let output = Command::new(env!("CARGO_BIN_EXE_hex-vst3-scan-macos"))
        .arg(&receipt)
        .arg("--trust-user-code")
        .arg(&module)
        .arg("10000")
        .output()
        .expect("owned scanner command");
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("bounded scanner report");
    assert!(
        output.status.success(),
        "{}; {}",
        String::from_utf8_lossy(&output.stderr),
        report
    );
    assert!(report["error"].is_null());
    assert_eq!(report["cleanup"]["direct_reaped"], true);
    assert_eq!(report["cleanup"]["direct_exit_code"], 0);
    assert_eq!(report["cleanup"]["descendants"], "unknown");
    assert_eq!(report["cleanup"]["pending"], false);
    assert_eq!(
        report["discovery"]["classes"][0]["name"],
        "hex original gain effect fixture"
    );
    assert_eq!(
        report["discovery"]["classes"][0]["category"],
        "Audio Module Class"
    );
    assert_eq!(
        report["discovery"]["classes"][0]["cid"],
        serde_json::json!([72, 69, 88, 51, 1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 17, 0])
    );
}

#[test]
#[ignore = "explicit original Mac helper/fixture build required"]
fn helper_content_change_refuses_before_spawn_and_module_changes_refuse_before_loading() {
    use hex_plugin_scan::macos::{
        build_identity::Build,
        identity::Bundle,
        protocol::{self, Attempt},
    };
    let original = receipt();
    let (owned, copy) = copy_build(&original);
    let helper = owned
        .0
        .join("hex-vst3-host.app/Contents/MacOS/hex-vst3-factory-scan");
    fs::OpenOptions::new()
        .append(true)
        .open(&helper)
        .expect("owned helper")
        .write_all(b"changed")
        .expect("helper mutation");
    let output = command(&copy, &fixture(&copy, "normal"), 10000)
        .output()
        .expect("changed helper scan");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("owned helper started"));

    let build = Build::inspect(&original).expect("original trusted helper build");
    for metadata_change in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "hex-mac-input-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("unique owned module root");
        let _owned = OwnedCopy(root.clone());
        let module = root.join("selected.vst3");
        copy_tree(&fixture(&original, "load-crash"), &module);
        let bundle = Bundle::inspect(&module).expect("original selected module");
        let attempt = Attempt {
            request: 73,
            generation: 9,
        };
        let (mut channel, peer) = UnixStream::pair().expect("owned protocol");
        channel
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read deadline");
        channel
            .set_write_timeout(Some(Duration::from_secs(5)))
            .expect("write deadline");
        let child = Command::new(&build.helper)
            .stdin(Stdio::from(OwnedFd::from(peer)))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("original native helper");
        channel
            .write_all(&protocol::request(&bundle, attempt).expect("request"))
            .expect("request send");
        let mut header = [0_u8; 12];
        channel.read_exact(&mut header).expect("preflight header");
        let size = u32::from_le_bytes(header[8..12].try_into().expect("width")) as usize;
        assert!(size <= hex_plugin_scan::MAX_PACKET);
        let mut preflight = header.to_vec();
        preflight.resize(12 + size, 0);
        channel
            .read_exact(&mut preflight[12..])
            .expect("preflight body");
        let selected = protocol::preflight(&preflight, attempt, &bundle).expect("metadata only");
        let executable = bundle
            .admit(&selected.executable)
            .expect("original executable admission");
        if metadata_change {
            let path = module.join("Contents/Info.plist");
            let old = fs::read_to_string(&path).expect("original XML fixture");
            let changed = old.replace("<string>fixture</string>", "<string>redirected</string>");
            assert_ne!(old, changed);
            fs::copy(
                module.join("Contents/MacOS/fixture"),
                module.join("Contents/MacOS/redirected"),
            )
            .expect("owned alternate");
            fs::write(path, changed).expect("owned selection mutation");
        } else {
            fs::OpenOptions::new()
                .append(true)
                .open(module.join(&executable.relative_path))
                .expect("owned original binary")
                .write_all(b"changed")
                .expect("module mutation");
        }
        channel
            .write_all(
                &protocol::admission(&bundle, &executable, attempt).expect("sealed original ACK"),
            )
            .expect("ACK send");
        drop(channel);
        let output = child.wait_with_output().expect("refused native input");
        assert_eq!(
            output.status.code(),
            Some(1),
            "must refuse normally before the crash-on-load constructor runs"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("stale or changed executable admission"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
    }
}

#[test]
#[ignore = "explicit original Mac helper/fixture build required"]
fn ordinary_termination_signals_cancel_owned_hangs_without_killing_unrelated_processes() {
    let receipt = receipt();
    let mut sentinel = Sentinel(
        Command::new("/bin/sleep")
            .arg("10")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("owned unrelated sentinel"),
    );
    for signal in [rustix::process::Signal::INT, rustix::process::Signal::TERM] {
        let mut child = command(&receipt, &fixture(&receipt, "entry-hang"), 10000)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("signal cancellation scan");
        let mut reader = BufReader::new(child.stderr.take().expect("progress"));
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(reader.read_line(&mut line).expect("admission progress"), 0);
            if line.contains("admission sent") {
                break;
            }
        }
        let pid = rustix::process::Pid::from_child(&child);
        rustix::process::kill_process(pid, signal).expect("signal exact owned scanner");
        rustix::process::kill_process(pid, signal)
            .expect("repeated signal retains graceful cleanup");
        drop(reader);
        let output = child.wait_with_output().expect("graceful signal result");
        let value = refused(&output);
        assert_eq!(value["interruption"], "cancelled", "{value}");
        assert!(
            sentinel
                .0
                .try_wait()
                .expect("unrelated observation")
                .is_none()
        );
    }
    sentinel.0.kill().expect("owned sentinel stop");
    sentinel.0.wait().expect("owned sentinel reap");
}

#[test]
#[ignore = "explicit original Mac helper/fixture build required"]
fn optional_factory_metadata_has_owned_release_and_strict_class_identity() {
    let receipt = receipt();
    for (name, expected) in [
        ("normal", None),
        ("factory2-instrument", Some("Instrument|Synth")),
        ("factory2-effect", Some("Fx|Delay")),
        ("factory2-dual", Some("Fx|Instrument")),
        ("factory2-unknown", Some("MyInstrument|CustomFx")),
    ] {
        let output = command(&receipt, &fixture(&receipt, name), 10000)
            .output()
            .unwrap();
        let value = report(&output);
        assert!(output.status.success(), "{name}: {value}");
        assert_eq!(
            value["discovery"]["classes"][0]["subcategories"].as_str(),
            expected
        );
        assert_eq!(
            value["cleanup"]["direct_exit_code"], 0,
            "fixture exit checks every factory reference released"
        );
        if expected.is_some() {
            assert_eq!(
                value["discovery"]["classes"][0]["class_vendor"],
                "original extended vendor"
            );
            assert_eq!(
                value["discovery"]["classes"][0]["class_flags"],
                0x8000_0000_u32
            );
        }
        assert!(
            !value["stderr"]
                .as_str()
                .unwrap()
                .contains("forbidden ordinary")
        );
    }
    for name in [
        "factory2-mismatch",
        "factory2-malformed",
        "factory2-query-fail",
        "factory2-invalid-utf8",
        "factory2-class-query-fail",
        "factory2-null-interface",
    ] {
        refused(
            &command(&receipt, &fixture(&receipt, name), 10000)
                .output()
                .unwrap(),
        );
    }
}
