use hex_plugin_scan::offline_protocol::{
    Block, COMPONENT_ID, CONTROLLER_ID, GAIN_ID, Point, Request, decode,
};
fn request() -> Request {
    Request {
        version: 1,
        request: 7,
        sample_rate: 48000,
        start_frame: 0,
        controller_value: 0.5,
        component_state: vec![],
        controller_state: vec![],
        blocks: vec![Block {
            left: vec![0.25; 8],
            right: vec![-0.5; 8],
            points: vec![],
        }],
    }
}
#[test]
fn request_boundaries_order_nonfinite_and_clocks() {
    let q = request();
    assert_eq!(q.validate().unwrap(), 8);
    assert!(q.encode().unwrap().starts_with(b"HVOFFQ01"));
    for mutate in [
        |q: &mut Request| q.version = 2,
        |q: &mut Request| q.request = 0,
        |q: &mut Request| q.sample_rate = 96000,
        |q: &mut Request| q.start_frame = i64::MAX as u64,
        |q: &mut Request| q.controller_value = f64::NAN,
        |q: &mut Request| q.component_state = vec![0; 65537],
        |q: &mut Request| q.controller_state = vec![0; 65537],
        |q: &mut Request| q.blocks.clear(),
        |q: &mut Request| q.blocks = vec![q.blocks[0].clone(); 33],
        |q: &mut Request| q.blocks[0].left[0] = f32::INFINITY,
        |q: &mut Request| q.blocks[0].right.pop().map(|_| ()).unwrap(),
        |q: &mut Request| {
            q.blocks[0].points = vec![Point {
                offset: 8,
                value: 0.5,
            }]
        },
        |q: &mut Request| {
            q.blocks[0].points = vec![
                Point {
                    offset: 1,
                    value: 0.5,
                },
                Point {
                    offset: 1,
                    value: 0.8,
                },
            ]
        },
        |q: &mut Request| {
            q.blocks[0].points = vec![Point {
                offset: 1,
                value: -0.1,
            }]
        },
    ] {
        let mut bad = q.clone();
        mutate(&mut bad);
        assert!(bad.encode().is_err());
    }
    let mut max = q.clone();
    max.blocks = vec![
        Block {
            left: vec![0.; 1024],
            right: vec![0.; 1024],
            points: (0..128)
                .map(|i| Point {
                    offset: i,
                    value: 0.5
                })
                .collect()
        };
        8
    ];
    assert_eq!(max.validate().unwrap(), 8192);
    max.blocks.push(max.blocks[0].clone());
    assert!(max.validate().is_err());
}
fn u(v: &mut Vec<u8>, n: u64, size: usize) {
    for i in 0..size {
        v.push((n >> (i * 8)) as u8);
    }
}
fn text(v: &mut Vec<u8>, s: &str) {
    u(v, s.len() as u64, 4);
    v.extend(s.as_bytes());
}
fn response() -> Vec<u8> {
    let mut p = vec![];
    u(&mut p, 1, 2);
    u(&mut p, 7, 8);
    text(&mut p, hex_plugin_scan::SDK);
    text(&mut p, hex_plugin_scan::PUBLIC_SDK);
    text(&mut p, "linux-x86_64");
    p.extend(COMPONENT_ID);
    p.extend(CONTROLLER_ID);
    for n in [48000, 0, 0, 2, 2, 0, 0, 1, GAIN_ID, 1, 0] {
        u(&mut p, n as u64, 4);
    }
    u(&mut p, 0.5f64.to_bits(), 8);
    text(&mut p, "gain");
    u(&mut p, 8, 4);
    for _ in 0..16 {
        u(&mut p, 0.25f32.to_bits() as u64, 4);
    }
    for _ in 0..4 {
        u(&mut p, 0, 4);
    }
    let mut result = b"HVOFFR01".to_vec();
    u(&mut result, p.len() as u64, 4);
    result.extend(p);
    result
}
#[test]
fn every_response_truncation_stale_identity_and_trailing_refuses() {
    let q = request();
    let bytes = response();
    assert_eq!(decode(&bytes, &q).unwrap().left, vec![0.25; 8]);
    for n in 0..bytes.len() {
        assert!(decode(&bytes[..n], &q).is_err(), "length{n}");
    }
    for at in [
        0, 8, 12, 14, 22, 65, 110, 126, 142, 158, 162, 166, 170, 174, 178, 182, 186, 190, 194,
    ] {
        let mut bad = bytes.clone();
        bad[at] ^= 0xff;
        assert!(decode(&bad, &q).is_err(), "offset{at}");
    }
    let mut bad = bytes.clone();
    bad.push(0);
    assert!(decode(&bad, &q).is_err());
    let mut q = q;
    q.request = 8;
    assert!(decode(&bytes, &q).is_err());
}
#[test]
fn output_nonfinite_and_oversized_state_refuse() {
    let q = request();
    let bytes = response();
    let pcm = bytes.len() - 16 - 64;
    let mut bad = bytes.clone();
    bad[pcm..pcm + 4].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
    assert!(decode(&bad, &q).is_err());
    let mut bad = bytes;
    let n = bad.len() - 16;
    bad[n..n + 4].copy_from_slice(&65537u32.to_le_bytes());
    assert!(decode(&bad, &q).is_err());
}
