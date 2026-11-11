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
    r: &impl serde::Serialize,
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

use hex_clap_scan::offline_protocol::{
    GAIN_ID, InputEvent, MIDI_PORT_ID, MIDI_PROFILE, MidiBlock, MidiRequest, StateOwner,
};
fn midi_request() -> MidiRequest {
    MidiRequest {
        version: 2,
        profile: MIDI_PROFILE.into(),
        request: 7,
        sample_rate: 48000,
        start_frame: 0,
        initial_gain: 1.0,
        state: vec![],
        state_owner: None,
        blocks: vec![MidiBlock {
            left: vec![1.0, 2.0, 3.0, 4.0],
            right: vec![-1.0, -2.0, -3.0, -4.0],
            events: vec![
                InputEvent::Midi {
                    offset: 1,
                    port: 0,
                    data: [0xb0, 7, 127],
                },
                InputEvent::Gain {
                    offset: 1,
                    value: 0.5,
                },
            ],
        }],
    }
}
fn midi_run(name: &str, r: &MidiRequest) -> (bool, serde_json::Value) {
    run_at(&receipt(), name, r, 2000, false)
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_mixed_pcm_order_owned_output_no_cookie_or_feedback() {
    let (ok, r) = midi_run("midi", &midi_request());
    assert!(ok, "{r}");
    clean(&r);
    let a = &r["result"];
    assert_eq!(a["audio"]["left"], serde_json::json!([1.0, 1.0, 1.5, 2.0]));
    assert_eq!(
        a["audio"]["right"],
        serde_json::json!([-1.0, -1.0, -1.5, -2.0])
    );
    assert_eq!(a["final_gain"], 0.5);
    assert_eq!(a["port"]["id"], MIDI_PORT_ID);
    assert_eq!(a["port"]["supported_dialects"], 2);
    assert_eq!(
        a["events"],
        serde_json::json!([{"block":0,"offset":1,"frame":1,"flags":2,"parameter":GAIN_ID,"value":2.0}])
    );
    // The fixture mutates its output value to -999 after try_push. The copied report stays2,
    // while the later same-offset host gain controls PCM and final state: no output feedback.
    assert!(a["events"][0].get("cookie").is_none());
    assert_eq!(a["abi"]["midi_event_bytes"], 24);
    assert_eq!(a["abi"]["parameter_event_bytes"], 56);
    assert_eq!(a["abi"]["input_slot_bytes"], 64);
    assert_eq!(a["abi"]["input_reserved_bytes"], 8192);
    assert_eq!(a["abi"]["output_reserved_bytes"], 32768);
    let mut reverse = midi_request();
    reverse.blocks[0].events.reverse();
    let (ok, r) = midi_run("midi", &reverse);
    assert!(ok, "{r}");
    assert_eq!(
        r["result"]["audio"]["left"],
        serde_json::json!([1.0, 4.0, 6.0, 8.0])
    );
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_independent_all_channel_cc_oracle_and_partition_equivalence() {
    let mut r = midi_request();
    r.initial_gain = 0.25;
    r.blocks = vec![MidiBlock {
        left: (0..64).map(|n| n as f32 / 8.0 - 4.0).collect(),
        right: (0..64).map(|n| 4.0 - n as f32 / 8.0).collect(),
        events: (0..16)
            .map(|channel| InputEvent::Midi {
                offset: channel as u32 * 4,
                port: 0,
                data: [0xb0 + channel, 7, channel * 8],
            })
            .collect(),
    }];
    let mut gain = r.initial_gain;
    let expected: Vec<f32> = r.blocks[0]
        .left
        .iter()
        .enumerate()
        .map(|(i, v)| {
            for e in &r.blocks[0].events {
                if let InputEvent::Midi { offset, data, .. } = e
                    && *offset == i as u32
                {
                    gain = 2.0 * f64::from(data[2]) / 127.0;
                }
            }
            (f64::from(*v) * gain) as f32
        })
        .collect();
    let (ok, a) = midi_run("midi", &r);
    assert!(ok, "{a}");
    let actual: Vec<f32> = serde_json::from_value(a["result"]["audio"]["left"].clone()).unwrap();
    assert_eq!(
        actual.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    let events = a["result"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 16);
    for (channel, e) in events.iter().enumerate() {
        assert_eq!(e["frame"], channel * 4);
        let expected = 2.0 * (channel * 8) as f64 / 127.0;
        // Compare in the diagnostic JSON representation; PCM above is exact Float32 bits.
        let expected_json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&expected).unwrap()).unwrap();
        assert_eq!(e["value"], expected_json);
        assert_eq!(e["flags"], 2);
    }
    let b = r.blocks.remove(0);
    for (start, end) in [(0, 7), (7, 23), (23, 42), (42, 64)] {
        r.blocks.push(MidiBlock {
            left: b.left[start..end].to_vec(),
            right: b.right[start..end].to_vec(),
            events: b
                .events
                .iter()
                .filter(|e| (start..end).contains(&(e.offset() as usize)))
                .map(|e| match e {
                    InputEvent::Midi { offset, port, data } => InputEvent::Midi {
                        offset: *offset - start as u32,
                        port: *port,
                        data: *data,
                    },
                    InputEvent::Gain { .. } => unreachable!(),
                })
                .collect(),
        });
    }
    let (ok, b) = midi_run("midi", &r);
    assert!(ok, "{b}");
    assert_eq!(a["result"]["audio"]["left"], b["result"]["audio"]["left"]);
    assert_eq!(a["result"]["audio"]["state"], b["result"]["audio"]["state"]);
    let frames: Vec<_> = b["result"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["frame"].clone())
        .collect();
    assert_eq!(
        frames,
        events
            .iter()
            .map(|e| e["frame"].clone())
            .collect::<Vec<_>>()
    );
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_state_owner_continuation_no_event_replay_and_endpoints() {
    let mut request = midi_request();
    request.blocks[0].events = vec![InputEvent::Midi {
        offset: 0,
        port: 0,
        data: [0xbf, 7, 127],
    }];
    let (ok, a) = midi_run("midi", &request);
    assert!(ok, "{a}");
    request.state = serde_json::from_value(a["result"]["audio"]["state"].clone()).unwrap();
    request.state_owner = Some(serde_json::from_value(a["result"]["state_owner"].clone()).unwrap());
    request.start_frame = 4;
    request.initial_gain = 0.0;
    request.blocks[0].events.clear();
    let (ok, b) = midi_run("midi", &request);
    assert!(ok, "{b}");
    assert_eq!(
        b["result"]["audio"]["left"],
        serde_json::json!([2.0, 4.0, 6.0, 8.0])
    );
    assert!(b["result"]["events"].as_array().unwrap().is_empty());
    request.blocks[0].events = vec![InputEvent::Midi {
        offset: 2,
        port: 0,
        data: [0xb1, 7, 0],
    }];
    let (ok, b) = midi_run("midi", &request);
    assert!(ok, "{b}");
    assert_eq!(
        b["result"]["audio"]["left"],
        serde_json::json!([2.0, 4.0, 0.0, 0.0])
    );
    assert_eq!(b["result"]["events"][0]["frame"], 6);
    assert_eq!(b["result"]["final_gain"], 0.0);
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_zero_pcm_events_wake_sleep_and_reports_are_preserved() {
    let mut r = midi_request();
    let zero = MidiBlock {
        left: vec![0.0; 4],
        right: vec![0.0; 4],
        events: vec![],
    };
    r.blocks = vec![zero.clone(), zero.clone(), zero];
    r.blocks[2].events = vec![InputEvent::Midi {
        offset: 3,
        port: 0,
        data: [0xb0, 7, 127],
    }];
    let (ok, a) = midi_run("midi-sleep", &r);
    assert!(ok, "{a}");
    assert_eq!(
        a["result"]["audio"]["statuses"],
        serde_json::json!([4, 5, 4])
    );
    assert_eq!(a["result"]["events"][0]["frame"], 11);
    assert_eq!(a["result"]["final_gain"], 2.0);
    assert_eq!(a["result"]["audio"]["end_frame"], 12);
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_all_malformed_output_fields_ignored_rejection_discards() {
    for bad in 0..13 {
        let mut r = midi_request();
        r.blocks[0].events = vec![InputEvent::Midi {
            offset: 0,
            port: 0,
            data: [0xb0, 7, bad],
        }];
        let (ok, a) = midi_run("midi-bad-output", &r);
        assert!(!ok, "case{bad}: {a}");
        clean(&a);
        assert!(a["result"].is_null());
        assert!(
            a["output"]["stderr"]
                .as_str()
                .unwrap()
                .contains("output event rejection discards whole request")
        );
    }
    for name in ["midi-unsorted-output", "midi-block-overflow"] {
        let (ok, a) = midi_run(name, &midi_request());
        assert!(!ok, "{a}");
        clean(&a);
        assert!(a["result"].is_null());
    }
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_output_aggregate_boundary_and_late_overflow_no_prefix_state() {
    let mut r = midi_request();
    r.blocks = vec![
        MidiBlock {
            left: vec![0.0],
            right: vec![0.0],
            events: vec![]
        };
        8
    ];
    let (ok, a) = midi_run("midi-total-overflow", &r);
    assert!(ok, "{a}");
    assert_eq!(a["result"]["events"].as_array().unwrap().len(), 1024);
    r.blocks.push(r.blocks[0].clone());
    let (ok, a) = midi_run("midi-total-overflow", &r);
    assert!(!ok, "{a}");
    clean(&a);
    assert!(a["result"].is_null());
    assert_eq!(a["output"]["packet"], serde_json::json!([]));
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_late_process_error_no_successful_events_pcm_or_state() {
    let mut r = midi_request();
    r.blocks.insert(
        0,
        MidiBlock {
            left: vec![1.0],
            right: vec![-1.0],
            events: vec![InputEvent::Midi {
                offset: 0,
                port: 0,
                data: [0xb0, 7, 127],
            }],
        },
    );
    let (ok, a) = midi_run("midi-error", &r);
    assert!(!ok, "{a}");
    clean(&a);
    assert!(a["result"].is_null());
    assert!(
        a["output"]["stderr"]
            .as_str()
            .unwrap()
            .contains("instance stop")
    );
    assert!(
        a["output"]["stderr"]
            .as_str()
            .unwrap()
            .contains("fixture unload entries=0 instances=0")
    );
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_rescan_wrong_thread_and_dialect_fail_cleanly() {
    for name in ["midi-rescan", "midi-wrong-thread", "midi-bad-port"] {
        let (ok, a) = midi_run(name, &midi_request());
        assert!(!ok, "{name} {a}");
        clean(&a);
        assert!(a["result"].is_null());
        assert!(
            a["output"]["stderr"]
                .as_str()
                .unwrap()
                .contains("fixture unload entries=0 instances=0")
        );
    }
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_crash_deadline_cancel_and_descendant_reap() {
    for (name, cancel) in [
        ("midi-crash", false),
        ("midi-hang", false),
        ("midi-hang", true),
    ] {
        let start = Instant::now();
        let (ok, a) = run_at(&receipt(), name, &midi_request(), 200, cancel);
        assert!(!ok, "{a}");
        clean(&a);
        assert!(a["result"].is_null());
        assert!(start.elapsed() < Duration::from_secs(3));
        if cancel {
            assert_eq!(a["error"], "cancelled");
        }
    }
    let (ok, a) = midi_run("midi-descendant", &midi_request());
    assert!(ok, "{a}");
    clean(&a);
    let ids = a["output"]["cleanup"]["descendant_pids"]
        .as_array()
        .unwrap();
    assert!(!ids.is_empty());
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
fn midi_actual_state_class_version_owner_and_malformed_refusal() {
    let (ok, a) = midi_run("midi", &midi_request());
    assert!(ok, "{a}");
    let mut r = midi_request();
    r.state = serde_json::from_value(a["result"]["audio"]["state"].clone()).unwrap();
    r.state_owner = Some(serde_json::from_value(a["result"]["state_owner"].clone()).unwrap());
    r.start_frame = 4;
    for field in 0..4 {
        let mut bad = r.clone();
        match field {
            0 => bad.state_owner = None,
            1 => bad.state_owner.as_mut().unwrap().state_version = 1,
            2 => bad.state_owner.as_mut().unwrap().profile = "legacy".into(),
            _ => bad.state_owner.as_mut().unwrap().module_sha256 = "0".repeat(64),
        }
        let (ok, a) = midi_run("midi", &bad);
        assert!(!ok, "{a}");
        assert!(a["output"].is_null());
    }
    for state in [vec![0], vec![0; 32], vec![0; 33]] {
        let mut bad = r.clone();
        bad.state = state;
        let (ok, a) = midi_run("midi", &bad);
        assert!(!ok, "{a}");
        clean(&a);
        assert!(a["result"].is_null());
    }
    let (ok, old) = run("process", &request());
    assert!(ok, "{old}");
    r.state = serde_json::from_value(old["result"]["state"].clone()).unwrap();
    let (ok, a) = midi_run("midi", &r);
    assert!(!ok, "{a}");
    clean(&a);
    assert!(a["result"].is_null());
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_unsupported_input_and_profile_refused_before_child() {
    for event in [
        InputEvent::Midi {
            offset: 0,
            port: 1,
            data: [0xb0, 7, 1],
        },
        InputEvent::Midi {
            offset: 0,
            port: 0,
            data: [0x90, 60, 1],
        },
        InputEvent::Midi {
            offset: 0,
            port: 0,
            data: [0xb0, 8, 1],
        },
        InputEvent::Midi {
            offset: 0,
            port: 0,
            data: [0xb0, 7, 128],
        },
        InputEvent::Gain {
            offset: 4,
            value: 1.0,
        },
        InputEvent::Gain {
            offset: 0,
            value: 3.0,
        },
    ] {
        let mut r = midi_request();
        r.blocks[0].events = vec![event];
        let (ok, a) = midi_run("midi", &r);
        assert!(!ok, "{a}");
        assert!(a["output"].is_null());
    }
    let (ok, a) = midi_run("process", &midi_request());
    assert!(!ok, "{a}");
    assert!(a["output"].is_null());
    let (ok, a) = run_at(&receipt(), "midi", &request(), 2000, false);
    assert!(!ok, "{a}");
    assert!(a["output"].is_null());
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_maximum_combined_input_capacity_and_checked_abi() {
    let mut r = midi_request();
    r.blocks = vec![
        MidiBlock {
            left: vec![0.25; 1024],
            right: vec![-0.25; 1024],
            events: (0..128)
                .map(|offset| if offset % 2 == 0 {
                    InputEvent::Midi {
                        offset,
                        port: 0,
                        data: [0xb0, 7, 127],
                    }
                } else {
                    InputEvent::Gain { offset, value: 2.0 }
                })
                .collect()
        };
        8
    ];
    let (ok, a) = midi_run("midi", &r);
    assert!(ok, "{a}");
    clean(&a);
    let left: Vec<f32> = serde_json::from_value(a["result"]["audio"]["left"].clone()).unwrap();
    assert_eq!(left, vec![0.5; 8192]);
    assert_eq!(a["result"]["events"].as_array().unwrap().len(), 512);
    assert_eq!(a["result"]["abi"]["input_reserved_bytes"], 8 * 128 * 64);
    r.blocks[0].events.push(InputEvent::Gain {
        offset: 129,
        value: 1.0,
    });
    let (ok, a) = midi_run("midi", &r);
    assert!(!ok, "{a}");
    assert!(a["output"].is_null());
}

#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_state_load_failure_and_memoryless_state_validation() {
    let (ok, a) = midi_run("midi", &midi_request());
    assert!(ok, "{a}");
    let mut r = midi_request();
    r.state = serde_json::from_value(a["result"]["audio"]["state"].clone()).unwrap();
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(receipt()).unwrap()).unwrap();
    let hash = receipt["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "midi-state-fail")
        .unwrap()["sha256"]
        .as_str()
        .unwrap();
    r.state_owner = Some(StateOwner {
        profile: MIDI_PROFILE.into(),
        module_sha256: hash.into(),
        state_version: 2,
    });
    let (ok, a) = midi_run("midi-state-fail", &r);
    assert!(!ok, "{a}");
    clean(&a);
    assert!(a["result"].is_null());
    assert!(
        a["output"]["stderr"]
            .as_str()
            .unwrap()
            .contains("state load failed")
    );
    let hash = receipt["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "midi")
        .unwrap()["sha256"]
        .as_str()
        .unwrap();
    r.state_owner.as_mut().unwrap().module_sha256 = hash.into();
    for at in [4, 12, 20, 24, 28] {
        let mut bad = r.clone();
        if at == 4 {
            bad.state[4..12].copy_from_slice(&f64::NAN.to_bits().to_le_bytes());
        } else if at == 12 {
            bad.state[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
        } else {
            bad.state[at] = 1;
        }
        let (ok, a) = midi_run("midi", &bad);
        assert!(!ok, "{a}");
        clean(&a);
        assert!(a["result"].is_null());
    }
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_raw_wire_malformed_cases_refuse_before_module_load() {
    use hex_clap_scan::offline_protocol::{MIDI_PLUGIN_ID, MIDI_PROFILE};
    use std::{
        io::Read,
        net::Shutdown,
        os::{fd::OwnedFd, unix::net::UnixStream},
    };
    let root = receipt().parent().unwrap().to_path_buf();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(receipt()).unwrap()).unwrap();
    let hash = manifest["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "midi")
        .unwrap()["sha256"]
        .as_str()
        .unwrap();
    let valid = midi_request().encode_bound(hash).unwrap();
    let event_at = 12
        + 2
        + 8
        + 4
        + MIDI_PLUGIN_ID.len()
        + 4
        + 8
        + 8
        + 4
        + MIDI_PROFILE.len()
        + 4
        + 4
        + 64
        + 4
        + 4
        + 4
        + 4;
    assert_eq!(valid[event_at], 1);
    for case in 0..9 {
        let mut packet = valid.clone();
        match case {
            0 => packet[0] ^= 0xff,
            1 => packet[12] = 1,
            2 => packet[event_at] = 255,
            3 => packet[event_at + 1..event_at + 5].copy_from_slice(&4u32.to_le_bytes()),
            4 => packet[event_at + 5] = 1,
            5 => packet[event_at + 7] = 0x70,
            6 => packet[event_at + 8] = 8,
            7 => packet[event_at + 9] = 128,
            _ => {
                packet.push(0);
                let n = packet.len() - 12;
                packet[8..12].copy_from_slice(&(n as u32).to_le_bytes());
            }
        }
        let (mut parent, child) = UnixStream::pair().unwrap();
        let child = Command::new(root.join("hex-clap-offline-helper"))
            .arg(root.join("midi.clap"))
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
        if let Err(e) = transfer {
            assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset);
        }
        assert!(!output.status.success(), "case{case}");
        assert!(response.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("entry init"));
    }
}

#[test]
#[ignore = "requires original optimized native receipt"]
fn midi_actual_output_on_foreign_thread_or_destroy_discards_all() {
    for name in ["midi-output-thread", "midi-late-output"] {
        let (ok, r) = midi_run(name, &midi_request());
        assert!(!ok, "{r}");
        clean(&r);
        assert!(r["result"].is_null());
        let log = r["output"]["stderr"].as_str().unwrap();
        assert!(log.contains("instance destroy"));
        assert!(log.contains("fixture unload entries=0 instances=0"));
        if name == "midi-late-output" {
            assert!(log.contains("late output rejection discards whole request"));
        }
    }
}
