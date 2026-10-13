#![cfg(target_os = "linux")]
//! Real ORIGINAL modules only. Explicit optimized offline builder receipt required.
use hex_clap_scan::offline_protocol::{Block, Point, Request};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn receipt() -> PathBuf {
    PathBuf::from(std::env::var_os("HEX_CLAP_OFFLINE_RECEIPT").expect("explicit original receipt"))
}
fn request() -> Request {
    Request {
        version: 1,
        request: 7,
        sample_rate: 48000,
        start_frame: 0,
        initial_gain: 1.0,
        state: vec![],
        blocks: vec![Block {
            left: vec![1.0, 2.0, 3.0, 4.0],
            right: vec![-1.0, -2.0, -3.0, -4.0],
            points: vec![
                Point {
                    offset: 1,
                    value: 0.5,
                },
                Point {
                    offset: 1,
                    value: 2.0,
                },
                Point {
                    offset: 3,
                    value: 0.0,
                },
            ],
        }],
    }
}
fn run_at(
    path: &std::path::Path,
    name: &str,
    r: &Request,
    deadline: u64,
    cancel: bool,
) -> (bool, serde_json::Value) {
    let file = std::env::temp_dir().join(format!(
        "hex-clap-offline-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&file, serde_json::to_vec(r).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hex-clap-offline"))
        .arg(path)
        .arg(name)
        .arg(&file)
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
        std::thread::sleep(Duration::from_millis(50));
        child.stdin.take().unwrap().write_all(b"cancel\n").unwrap();
    }
    let output = child.wait_with_output().unwrap();
    std::fs::remove_file(file).unwrap();
    let value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e} {}", String::from_utf8_lossy(&output.stderr)));
    (output.status.success(), value)
}
fn run(name: &str, r: &Request) -> (bool, serde_json::Value) {
    run_at(&receipt(), name, r, 2000, false)
}
fn clean(r: &serde_json::Value) {
    assert_eq!(r["output"]["cleanup"]["direct_reaped"], true);
    assert_eq!(r["output"]["cleanup"]["pending"], false);
    assert_eq!(r["output"]["cleanup"]["group_signals_scoped"], true);
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_plain_step_pcm_coincident_events_and_thread_lifecycle() {
    let (ok, r) = run("process", &request());
    assert!(ok, "{r}");
    clean(&r);
    let result = &r["result"];
    assert_eq!(result["left"], serde_json::json!([1.0, 4.0, 6.0, 0.0]));
    assert_eq!(result["right"], serde_json::json!([-1.0, -4.0, -6.0, 0.0]));
    assert_eq!(result["start_frame"], 0);
    assert_eq!(result["end_frame"], 4);
    assert_eq!(result["latency"], 0);
    assert_eq!(result["parameters"][0]["min"], 0.0);
    assert_eq!(result["parameters"][0]["max"], 2.0);
    assert_eq!(result["statuses"], serde_json::json!([1]));
    let life = result["lifecycle"].as_array().unwrap();
    for label in [
        "create main",
        "init main",
        "initial gain flush inactive main",
        "activate main",
        "start audio",
        "stop audio",
        "deactivate main",
        "state save partial main",
        "destroy main",
        "entry deinit main",
        "unload main",
    ] {
        assert!(life.contains(&serde_json::json!(label)), "{life:?}");
    }
    assert_eq!(
        result["callbacks"][1],
        "request_callback during destroy retained"
    );
    assert!(
        r["output"]["stderr"]
            .as_str()
            .unwrap()
            .contains("fixture unload entries=0 instances=0")
    );
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_state_partial_transfer_continuation_and_parameter_effect() {
    let (ok, r) = run("process", &request());
    assert!(ok, "{r}");
    let mut next = request();
    next.state = serde_json::from_value(r["result"]["state"].clone()).unwrap();
    next.start_frame = 4;
    next.initial_gain = 2.0;
    next.blocks[0].points.clear();
    let (ok, s) = run("process", &next);
    assert!(ok, "{s}");
    assert_eq!(s["result"]["left"], serde_json::json!([0.0, 0.0, 0.0, 0.0]));
    next.blocks[0].points = vec![Point {
        offset: 2,
        value: 0.5,
    }];
    let (ok, s) = run("process", &next);
    assert!(ok, "{s}");
    assert_eq!(s["result"]["left"], serde_json::json!([0.0, 0.0, 1.5, 2.0]));
    assert_eq!(s["result"]["end_frame"], 8);
    let state: Vec<u8> = serde_json::from_value(s["result"]["state"].clone()).unwrap();
    assert_eq!(u64::from_le_bytes(state[12..20].try_into().unwrap()), 8);
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_independent_oracle_and_partition_continuity() {
    let mut r = request();
    r.initial_gain = 0.25;
    r.blocks = vec![Block {
        left: (0..64).map(|n| n as f32 / 8.0 - 4.0).collect(),
        right: (0..64).map(|n| 4.0 - n as f32 / 8.0).collect(),
        points: vec![
            Point {
                offset: 7,
                value: 1.5,
            },
            Point {
                offset: 23,
                value: 0.0,
            },
            Point {
                offset: 42,
                value: 2.0,
            },
        ],
    }];
    let mut gain = r.initial_gain;
    let expected: Vec<f32> = r.blocks[0]
        .left
        .iter()
        .enumerate()
        .map(|(i, v)| {
            for p in &r.blocks[0].points {
                if p.offset == i as u32 {
                    gain = p.value;
                }
            }
            (f64::from(*v) * gain) as f32
        })
        .collect();
    let (ok, a) = run("process", &r);
    assert!(ok, "{a}");
    assert_eq!(a["result"]["left"], serde_json::json!(expected));
    let original = r.blocks.remove(0);
    for (start, end) in [(0, 7), (7, 23), (23, 42), (42, 64)] {
        r.blocks.push(Block {
            left: original.left[start..end].to_vec(),
            right: original.right[start..end].to_vec(),
            points: original
                .points
                .iter()
                .filter(|p| (start..end).contains(&(p.offset as usize)))
                .map(|p| Point {
                    offset: p.offset - start as u32,
                    value: p.value,
                })
                .collect(),
        });
    }
    let (ok, b) = run("process", &r);
    assert!(ok, "{b}");
    assert_eq!(a["result"]["left"], b["result"]["left"]);
    assert_eq!(a["result"]["state"], b["result"]["state"]);
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_sleep_stop_skip_clock_and_event_wake() {
    let mut r = request();
    let zero = Block {
        left: vec![0.0; 4],
        right: vec![0.0; 4],
        points: vec![],
    };
    r.blocks = vec![
        zero.clone(),
        zero,
        Block {
            left: vec![1.0; 4],
            right: vec![1.0; 4],
            points: vec![Point {
                offset: 0,
                value: 0.5,
            }],
        },
    ];
    let (ok, a) = run("sleep", &r);
    assert!(ok, "{a}");
    clean(&a);
    assert_eq!(a["result"]["statuses"], serde_json::json!([4, 5, 1]));
    assert_eq!(a["result"]["end_frame"], 12);
    assert_eq!(
        a["result"]["left"],
        serde_json::json!([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5])
    );
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_finite_tail_explicit_blocks_and_state_continuation() {
    let mut r = request();
    r.blocks = vec![
        Block {
            left: vec![2.0],
            right: vec![-2.0],
            points: vec![],
        },
        Block {
            left: vec![0.0; 3],
            right: vec![0.0; 3],
            points: vec![],
        },
    ];
    let (ok, a) = run("tail", &r);
    assert!(ok, "{a}");
    assert_eq!(a["result"]["tail"], 2);
    assert_eq!(a["result"]["statuses"], serde_json::json!([3, 2]));
    assert_eq!(a["result"]["left"], serde_json::json!([2.0, 2.0, 1.0, 0.0]));
    r.blocks.truncate(1);
    let (ok, a) = run("tail", &r);
    assert!(ok, "{a}");
    assert_eq!(a["result"]["statuses"], serde_json::json!([3]));
    r.state = serde_json::from_value(a["result"]["state"].clone()).unwrap();
    r.start_frame = 1;
    r.blocks = vec![Block {
        left: vec![0.0; 3],
        right: vec![0.0; 3],
        points: vec![],
    }];
    let (ok, b) = run("tail", &r);
    assert!(ok, "{b}");
    assert_eq!(b["result"]["left"], serde_json::json!([2.0, 1.0, 0.0]));
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_failed_instance_init_is_destroyed_and_entry_deinitialized() {
    let (ok, r) = run("instance-init-fail", &request());
    assert!(!ok, "{r}");
    clean(&r);
    assert!(r["result"].is_null());
    let log = r["output"]["stderr"].as_str().unwrap();
    assert!(log.contains("instance destroy"));
    assert!(log.contains("entry deinit"));
    assert!(log.contains("fixture unload entries=0 instances=0"));
    assert!(!log.contains("instance activate"));
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_error_nonfinite_bus_restart_and_malformed_state_discard() {
    for name in [
        "process-error",
        "process-nonfinite",
        "bus-refuse",
        "restart",
    ] {
        let (ok, r) = run(name, &request());
        assert!(!ok, "{name}: {r}");
        clean(&r);
        assert!(r["result"].is_null());
        assert!(
            r["output"]["stderr"]
                .as_str()
                .unwrap()
                .contains("fixture unload entries=0 instances=0")
        );
    }
    for state in [vec![0], vec![0; 32], vec![0; 33]] {
        let mut request = request();
        request.state = state;
        let (ok, r) = run("process", &request);
        assert!(!ok, "{r}");
        clean(&r);
        assert!(r["result"].is_null());
    }
    let mut request = request();
    request.state = vec![0; 32];
    let (ok, r) = run("state-fail", &request);
    assert!(!ok, "{r}");
    clean(&r);
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_crash_hang_cancel_ownership_reaped() {
    for (name, cancel) in [
        ("process-crash", false),
        ("process-hang", false),
        ("process-hang", true),
    ] {
        let start = Instant::now();
        let (ok, r) = run_at(&receipt(), name, &request(), 200, cancel);
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
#[ignore = "requires original optimized native receipt"]
fn actual_descendant_ledger_kernel_absence() {
    let (ok, r) = run("process-descendant", &request());
    assert!(ok, "{r}");
    clean(&r);
    let ids = r["output"]["cleanup"]["descendant_pids"]
        .as_array()
        .unwrap();
    assert!(!ids.is_empty(), "{r}");
    for pid in ids {
        assert_eq!(
            rustix::process::test_kill_process(
                rustix::process::Pid::from_raw(pid.as_i64().unwrap() as i32).unwrap()
            ),
            Err(rustix::io::Errno::SRCH)
        );
    }
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_preflight_capacity_and_arbitrary_module_refused_before_child() {
    let (mut ok, mut r) = run("installed-plugin", &request());
    assert!(!ok);
    assert!(r["output"].is_null());
    let mut q = request();
    q.blocks[0].points[0].value = 3.0;
    (ok, r) = run("process", &q);
    assert!(!ok);
    assert!(r["output"].is_null());
    q = request();
    q.blocks[0].left = vec![0.0; 1025];
    q.blocks[0].right = vec![0.0; 1025];
    (ok, r) = run("process", &q);
    assert!(!ok);
    assert!(r["output"].is_null());
}

#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_native_wire_negative_controls_refuse_before_module_load() {
    use std::{
        io::Read,
        net::Shutdown,
        os::{fd::OwnedFd, unix::net::UnixStream},
    };
    let root = receipt().parent().unwrap().to_path_buf();
    for kind in 0..4 {
        let mut packet = request().encode().unwrap();
        match kind {
            0 => packet[0] ^= 0xff,
            1 => packet[12] = 2,
            2 => packet[14] = 8,
            _ => {
                let n = packet.len();
                packet[n - 4..].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
            }
        }
        let (mut parent, child) = UnixStream::pair().unwrap();
        let child = Command::new(root.join("hex-clap-offline-helper"))
            .arg(root.join("process.clap"))
            .arg("7")
            .stdin(Stdio::from(OwnedFd::from(child)))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        parent.write_all(&packet).unwrap();
        parent.shutdown(Shutdown::Write).unwrap();
        let mut response = vec![];
        let transfer = parent.read_to_end(&mut response);
        let output = child.wait_with_output().unwrap();
        if let Err(error) = transfer {
            assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
        }
        assert!(!output.status.success());
        assert!(response.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("entry init"));
    }
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_offline_receipt_helper_module_identity_refuse_before_launch() {
    let source = receipt().parent().unwrap().to_path_buf();
    let target = std::env::temp_dir().join(format!("hex-clap-identity-{}", std::process::id()));
    std::fs::create_dir(&target).unwrap();
    for item in std::fs::read_dir(&source).unwrap() {
        let item = item.unwrap();
        if item.file_type().unwrap().is_file() {
            std::fs::copy(item.path(), target.join(item.file_name())).unwrap();
        }
    }
    for file in [
        "process.clap",
        "hex-clap-offline-helper",
        "controlled-offline.json",
    ] {
        let path = target.join(file);
        let original = std::fs::read(&path).unwrap();
        let mut bad = original.clone();
        bad[0] ^= 0xff;
        std::fs::write(&path, &bad).unwrap();
        let (ok, r) = run_at(
            &target.join("controlled-offline.json"),
            "process",
            &request(),
            2000,
            false,
        );
        assert!(!ok, "{r}");
        assert!(r["output"].is_null());
        std::fs::write(&path, &original).unwrap();
    }
    std::fs::remove_dir_all(target).unwrap();
}

#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_late_error_discards_successful_prefix_and_state_clock_overflow() {
    let mut request = request();
    request.blocks.insert(
        0,
        Block {
            left: vec![1.0],
            right: vec![-1.0],
            points: vec![],
        },
    );
    for name in ["process-error", "process-nonfinite"] {
        let (ok, r) = run(name, &request);
        assert!(!ok, "{r}");
        clean(&r);
        assert!(r["result"].is_null());
        let log = r["output"]["stderr"].as_str().unwrap();
        assert!(log.contains("instance stop"));
        assert!(log.contains("fixture unload entries=0 instances=0"));
    }
    let (ok, r) = run("process", &request);
    assert!(ok, "{r}");
    request.state = serde_json::from_value(r["result"]["state"].clone()).unwrap();
    request.state[12..20].copy_from_slice(&(i64::MAX as u64).to_le_bytes());
    request.start_frame = 5;
    let (ok, r) = run("process", &request);
    assert!(!ok, "{r}");
    clean(&r);
    assert!(r["result"].is_null());
    assert!(
        r["output"]["stderr"]
            .as_str()
            .unwrap()
            .contains("fixture state frame overflow")
    );
}

#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_maximum_admitted_frames_and_events_are_processed() {
    let mut r = request();
    r.blocks = vec![
        Block {
            left: vec![0.25; 1024],
            right: vec![-0.25; 1024],
            points: (0..128)
                .map(|offset| Point { offset, value: 2.0 })
                .collect()
        };
        8
    ];
    let (ok, a) = run("process", &r);
    assert!(ok, "{a}");
    clean(&a);
    assert_eq!(a["result"]["end_frame"], 8192);
    let left: Vec<f32> = serde_json::from_value(a["result"]["left"].clone()).unwrap();
    assert_eq!(left, vec![0.5; 8192]);
}
