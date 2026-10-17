#![cfg(target_os = "linux")]
//! ORIGINAL protocol3 actual native FIFO oracles; never third-party admission.
use hex_clap_scan::offline_protocol::{DELAY_PROFILE, DelayBlock, DelayOwner, DelayRequest, Point};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn run(name: &str, r: &DelayRequest, deadline: u64, cancel: bool) -> (bool, serde_json::Value) {
    let path =
        PathBuf::from(std::env::var_os("HEX_CLAP_OFFLINE_RECEIPT").expect("explicit receipt"));
    let file = std::env::temp_dir().join(format!(
        "hex-clap-delay-{}-{}.json",
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
fn clean(v: &serde_json::Value) {
    assert_eq!(v["output"]["cleanup"]["direct_reaped"], true);
    assert_eq!(v["output"]["cleanup"]["pending"], false);
    assert_eq!(v["output"]["cleanup"]["group_signals_scoped"], true);
}
fn request(delay: u32, rate: u32, parts: &[usize], drain: &[usize]) -> DelayRequest {
    let mut blocks = vec![];
    let mut frame = 0;
    let events = [
        (0, 0.5),
        (1, 2.0),
        (31, 0.0),
        (31, 1.5),
        (64, 0.0),
        (65, 2.0),
        (127, 1.0),
    ];
    for &n in parts {
        let left = (frame..frame + n)
            .map(|i| {
                if i % 7 == 0 {
                    (i as f32 + 1.0) / 16.0
                } else {
                    0.0
                }
            })
            .collect::<Vec<_>>();
        let right = left.iter().map(|v| -*v).collect();
        let points = events
            .iter()
            .filter(|(i, _)| *i >= frame && *i < frame + n)
            .map(|(i, v)| Point {
                offset: (i - frame) as u32,
                value: *v,
            })
            .collect();
        blocks.push(DelayBlock {
            drain: false,
            left,
            right,
            points,
        });
        frame += n;
    }
    for &n in drain {
        blocks.push(DelayBlock {
            drain: true,
            left: vec![0.0; n],
            right: vec![0.0; n],
            points: vec![],
        });
    }
    DelayRequest {
        version: 3,
        profile: DELAY_PROFILE.into(),
        request: 17,
        sample_rate: rate,
        start_frame: 0,
        initial_gain: 1.0,
        delay,
        state: vec![],
        state_owner: None,
        blocks,
    }
}
fn oracle(r: &DelayRequest) -> Vec<f32> {
    let mut gain = r.initial_gain;
    let mut applied = vec![];
    for b in &r.blocks {
        for (i, x) in b.left.iter().enumerate() {
            for p in b.points.iter().filter(|p| p.offset as usize == i) {
                gain = p.value;
            }
            applied.push((f64::from(*x) * gain) as f32);
        }
    }
    (0..applied.len())
        .map(|i| i.checked_sub(r.delay as usize).map_or(0.0, |j| applied[j]))
        .collect()
}
fn bits(v: &serde_json::Value) -> Vec<u32> {
    serde_json::from_value::<Vec<f32>>(v.clone())
        .unwrap()
        .iter()
        .map(|v| v.to_bits())
        .collect()
}
fn continuation(r: &DelayRequest, v: &serde_json::Value, drain: usize) -> DelayRequest {
    let mut next = r.clone();
    next.start_frame = v["result"]["audio"]["end_frame"].as_u64().unwrap();
    next.state = serde_json::from_value(v["result"]["audio"]["state"].clone()).unwrap();
    next.state_owner =
        Some(serde_json::from_value::<DelayOwner>(v["result"]["state_owner"].clone()).unwrap());
    next.blocks = vec![DelayBlock {
        drain: true,
        left: vec![0.0; drain],
        right: vec![0.0; drain],
        points: vec![],
    }];
    next
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_gain_before_fifo_multirate_partition_wrap_and_demand() {
    for d in [0, 1, 16, 31, 32] {
        for rate in [44100, 48000] {
            let a = request(d, rate, &[128], &[32]);
            let b = request(d, rate, &[1, 30, 1, 32, 1, 31, 32], &[1, 15, 16]);
            let expected = oracle(&a).iter().map(|v| v.to_bits()).collect::<Vec<_>>();
            let mut observed = vec![];
            for r in [&a, &b] {
                let (ok, v) = run(&format!("delay-{d}"), r, 2000, false);
                assert!(ok, "{v}");
                clean(&v);
                let audio = &v["result"]["audio"];
                assert_eq!(bits(&audio["left"]), expected);
                observed.push(bits(&audio["left"]));
                // Right-channel zero may be negative; assert numeric independent stereo oracle.
                let right = serde_json::from_value::<Vec<f32>>(audio["right"].clone()).unwrap();
                for (x, y) in right.iter().zip(oracle(r)) {
                    assert_eq!(*x, -y);
                }
                assert_eq!(audio["latency"], d);
                assert_eq!(audio["tail"], d);
                assert_eq!(v["result"]["pending"], 0);
                assert_eq!(v["result"]["sample_bytes"], 256);
                assert_eq!(v["result"]["metadata_bytes"], 32);
                assert_eq!(
                    v["result"]["input_reserved_bytes"],
                    r.blocks.len() * 128 * 56
                );
                assert_eq!(audio["end_frame"], 160);
                assert_eq!(audio["state"].as_array().unwrap().len(), 360);
            }
            assert_eq!(observed[0], observed[1]);
        }
    }
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn short_drain_pending_state_continuation_and_no_replay() {
    let r = request(32, 48000, &[128], &[1]);
    let (ok, v) = run("delay-32", &r, 2000, false);
    assert!(ok, "{v}");
    assert_eq!(v["result"]["pending"], 31);
    let next = continuation(&r, &v, 31);
    let (ok, w) = run("delay-32", &next, 2000, false);
    assert!(ok, "{w}");
    clean(&w);
    let mut whole = bits(&v["result"]["audio"]["left"]);
    whole.extend(bits(&w["result"]["audio"]["left"]));
    assert_eq!(
        whole,
        oracle(&request(32, 48000, &[128], &[32]))
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>()
    );
    assert_eq!(w["result"]["pending"], 0);
    assert_eq!(w["result"]["state_owner"]["drained"], 32);
    let beyond = continuation(&next, &w, 1);
    let (ok, fail) = run("delay-32", &beyond, 2000, false);
    assert!(!ok);
    assert!(fail["result"].is_null());
    assert!(fail["output"].is_null());
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn restored_state_owner_cursor_queue_ledger_and_profile_refusal_before_child() {
    let r = request(16, 48000, &[128], &[1]);
    let (ok, v) = run("delay-16", &r, 2000, false);
    assert!(ok, "{v}");
    let next = continuation(&r, &v, 15);
    for fault in 0..10 {
        let mut bad = next.clone();
        match fault {
            0 => bad.sample_rate = 44100,
            1 => bad.delay = 31,
            2 => bad.start_frame += 1,
            3 => bad.state_owner.as_mut().unwrap().drained = 0,
            4 => bad.state_owner.as_mut().unwrap().module_sha256 = "f".repeat(64),
            5 => bad.state_owner.as_mut().unwrap().state_version = 2,
            6 => bad.state[12] = 32,
            7 => bad.state[32..36].copy_from_slice(&f32::NAN.to_bits().to_le_bytes()),
            8 => bad.state.pop().map(|_| ()).unwrap(),
            _ => bad.profile = "midi1-cc7-gain-v1".into(),
        }
        let (ok, v) = run("delay-16", &bad, 2000, false);
        assert!(!ok, "fault{fault} {v}");
        assert!(v["result"].is_null());
        assert!(v["output"].is_null());
    }
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn callback_thread_phase_drift_restart_infinite_and_late_failures_discard_all() {
    let r = request(16, 48000, &[64, 64], &[16]);
    for name in [
        "delay-inactive-callback",
        "delay-thread-callback",
        "delay-drift",
        "delay-restart",
        "delay-infinite-tail",
        "delay-late-error",
        "delay-late-nonfinite",
    ] {
        let (ok, v) = run(name, &r, 2000, false);
        assert!(!ok, "{name} {v}");
        assert!(v["result"].is_null());
        clean(&v);
    }
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn shrinking_tail_retains_max_observed_ceiling_across_continuation() {
    let r = request(16, 48000, &[128], &[16]);
    let (ok, v) = run("delay-shrink-tail", &r, 2000, false);
    assert!(ok, "{v}");
    assert_eq!(v["result"]["audio"]["tail"], 32);
    assert_eq!(v["result"]["pending"], 16);
    let next = continuation(&r, &v, 16);
    let (ok, v) = run("delay-shrink-tail", &next, 2000, false);
    assert!(ok, "{v}");
    assert_eq!(v["result"]["audio"]["tail"], 32);
    assert_eq!(v["result"]["pending"], 0);
    clean(&v);
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn event_free_drain_suffix_and_existing_capacities_refuse_before_child() {
    let r = request(16, 48000, &[64], &[16]);
    for fault in 0..9 {
        let mut bad = r.clone();
        match fault {
            0 => bad.blocks[1].left[0] = 1.0,
            1 => bad.blocks[1].points.push(Point {
                offset: 0,
                value: 1.0,
            }),
            2 => bad.blocks.swap(0, 1),
            3 => bad.blocks[1].left.resize(33, 0.0),
            4 => {
                bad.blocks[0].points = vec![
                    Point {
                        offset: 0,
                        value: 1.0
                    };
                    129
                ]
            }
            5 => bad.blocks = vec![bad.blocks[0].clone(); 33],
            6 => {
                bad.blocks[0].left = vec![0.0; 1024];
                bad.blocks[0].right = vec![0.0; 1024];
                bad.blocks = vec![bad.blocks[0].clone(); 9];
            }
            7 => bad.blocks[0].points[0].offset = 64,
            _ => bad.initial_gain = 3.0,
        }
        let (ok, v) = run("delay-16", &bad, 2000, false);
        assert!(!ok, "{fault} {v}");
        assert!(v["result"].is_null());
        assert!(v["output"].is_null());
    }
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn drain_hang_crash_cancel_and_descendant_cleanup_expose_no_partial_success() {
    let r = request(16, 48000, &[64], &[16]);
    for (name, cancel) in [
        ("delay-drain-hang", false),
        ("delay-drain-hang", true),
        ("delay-drain-crash", false),
    ] {
        let (ok, v) = run(name, &r, 500, cancel);
        assert!(!ok, "{name} {v}");
        assert!(v["result"].is_null());
        clean(&v);
    }
}

#[test]
#[ignore = "requires original optimized native receipt"]
fn descendant_success_is_published_only_after_actual_group_reap() {
    let r = request(16, 48000, &[64], &[16]);
    let (ok, v) = run("delay-descendant", &r, 2000, false);
    assert!(ok, "{v}");
    clean(&v);
    assert!(
        v["output"]["cleanup"]["descendants_reaped"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert_eq!(
        bits(&v["result"]["audio"]["left"]),
        oracle(&r).iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn callback_phases_through_destruction_and_final_stop_tail_are_checked() {
    let r = request(16, 48000, &[64], &[16]);
    for name in [
        "delay-active-callback",
        "delay-main-tail",
        "delay-large-tail",
        "delay-late-callback",
        "delay-ignored-output",
        "delay-late-output",
        "delay-foreign-output",
    ] {
        let (ok, v) = run(name, &r, 2000, false);
        assert!(!ok, "{name} {v}");
        assert!(v["result"].is_null());
        clean(&v);
    }
    let (ok, v) = run("delay-stop-tail", &r, 2000, false);
    assert!(ok, "{v}");
    clean(&v);
    assert_eq!(v["result"]["audio"]["tail"], 32);
    assert_eq!(v["result"]["pending"], 16);
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn zero_delay_direct_and_single_impulse_have_exact_declared_sample_delay() {
    for d in [0, 1, 16, 31, 32] {
        let mut r = request(d, 48000, &[1], &[32]);
        r.blocks[0].left = vec![1.0];
        r.blocks[0].right = vec![-1.0];
        r.blocks[0].points = vec![];
        let (ok, v) = run(&format!("delay-{d}"), &r, 2000, false);
        assert!(ok, "{v}");
        let mut expected = [0.0f32; 33];
        expected[d as usize] = 1.0;
        assert_eq!(
            bits(&v["result"]["audio"]["left"]),
            expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
    }
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn raw_protocol3_malformed_headers_events_and_drain_refuse_before_load() {
    use hex_clap_scan::offline_protocol::DELAY_PLUGIN_ID;
    use std::{
        io::Read,
        net::Shutdown,
        os::{fd::OwnedFd, unix::net::UnixStream},
    };
    let path = PathBuf::from(std::env::var_os("HEX_CLAP_OFFLINE_RECEIPT").unwrap());
    let root = path.parent().unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let hash = manifest["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "delay-16")
        .unwrap()["sha256"]
        .as_str()
        .unwrap();
    let mut r = request(16, 48000, &[1], &[]);
    r.blocks[0].points = vec![Point {
        offset: 0,
        value: 1.0,
    }];
    let valid = r.encode_bound(hash).unwrap();
    let rate = 12 + 2 + 8 + 4 + DELAY_PLUGIN_ID.len();
    let delay = rate + 4 + 8 + 8 + 4 + DELAY_PROFILE.len() + 4 + 4 + 64;
    let block = delay + 4 + 4 + 4 + 4 + 4;
    let event = block + 1 + 4 + 4;
    assert_eq!(valid[block], 0);
    for case in 0..15 {
        let mut packet = valid.clone();
        let mut word = |at: usize, n: u32| packet[at..at + 4].copy_from_slice(&n.to_le_bytes());
        match case {
            0 => packet[0] ^= 0xff,
            1 => packet[12] = 2,
            2 => word(rate, 96000),
            3 => word(delay, 17),
            4 => word(delay + 4, 15),
            5 => word(delay + 8, 33),
            6 => packet[block] = 2,
            7 => word(block + 1, 0),
            8 => word(block + 5, 129),
            9 => word(event, 1),
            10 => packet[event + 4..event + 12].copy_from_slice(&f64::NAN.to_bits().to_le_bytes()),
            11 => word(event + 12, f32::INFINITY.to_bits()),
            12 => packet[block] = 1, // nonempty event list in drain
            13 => word(block - 4, 33),
            _ => {
                packet.push(0);
                let n = packet.len() - 12;
                packet[8..12].copy_from_slice(&(n as u32).to_le_bytes());
            }
        }
        let (mut parent, child) = UnixStream::pair().unwrap();
        parent
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut child = Command::new(root.join("hex-clap-offline-helper"))
            .arg(root.join("delay-16.clap"))
            .arg("17")
            .stdin(Stdio::from(OwnedFd::from(child)))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        parent.write_all(&packet).unwrap();
        parent.shutdown(Shutdown::Write).unwrap();
        let mut response = vec![];
        let transfer = parent.read_to_end(&mut response);
        if transfer
            .as_ref()
            .is_err_and(|e| e.kind() != std::io::ErrorKind::ConnectionReset)
        {
            let _ = child.kill();
        }
        let output = child.wait_with_output().unwrap();
        if let Err(e) = transfer {
            assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset);
        }
        assert!(!output.status.success(), "case{case}");
        assert!(response.is_empty());
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("entry init"),
            "case{case}"
        );
    }
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_input_1024_boundary_and_exact_1025_prechild_refusal() {
    let mut r = request(16, 48000, &[1; 8], &[32]);
    for b in &mut r.blocks[..8] {
        b.points = vec![
            Point {
                offset: 0,
                value: 2.0
            };
            128
        ];
    }
    let (ok, v) = run("delay-16", &r, 2000, false);
    assert!(ok, "{v}");
    clean(&v);
    assert_eq!(
        bits(&v["result"]["audio"]["left"]),
        oracle(&r).iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    let extra = DelayBlock {
        drain: false,
        left: vec![0.0],
        right: vec![0.0],
        points: vec![Point {
            offset: 0,
            value: 1.0,
        }],
    };
    r.blocks.insert(8, extra);
    let (ok, v) = run("delay-16", &r, 2000, false);
    assert!(!ok);
    assert!(v["result"].is_null());
    assert!(v["output"].is_null());
}
#[test]
#[ignore = "requires original optimized native receipt"]
fn actual_arithmetic_overflow_in_delayed_pcm_or_pending_state_discards_prefix() {
    for d in [0, 16, 32] {
        for drain in [false, true] {
            let mut r = request(d, 48000, &[64], &[]);
            r.blocks.push(DelayBlock {
                drain: false,
                left: vec![f32::MAX],
                right: vec![-f32::MAX],
                points: vec![Point {
                    offset: 0,
                    value: 2.0,
                }],
            });
            if drain && d > 0 {
                r.blocks.push(DelayBlock {
                    drain: true,
                    left: vec![0.0; d as usize],
                    right: vec![0.0; d as usize],
                    points: vec![],
                });
            }
            let (ok, v) = run(&format!("delay-{d}"), &r, 2000, false);
            assert!(!ok, "{v}");
            assert!(v["result"].is_null());
            clean(&v);
        }
    }
}
