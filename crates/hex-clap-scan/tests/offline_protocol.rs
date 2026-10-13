use hex_clap_scan::{
    SDK,
    offline_protocol::{Block, GAIN_ID, PLUGIN_ID, Point, Request, decode},
};
fn request() -> Request {
    Request {
        version: 1,
        request: 7,
        sample_rate: 48000,
        start_frame: 0,
        initial_gain: 1.0,
        state: vec![],
        blocks: vec![Block {
            left: vec![1.0; 4],
            right: vec![-1.0; 4],
            points: vec![],
        }],
    }
}
fn u(b: &mut Vec<u8>, v: u64, n: usize) {
    for i in 0..n {
        b.push((v >> (i * 8)) as u8);
    }
}
fn blob(b: &mut Vec<u8>, v: &[u8]) {
    u(b, v.len() as u64, 4);
    b.extend(v);
}
fn packet() -> Vec<u8> {
    let mut p = vec![];
    u(&mut p, 1, 2);
    u(&mut p, 7, 8);
    for s in [SDK, "linux-x86_64", PLUGIN_ID] {
        blob(&mut p, s.as_bytes());
    }
    u(&mut p, 48000, 4);
    u(&mut p, 0, 8);
    u(&mut p, 4, 8);
    for v in [0, 0, 2, 2, 0, 0, 1, GAIN_ID, 32] {
        u(&mut p, u64::from(v), 4);
    }
    for v in [0.0_f64, 2.0, 1.0] {
        u(&mut p, v.to_bits(), 8);
    }
    blob(&mut p, b"gain");
    u(&mut p, 4, 4);
    for v in [1.0_f32, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0] {
        u(&mut p, u64::from(v.to_bits()), 4);
    }
    blob(&mut p, &[0; 32]);
    u(&mut p, 1, 4);
    blob(&mut p, b"unload main");
    u(&mut p, 0, 4);
    u(&mut p, 1, 4);
    u(&mut p, 1, 4);
    let mut b = b"HCOFFR01".to_vec();
    u(&mut b, p.len() as u64, 4);
    b.extend(p);
    b
}
#[test]
fn owned_response_and_every_truncation_refusal() {
    let b = packet();
    let r = decode(&b, &request()).unwrap();
    assert_eq!(r.left, vec![1.0; 4]);
    assert_eq!(r.parameters[0].max, 2.0);
    for n in 0..b.len() {
        assert!(decode(&b[..n], &request()).is_err(), "{n}");
    }
    let mut trailing = b.clone();
    trailing.push(0);
    assert!(decode(&trailing, &request()).is_err());
}
#[test]
fn response_identity_clock_capability_pcm_status_refuse() {
    let b = packet();
    for at in [
        0, 8, 12, 14, 26, 74, 92, 130, 134, 142, 150, 154, 158, 162, 166, 170, 174, 178, 182, 190,
        198,
    ] {
        let mut bad = b.clone();
        bad[at] ^= 0xff;
        assert!(decode(&bad, &request()).is_err(), "offset {at}");
    }
    let mut nonfinite = b.clone();
    nonfinite[209..213].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
    assert!(decode(&nonfinite, &request()).is_err());
    let mut bad = b;
    let n = bad.len();
    bad[n - 4..].copy_from_slice(&0u32.to_le_bytes());
    assert!(decode(&bad, &request()).is_err());
}
#[test]
fn request_plain_order_boundaries_coincident_and_nonfinite() {
    let mut r = request();
    r.blocks[0].points = vec![
        Point {
            offset: 0,
            value: 0.0,
        },
        Point {
            offset: 0,
            value: 2.0,
        },
    ];
    assert!(r.encode().is_ok());
    r.blocks[0].points[1].value = 2.0001;
    assert!(r.encode().is_err());
    r.blocks[0].points[1].value = f64::NAN;
    assert!(r.encode().is_err());
    r.blocks[0].points = vec![
        Point {
            offset: 2,
            value: 1.0,
        },
        Point {
            offset: 1,
            value: 1.0,
        },
    ];
    assert!(r.encode().is_err());
    r.blocks[0].points = vec![Point {
        offset: 4,
        value: 1.0,
    }];
    assert!(r.encode().is_err());
    r = request();
    r.blocks[0].right[0] = f32::INFINITY;
    assert!(r.encode().is_err());
    r = request();
    r.start_frame = 1;
    assert!(r.encode().is_err());
    r.state = vec![1];
    r.start_frame = i64::MAX as u64;
    assert!(r.encode().is_err());
}
#[test]
fn actual_finite_request_capacity_boundaries() {
    let mut r = request();
    r.blocks = vec![
        Block {
            left: vec![0.0; 1024],
            right: vec![0.0; 1024],
            points: (0..128)
                .map(|i| Point {
                    offset: i,
                    value: 1.0
                })
                .collect()
        };
        8
    ];
    r.state = vec![0; 65536];
    assert_eq!(r.validate().unwrap(), 8192);
    assert!(r.encode().is_ok());
    r.blocks.push(r.blocks[0].clone());
    assert!(r.encode().is_err());
    r.blocks.pop();
    r.blocks[0].points.push(Point {
        offset: 129,
        value: 1.0,
    });
    assert!(r.encode().is_err());
    r = request();
    r.state = vec![0; 65537];
    assert!(r.encode().is_err());
    r = request();
    r.blocks = vec![r.blocks[0].clone(); 33];
    assert!(r.encode().is_err());
}
#[test]
fn strict_json_and_impossible_sleep_skip_refuse() {
    let r = request();
    let mut json = serde_json::to_value(&r).unwrap();
    json["device"] = serde_json::json!("installed");
    assert!(serde_json::from_value::<Request>(json).is_err());
    let mut b = packet();
    let n = b.len();
    b[n - 4..].copy_from_slice(&5u32.to_le_bytes());
    assert!(decode(&b, &r).is_err());
}
