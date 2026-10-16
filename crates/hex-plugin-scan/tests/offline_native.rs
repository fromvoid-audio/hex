#![cfg(target_os = "linux")]
//! Must explicitly run --ignored with an independently built controlled receipt.
use hex_plugin_scan::offline_protocol::{Block, Point, Request};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn receipt() -> PathBuf {
    PathBuf::from(
        std::env::var_os("HEX_OFFLINE_FIXTURE_RECEIPT")
            .expect("explicit pinned offline fixture receipt"),
    )
}
fn request() -> Request {
    Request {
        version: 1,
        request: 7,
        sample_rate: 48000,
        start_frame: 0,
        controller_value: 0.,
        component_state: vec![],
        controller_state: vec![],
        blocks: vec![Block {
            left: vec![0.125; 8],
            right: vec![-0.25; 8],
            points: vec![],
        }],
    }
}
fn run(name: &str, q: &Request, deadline: u64, cancel: bool) -> (bool, serde_json::Value) {
    let folder = std::env::temp_dir().join(format!(
        "hex-offline-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    std::fs::create_dir_all(&folder).unwrap();
    let input = folder.join("request.json");
    std::fs::write(&input, serde_json::to_vec(q).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hex-vst3-offline"))
        .arg(receipt())
        .arg(name)
        .arg(&input)
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
            "report{e} stderr{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    std::fs::remove_dir_all(folder).unwrap();
    (output.status.success(), report)
}
fn clean(r: &serde_json::Value) {
    assert_eq!(r["output"]["cleanup"]["direct_reaped"], true);
    assert_eq!(r["output"]["cleanup"]["pending"], false);
    assert_eq!(r["output"]["cleanup"]["group_signals_scoped"], true);
}
fn pcm(r: &serde_json::Value, channel: &str) -> Vec<f32> {
    serde_json::from_value(r["result"][channel].clone()).unwrap()
}
fn state(r: &serde_json::Value, field: &str) -> Vec<u8> {
    serde_json::from_value(r["result"][field].clone()).unwrap()
}
#[test]
#[ignore = "requires explicit pinned SDK offline receipt"]
fn actual_unity_controller_only_and_lifecycle() {
    let (ok, r) = run("process", &request(), 2000, false);
    assert!(ok, "{r}");
    clean(&r);
    assert_eq!(pcm(&r, "left"), vec![0.125; 8]);
    assert_eq!(pcm(&r, "right"), vec![-0.25; 8]);
    let trace: Vec<String> = serde_json::from_value(r["result"]["lifecycle"].clone()).unwrap();
    let order = [
        "initialize-component",
        "initialize-controller",
        "connect-component",
        "connect-controller",
        "setup-processing",
        "activate",
        "start-processing",
        "process",
        "stop-processing",
        "deactivate",
        "clear-handler",
        "disconnect-component",
        "disconnect-controller",
        "terminate-component",
        "terminate-controller",
        "release-component",
        "release-controller",
        "release-factory",
        "module-exit",
        "module-unload",
    ];
    let mut prior = 0;
    for (i, name) in order.iter().enumerate() {
        let at = trace.iter().position(|s| s == name).unwrap();
        assert!(i == 0 || at > prior, "{name}");
        prior = at;
    }
    assert_eq!(r["result"]["callbacks"][0], "begin-edit");
    assert!(
        r["output"]["stderr"]
            .as_str()
            .unwrap()
            .contains("offline module exit objects=0 factory=0")
    );
}
// Independent analytic oracle: prescribed global linear envelope, not the SDK
// fixture's queue-consumption implementation. Every partition supplies its own
// endpoint; values below are exact dyadic numbers.
#[test]
#[ignore = "requires explicit pinned SDK offline receipt"]
fn actual_ramp_partition_oracle_and_boundary_jump() {
    for sizes in [vec![16], vec![4, 4, 4, 4], vec![1; 16], vec![3, 5, 8]] {
        let mut q = request();
        q.blocks.clear();
        let mut start = 0;
        for n in sizes {
            q.blocks.push(Block {
                left: vec![0.125; n],
                right: vec![-0.25; n],
                points: vec![Point {
                    offset: n as u32 - 1,
                    value: 0.5 + (start + n) as f64 / 32.,
                }],
            });
            start += n;
        }
        let (ok, r) = run("process", &q, 2000, false);
        assert!(ok, "{r}");
        let expected = (1..=16)
            .map(|i| (0.125f64 * (1. + i as f64 / 16.)) as f32)
            .collect::<Vec<_>>();
        assert_eq!(pcm(&r, "left"), expected);
        assert_eq!(
            pcm(&r, "right"),
            expected.iter().map(|v| -2. * v).collect::<Vec<_>>()
        );
    }
    let mut q = request();
    q.blocks[0].points = vec![
        Point {
            offset: 2,
            value: 0.5,
        },
        Point {
            offset: 3,
            value: 0.,
        },
        Point {
            offset: 6,
            value: 0.,
        },
        Point {
            offset: 7,
            value: 1.,
        },
    ];
    let (ok, r) = run("process", &q, 2000, false);
    assert!(ok, "{r}");
    assert_eq!(
        pcm(&r, "left"),
        vec![0.125, 0.125, 0.125, 0., 0., 0., 0., 0.25]
    );
}
#[test]
#[ignore = "requires explicit pinned SDK offline receipt"]
fn actual_state_restoration_effect_and_controller_separation() {
    let mut q = request();
    q.blocks[0].points = vec![Point {
        offset: 0,
        value: 0.25,
    }];
    let (ok, r) = run("process", &q, 2000, false);
    assert!(ok, "{r}");
    assert_eq!(pcm(&r, "left"), vec![0.0625; 8]);
    let mut next = request();
    next.start_frame = 8;
    next.component_state = state(&r, "component_state");
    next.controller_state = state(&r, "controller_state");
    next.controller_state[4] = 19;
    let (ok, restored) = run("process", &next, 2000, false);
    assert!(ok, "{restored}");
    assert_eq!(pcm(&restored, "left"), vec![0.0625; 8]);
    assert_eq!(restored["result"]["controller_state"][4], 19);
    assert_eq!(
        &state(&restored, "component_state")[20..],
        &16u64.to_le_bytes()
    );
    next.component_state.clear();
    next.start_frame = 0;
    let (ok, controller_only) = run("process", &next, 2000, false);
    assert!(ok);
    assert_eq!(pcm(&controller_only, "left"), vec![0.125; 8]);
}
#[test]
#[ignore = "requires explicit pinned SDK offline receipt"]
fn actual_nonfinite_and_late_failure_discard_whole_attempt() {
    let mut overflow = request();
    overflow.blocks[0].left.fill(f32::MAX);
    overflow.blocks[0].points = vec![Point {
        offset: 0,
        value: 1.0,
    }];
    let (ok, refused) = run("process", &overflow, 2000, false);
    assert!(!ok);
    clean(&refused);
    assert!(refused["result"].is_null());
    assert!(
        refused["output"]["stderr"]
            .as_str()
            .unwrap()
            .contains("nonfinite output refused")
    );
    let mut q = request();
    q.blocks.push(q.blocks[0].clone());
    for name in ["process-nonfinite", "process-error"] {
        let (ok, r) = run(name, &q, 2000, false);
        assert!(!ok, "{r}");
        clean(&r);
        assert!(r["result"].is_null());
        assert!(
            r["output"]["stderr"]
                .as_str()
                .unwrap()
                .contains("offline module exit objects=0 factory=0")
        );
    }
}
#[test]
#[ignore = "requires explicit pinned SDK offline receipt"]
fn actual_initialize_controller_partial_connect_bus_and_restart_refusals() {
    for name in [
        "initialize-fail",
        "controller-fail",
        "connect-fail",
        "bus-refuse",
        "restart",
    ] {
        let (ok, r) = run(name, &request(), 2000, false);
        assert!(!ok, "{name}{r}");
        clean(&r);
        let logs = r["output"]["stderr"].as_str().unwrap();
        assert!(
            logs.contains("offline module exit objects=0 factory=0"),
            "{logs}"
        );
        if name == "initialize-fail" {
            assert!(!logs.contains("component terminated"));
        }
        if name == "controller-fail" {
            assert!(!logs.contains("controller terminated"));
            assert!(logs.contains("component terminated"));
        }
    }
}
#[test]
#[ignore = "requires explicit pinned SDK offline receipt"]
fn actual_malformed_state_and_restore_failure() {
    let mut q = request();
    q.component_state = vec![0; 28];
    let (ok, r) = run("process", &q, 2000, false);
    assert!(!ok);
    clean(&r);
    let (_, valid) = run("process", &request(), 2000, false);
    q.component_state = state(&valid, "component_state");
    q.component_state.push(0);
    let (ok, trailing) = run("process", &q, 2000, false);
    assert!(!ok);
    clean(&trailing);
    q.component_state.pop();
    let mut bad_controller = request();
    bad_controller.controller_state = state(&valid, "controller_state");
    bad_controller.controller_state.push(0);
    let (ok, trailing_controller) = run("process", &bad_controller, 2000, false);
    assert!(!ok);
    clean(&trailing_controller);
    let (ok, r) = run("state-fail", &q, 2000, false);
    assert!(!ok);
    clean(&r);
}
#[test]
#[ignore = "requires explicit pinned SDK offline receipt"]
fn actual_process_crash_hang_cancel_and_unload_hang_cleanup() {
    for (name, cancel) in [
        ("process-crash", false),
        ("process-hang", false),
        ("process-hang", true),
        ("unload-hang", false),
    ] {
        let start = Instant::now();
        let (ok, r) = run(name, &request(), 120, cancel);
        assert!(!ok, "{r}");
        clean(&r);
        assert!(r["result"].is_null());
        assert!(start.elapsed() < Duration::from_secs(3));
        if cancel {
            assert_eq!(r["error"], "cancelled");
        }
    }
}
#[test]
#[ignore = "requires explicit pinned SDK offline receipt"]
fn actual_processing_descendant_reaped_kernel_ledger() {
    let (ok, r) = run("process-descendant", &request(), 2000, false);
    assert!(ok, "{r}");
    clean(&r);
    let pids = r["output"]["cleanup"]["descendant_pids"]
        .as_array()
        .unwrap();
    assert_eq!(pids.len(), 1);
    for pid in pids {
        let pid = rustix::process::Pid::from_raw(pid.as_i64().unwrap() as i32).unwrap();
        assert_eq!(
            rustix::process::test_kill_process(pid),
            Err(rustix::io::Errno::SRCH)
        );
    }
}
#[test]
#[ignore = "requires explicit pinned SDK offline receipt"]
fn actual_capacity_preflight_and_arbitrary_module_refuse_before_spawn() {
    let mut q = request();
    q.blocks = vec![q.blocks[0].clone(); 33];
    let (ok, r) = run("process", &q, 2000, false);
    assert!(!ok);
    assert!(r["output"].is_null());
    let (ok, r) = run("installed-plugin", &request(), 2000, false);
    assert!(!ok);
    assert!(r["output"].is_null());
}
#[test]
#[ignore = "requires explicit pinned SDK offline receipt"]
fn actual_changed_request_refuses_after_owned_cleanup() {
    use std::io::{BufRead, BufReader};
    let folder = std::env::temp_dir().join(format!("hex-offline-change-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let input = folder.join("request.json");
    std::fs::write(&input, serde_json::to_vec(&request()).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hex-vst3-offline"))
        .arg(receipt())
        .arg("process-hang")
        .arg(&input)
        .arg("200")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut notice = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut notice)
        .unwrap();
    assert!(notice.contains("controlled helper started"));
    std::fs::OpenOptions::new()
        .append(true)
        .open(&input)
        .unwrap()
        .write_all(b" \n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let r: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        r["error"]
            .as_str()
            .unwrap()
            .contains("changed during processing")
    );
    clean(&r);
    assert!(r["result"].is_null());
    std::fs::remove_dir_all(folder).unwrap();
}
#[test]
#[ignore = "requires explicit pinned SDK offline receipt"]
fn actual_maximum_admitted_block_point_frame_capacities() {
    let mut q = request();
    q.blocks = vec![
        Block {
            left: vec![0.125; 1024],
            right: vec![-0.25; 1024],
            points: (0..128)
                .map(|offset| Point { offset, value: 0.5 })
                .collect()
        };
        8
    ];
    let (ok, r) = run("process", &q, 2000, false);
    assert!(ok, "{r}");
    clean(&r);
    assert_eq!(pcm(&r, "left"), vec![0.125; 8192]);
    let mut q = request();
    q.blocks = vec![
        Block {
            left: vec![0.125; 256],
            right: vec![-0.25; 256],
            points: vec![]
        };
        32
    ];
    let (ok, r) = run("process", &q, 2000, false);
    assert!(ok, "{r}");
    assert_eq!(pcm(&r, "left").len(), 8192);
}
#[test]
#[ignore = "requires explicit pinned SDK offline receipt"]
fn native_independently_refuses_wire_before_module_entry() {
    use std::os::{
        fd::OwnedFd,
        unix::{net::UnixStream, process::CommandExt},
    };
    let root = receipt().parent().unwrap().to_path_buf();
    let good = request().encode().unwrap();
    let mut cases = vec![];
    for at in [0, 8, 12, 14, 22, 38] {
        let mut bytes = good.clone();
        bytes[at] ^= 0xff;
        cases.push(bytes);
    }
    let mut bad = good.clone();
    bad[50..58].copy_from_slice(&f64::NAN.to_bits().to_le_bytes());
    cases.push(bad);
    let mut bad = good.clone();
    bad[66..70].copy_from_slice(&33u32.to_le_bytes());
    cases.push(bad);
    let mut bad = good.clone();
    bad[78..82].copy_from_slice(&f32::INFINITY.to_bits().to_le_bytes());
    cases.push(bad);
    let mut bad = good.clone();
    bad.push(0);
    cases.push(bad);
    for bytes in cases {
        let (mut socket, peer) = UnixStream::pair().unwrap();
        let mut child = Command::new(root.join("hex-vst3-offline-helper"))
            .arg(root.join("process.vst3/Contents/x86_64-linux/process.so"))
            .arg("7")
            .process_group(0)
            .stdin(Stdio::from(OwnedFd::from(peer)))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = socket.write_all(&bytes);
        socket.shutdown(std::net::Shutdown::Write).unwrap();
        let start = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if start.elapsed() > Duration::from_secs(2) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("native preflight hung");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty(), "module entry must not run");
        assert!(String::from_utf8_lossy(&output.stderr).contains("offline helper:"));
    }
}
