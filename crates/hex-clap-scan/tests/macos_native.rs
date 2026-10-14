//! Actual enrolled arm64 Mac originals only. Never invoked as Linux native proof.
use hex_clap_scan::offline_protocol::{Block, Point, Request};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn root() -> PathBuf {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        panic!("actual arm64 Mac required");
    }
    PathBuf::from(
        std::env::var_os("HEX_CLAP_DARWIN_ROOT").expect("operator-enrolled root required"),
    )
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
            left: vec![1., 2., 3., 4.],
            right: vec![-1., -2., -3., -4.],
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
fn run(
    name: &str,
    gain: Option<serde_json::Value>,
    cancel: bool,
    ignored: bool,
) -> (bool, serde_json::Value) {
    let root = root();
    let enrolled = hex_clap_scan::receipt_macos::OperatorRoot::open(&root)
        .expect("exclusive enrolled sealed root required before any test execution");
    let binary = if gain.is_some() {
        env!("CARGO_BIN_EXE_hex-clap-offline")
    } else {
        env!("CARGO_BIN_EXE_hex-clap-scan")
    };
    let mut cmd = if ignored {
        let mut c = Command::new(
            enrolled
                .image("launch-ignored", true)
                .expect("enrolled original launcher"),
        );
        c.arg(binary);
        c
    } else {
        Command::new(binary)
    };
    cmd.arg(&root).arg(name);
    let file = gain.map(|v| {
        let p = std::env::temp_dir().join(format!(
            "hex-mac-gain-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
        cmd.arg(&p);
        p
    });
    let mut child = cmd
        .arg("1000")
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
    if let Some(p) = file {
        std::fs::remove_file(p).unwrap();
    }
    let v = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    (out.status.success(), v)
}
fn clean(v: &serde_json::Value) {
    assert_eq!(v["output"]["cleanup"]["direct_reaped"], true, "{v}");
    assert_eq!(v["output"]["cleanup"]["pending"], false, "{v}");
    assert_eq!(v["output"]["cleanup"]["group_signals_scoped"], true, "{v}");
    assert_eq!(
        v["output"]["cleanup"]["descendant_policy"],
        "not-applicable-audited-original-never-fork"
    );
    assert!(v["output"]["packet"].as_array().unwrap().is_empty());
}
#[test]
#[ignore = "requires actual enrolled arm64 Mac ORIGINAL artifacts"]
fn descriptor_enumerates_deinit_unloads_with_native_identity() {
    let (ok, v) = run("good", None, false, false);
    assert!(ok, "{v}");
    clean(&v);
    let d = &v["catalog"]["last_valid"]["discovery"];
    assert_eq!(d["platform"], "macos-arm64");
    assert_eq!(d["descriptors"][0]["id"], "org.hex.controlled.gain");
    assert_eq!(d["descriptors"][0]["features"][2], "hex-original-unknown");
    let logs = v["output"]["stderr"].as_str().unwrap();
    assert!(logs.contains("fixture deinit count=0"));
    assert!(logs.contains("fixture unload initialized=0"));
    assert!(!logs.contains("FORBIDDEN create_plugin"));
}
#[test]
#[ignore = "requires actual enrolled arm64 Mac ORIGINAL artifacts"]
fn descriptor_all_malformed_crash_hang_modes_discard_and_wait() {
    for name in hex_clap_scan::receipt_macos::DESCRIPTOR_NAMES
        .into_iter()
        .filter(|n| *n != "good")
    {
        let (ok, v) = run(name, None, false, false);
        assert!(!ok, "{name}: {v}");
        clean(&v);
        assert!(v["catalog"]["last_valid"].is_null());
    }
}
#[test]
#[ignore = "requires actual enrolled arm64 Mac ORIGINAL artifacts"]
fn gain_pcm_state_continuation_and_owner_oracle() {
    let (ok, v) = run(
        "process",
        Some(serde_json::json!({"request":request(),"state_owner":null})),
        false,
        false,
    );
    assert!(ok, "{v}");
    clean(&v);
    assert_eq!(v["result"]["left"], serde_json::json!([1., 4., 6., 0.]));
    assert_eq!(v["result"]["right"], serde_json::json!([-1., -4., -6., 0.]));
    assert_eq!(v["result"]["platform"], "macos-arm64");
    let mut r = request();
    r.start_frame = 4;
    r.state = serde_json::from_value(v["result"]["state"].clone()).unwrap();
    r.blocks = vec![Block {
        left: vec![1.; 2],
        right: vec![-1.; 2],
        points: vec![],
    }];
    let (ok, next) = run(
        "process",
        Some(serde_json::json!({"request":r,"state_owner":v["state_owner"]})),
        false,
        false,
    );
    assert!(ok, "{next}");
    clean(&next);
    assert_eq!(next["result"]["left"], serde_json::json!([0., 0.]));
    assert_eq!(next["result"]["end_frame"], 6);
    let (ok, wrong) = run(
        "tail",
        Some(serde_json::json!({"request":r,"state_owner":v["state_owner"]})),
        false,
        false,
    );
    assert!(!ok);
    assert!(wrong["output"].is_null());
    assert!(wrong["result"].is_null());
}
#[test]
#[ignore = "requires actual enrolled arm64 Mac ORIGINAL artifacts"]
fn gain_all_failure_profiles_discard_whole_result_and_wait() {
    for name in [
        "process-hang",
        "process-crash",
        "process-nonfinite",
        "process-error",
        "instance-init-fail",
        "state-fail",
        "bus-refuse",
        "restart",
    ] {
        let mut r = request();
        if name == "state-fail" {
            r.state = vec![0; 32];
        }
        let (ok, v) = run(
            name,
            Some(serde_json::json!({"request":r,"state_owner":null})),
            false,
            false,
        );
        assert!(!ok, "{name}: {v}");
        assert!(v["result"].is_null());
        assert!(v["state_owner"].is_null());
        if name == "state-fail" {
            assert!(v["output"].is_null());
        } else {
            clean(&v);
        }
    }
}
#[test]
#[ignore = "requires actual enrolled arm64 Mac ORIGINAL artifacts"]
fn cancellation_and_inherited_autoreap_have_actual_direct_wait() {
    for gain in [false, true] {
        let name = if gain { "process-hang" } else { "hang" };
        let input = gain.then(|| serde_json::json!({"request":request(),"state_owner":null}));
        let (ok, v) = run(name, input, true, false);
        assert!(!ok, "{v}");
        clean(&v);
        assert_eq!(v["output"]["cause"], "cancelled");
        let name = if gain { "process" } else { "good" };
        let input = gain.then(|| serde_json::json!({"request":request(),"state_owner":null}));
        let (ok, v) = run(name, input, false, true);
        assert!(ok, "{v}");
        clean(&v);
    }
}
#[test]
#[ignore = "requires actual enrolled arm64 Mac ORIGINAL artifacts"]
fn unsupported_mac_profiles_refuse_before_child() {
    for name in [
        "descendant",
        "process-descendant",
        "midi",
        "delay",
        "installed",
        "../good.clap",
    ] {
        let (ok, v) = run(name, None, false, false);
        assert!(!ok);
        assert!(v["output"].is_null());
        let (ok, v) = run(
            name,
            Some(serde_json::json!({"request":request(),"state_owner":null})),
            false,
            false,
        );
        assert!(!ok);
        assert!(v["output"].is_null());
    }
}
#[test]
#[ignore = "requires actual enrolled arm64 Mac ORIGINAL artifacts"]
fn gain_state_load_refusal_and_sleep_tail_have_real_native_calls() {
    let (ok, v) = run(
        "state-fail",
        Some(serde_json::json!({"request":request(),"state_owner":null})),
        false,
        false,
    );
    assert!(ok, "{v}");
    clean(&v);
    let mut r = request();
    r.start_frame = 4;
    r.state = serde_json::from_value(v["result"]["state"].clone()).unwrap();
    let (ok, failed) = run(
        "state-fail",
        Some(serde_json::json!({"request":r,"state_owner":v["state_owner"]})),
        false,
        false,
    );
    assert!(!ok, "{failed}");
    clean(&failed);
    assert!(failed["result"].is_null());
    let mut r = request();
    r.blocks = vec![
        Block {
            left: vec![0.; 2],
            right: vec![0.; 2],
            points: vec![]
        };
        2
    ];
    let (ok, sleep) = run(
        "sleep",
        Some(serde_json::json!({"request":r,"state_owner":null})),
        false,
        false,
    );
    assert!(ok, "{sleep}");
    clean(&sleep);
    assert_eq!(sleep["result"]["statuses"], serde_json::json!([4, 5]));
    r.blocks = vec![
        Block {
            left: vec![1.],
            right: vec![-1.],
            points: vec![],
        },
        Block {
            left: vec![0.; 2],
            right: vec![0.; 2],
            points: vec![],
        },
    ];
    let (ok, tail) = run(
        "tail",
        Some(serde_json::json!({"request":r,"state_owner":null})),
        false,
        false,
    );
    assert!(ok, "{tail}");
    clean(&tail);
    assert_eq!(tail["result"]["left"], serde_json::json!([1., 1., 0.5]));
    assert_eq!(tail["result"]["tail"], 2);
}
#[test]
#[ignore = "requires actual enrolled arm64 Mac ORIGINAL artifacts"]
fn partition_oracle_and_late_failure_never_publish_prefix() {
    let mut r = request();
    r.blocks = vec![
        Block {
            left: vec![1., 2.],
            right: vec![-1., -2.],
            points: vec![Point {
                offset: 1,
                value: 2.,
            }],
        },
        Block {
            left: vec![3., 4.],
            right: vec![-3., -4.],
            points: vec![Point {
                offset: 1,
                value: 0.,
            }],
        },
    ];
    let (ok, v) = run(
        "process",
        Some(serde_json::json!({"request":r,"state_owner":null})),
        false,
        false,
    );
    assert!(ok, "{v}");
    clean(&v);
    assert_eq!(v["result"]["left"], serde_json::json!([1., 4., 6., 0.]));
    r.blocks = vec![
        Block {
            left: vec![1.; 2],
            right: vec![1.; 2],
            points: vec![],
        },
        Block {
            left: vec![1.; 4],
            right: vec![1.; 4],
            points: vec![],
        },
    ];
    for name in ["process-error", "process-nonfinite"] {
        let (ok, v) = run(
            name,
            Some(serde_json::json!({"request":r,"state_owner":null})),
            false,
            false,
        );
        assert!(!ok, "{v}");
        clean(&v);
        assert!(v["result"].is_null());
        assert!(v["state_owner"].is_null());
    }
}
