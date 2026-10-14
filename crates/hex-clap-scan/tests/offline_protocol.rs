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
    for s in [SDK, hex_clap_scan::platform(), PLUGIN_ID] {
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

use hex_clap_scan::offline_protocol::{
    InputEvent, MIDI_PLUGIN_ID, MIDI_PORT_ID, MIDI_PROFILE, MidiBlock, MidiRequest, StateOwner,
    decode_midi,
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
            left: vec![1.0; 4],
            right: vec![-1.0; 4],
            events: vec![
                InputEvent::Midi {
                    offset: 0,
                    port: 0,
                    data: [0xb0, 7, 127],
                },
                InputEvent::Gain {
                    offset: 0,
                    value: 0.5,
                },
            ],
        }],
    }
}
fn midi_packet() -> Vec<u8> {
    let mut p = vec![];
    u(&mut p, 2, 2);
    u(&mut p, 7, 8);
    for text in [SDK, hex_clap_scan::platform(), MIDI_PLUGIN_ID] {
        blob(&mut p, text.as_bytes());
    }
    u(&mut p, 48000, 4);
    u(&mut p, 0, 8);
    u(&mut p, 4, 8);
    for v in [0, 0, 2, 2, 1, 0, 1, GAIN_ID, 32] {
        u(&mut p, u64::from(v), 4);
    }
    for v in [0.0_f64, 2.0, 1.0] {
        u(&mut p, v.to_bits(), 8);
    }
    blob(&mut p, b"gain");
    u(&mut p, 4, 4);
    for v in [0.5_f32, 0.5, 0.5, 0.5, -0.5, -0.5, -0.5, -0.5] {
        u(&mut p, u64::from(v.to_bits()), 4);
    }
    blob(&mut p, &[0; 32]);
    u(&mut p, 1, 4);
    blob(&mut p, b"unload main");
    u(&mut p, 0, 4);
    u(&mut p, 1, 4);
    u(&mut p, 1, 4);
    blob(&mut p, MIDI_PROFILE.as_bytes());
    u(&mut p, 2, 4);
    blob(&mut p, "0".repeat(64).as_bytes());
    for v in [MIDI_PORT_ID, 2, 2] {
        u(&mut p, u64::from(v), 4);
    }
    blob(&mut p, b"original cc7 input");
    for v in [24, 56, 64] {
        u(&mut p, v, 4);
    }
    u(&mut p, 8192, 8);
    u(&mut p, 32768, 8);
    u(&mut p, 0.5_f64.to_bits(), 8);
    u(&mut p, 1, 4);
    for v in [0, 0, 2, GAIN_ID] {
        u(&mut p, u64::from(v), 4);
    }
    u(&mut p, 0, 8);
    u(&mut p, 2.0_f64.to_bits(), 8);
    let mut b = b"HCOFFR02".to_vec();
    u(&mut b, p.len() as u64, 4);
    b.extend(p);
    b
}
#[test]
fn midi_owned_profile_and_all_response_truncations_refuse() {
    let b = midi_packet();
    let r = decode_midi(&b, &midi_request(), &"0".repeat(64)).unwrap();
    assert_eq!(r.events[0].value, 2.0);
    assert_eq!(r.final_gain, 0.5);
    assert_eq!(r.abi.input_reserved_bytes, 8192);
    for n in 0..b.len() {
        assert!(
            decode_midi(&b[..n], &midi_request(), &"0".repeat(64)).is_err(),
            "{n}"
        );
    }
    let mut trailing = b;
    trailing.push(0);
    assert!(decode_midi(&trailing, &midi_request(), &"0".repeat(64)).is_err());
}
#[test]
fn midi_response_profile_owner_port_abi_flags_clock_value_refuse() {
    let b = midi_packet();
    let mut positions = vec![0, 8, 12, 14, 26];
    let tail = b.len() - 32;
    positions.extend([tail, tail + 4, tail + 8, tail + 12, tail + 16, tail + 23]);
    let profile_at = b
        .windows(MIDI_PROFILE.len())
        .position(|w| w == MIDI_PROFILE.as_bytes())
        .unwrap();
    positions.push(profile_at);
    let owner_at = b
        .windows(64)
        .position(|w| w == "0".repeat(64).as_bytes())
        .unwrap();
    positions.push(owner_at);
    let port_at = b
        .windows(4)
        .position(|w| w == MIDI_PORT_ID.to_le_bytes())
        .unwrap();
    positions.extend([port_at, port_at + 4, port_at + 8]);
    let abi_at = b.len() - 32 - 4 - 8 - 16 - 12;
    positions.extend([abi_at, abi_at + 4, abi_at + 8, abi_at + 12, abi_at + 20]);
    for at in positions {
        let mut bad = b.clone();
        bad[at] ^= 0xff;
        assert!(
            decode_midi(&bad, &midi_request(), &"0".repeat(64)).is_err(),
            "{at}"
        );
    }
    let mut bad = b;
    let n = bad.len();
    bad[n - 8..].copy_from_slice(&f64::NAN.to_bits().to_le_bytes());
    assert!(decode_midi(&bad, &midi_request(), &"0".repeat(64)).is_err());
}
#[test]
fn midi_request_kind_dialect_order_target_and_strict_json_refuse() {
    let mut r = midi_request();
    assert!(r.encode_bound(&"0".repeat(64)).is_ok());
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
            offset: 0,
            value: f64::NAN,
        },
    ] {
        r.blocks[0].events = vec![event];
        assert!(r.validate().is_err());
    }
    r = midi_request();
    r.blocks[0].events.reverse();
    assert!(r.validate().is_ok());
    r.blocks[0].events[0] = InputEvent::Gain {
        offset: 3,
        value: 1.0,
    };
    assert!(r.validate().is_err());
    let mut json = serde_json::to_value(midi_request()).unwrap();
    json["blocks"][0]["events"][0]["cookie"] = serde_json::json!(123);
    assert!(serde_json::from_value::<MidiRequest>(json).is_err());
}
#[test]
fn midi_state_profile_owner_presence_and_identity_refuse() {
    let mut r = midi_request();
    r.state = vec![0; 32];
    assert!(r.validate().is_err());
    r.state_owner = Some(StateOwner {
        profile: MIDI_PROFILE.into(),
        module_sha256: "0".repeat(64),
        state_version: 2,
    });
    assert!(r.encode_bound(&"0".repeat(64)).is_ok());
    assert!(r.encode_bound(&"1".repeat(64)).is_err());
    r.state_owner.as_mut().unwrap().state_version = 1;
    assert!(r.validate().is_err());
    r.state.clear();
    assert!(r.validate().is_err());
}
#[test]
fn midi_combined_capacity_does_not_double_and_output_order_bounds_refuse() {
    let mut r = midi_request();
    r.blocks = vec![
        MidiBlock {
            left: vec![0.0; 1024],
            right: vec![0.0; 1024],
            events: (0..128)
                .map(|offset| if offset % 2 == 0 {
                    InputEvent::Midi {
                        offset,
                        port: 0,
                        data: [0xb0, 7, 127],
                    }
                } else {
                    InputEvent::Gain { offset, value: 0.0 }
                })
                .collect()
        };
        8
    ];
    assert_eq!(r.validate().unwrap(), 8192);
    r.blocks[0].events.push(InputEvent::Gain {
        offset: 128,
        value: 0.0,
    });
    assert!(r.validate().is_err());
    r.blocks[0].events.pop();
    r.blocks.push(r.blocks[0].clone());
    assert!(r.validate().is_err());
    let mut b = midi_packet();
    let n = b.len();
    let event = b[n - 32..].to_vec();
    let count_at = n - 36;
    b[count_at..count_at + 4].copy_from_slice(&129u32.to_le_bytes());
    for _ in 0..128 {
        b.extend(&event);
    }
    let len = b.len() - 12;
    b[8..12].copy_from_slice(&(len as u32).to_le_bytes());
    assert!(decode_midi(&b, &midi_request(), &"0".repeat(64)).is_err());
}
