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
fn original_state(magic: u32, gain: f64, frames: u64) -> Vec<u8> {
    let mut state = vec![];
    u(&mut state, magic.into(), 4);
    u(&mut state, gain.to_bits(), 8);
    u(&mut state, frames, 8);
    u(&mut state, 0, 4);
    u(&mut state, 0, 4);
    u(&mut state, 0, 4);
    state
}
struct PacketFixture {
    bytes: Vec<u8>,
    reject_offsets: Vec<usize>,
    pcm_start: usize,
    status: usize,
    platform_data: usize,
}
fn packet_for(platform: &str) -> PacketFixture {
    let mut p = vec![];
    let mut reject_offsets = vec![0, 8, 12, 14];
    u(&mut p, 1, 2);
    u(&mut p, 7, 8);
    let mut platform_data = 0;
    for text in [SDK, platform, PLUGIN_ID] {
        if text == platform {
            platform_data = 16 + p.len();
        }
        reject_offsets.extend([12 + p.len(), 16 + p.len()]);
        blob(&mut p, text.as_bytes());
    }
    for (value, width) in [(48000, 4), (0, 8), (4, 8)] {
        reject_offsets.push(12 + p.len());
        u(&mut p, value, width);
    }
    for value in [0, 0, 2, 2, 0, 0, 1, GAIN_ID, 32] {
        reject_offsets.push(12 + p.len());
        u(&mut p, u64::from(value), 4);
    }
    for value in [0.0_f64, 2.0, 1.0] {
        reject_offsets.push(12 + p.len());
        u(&mut p, value.to_bits(), 8);
    }
    blob(&mut p, b"gain");
    reject_offsets.push(12 + p.len());
    u(&mut p, 4, 4);
    let pcm_start = 12 + p.len();
    for value in [1.0_f32, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0] {
        u(&mut p, u64::from(value.to_bits()), 4);
    }
    blob(&mut p, &original_state(1, 1.0, 4));
    u(&mut p, 1, 4);
    blob(&mut p, b"unload main");
    u(&mut p, 0, 4);
    u(&mut p, 1, 4);
    let status = 12 + p.len();
    u(&mut p, 1, 4);
    let mut bytes = b"HCOFFR01".to_vec();
    u(&mut bytes, p.len() as u64, 4);
    bytes.extend(p);
    PacketFixture {
        bytes,
        reject_offsets,
        pcm_start,
        status,
        platform_data,
    }
}
fn packet() -> Vec<u8> {
    packet_for(hex_clap_scan::platform()).bytes
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
    let fixture = packet_for(hex_clap_scan::platform());
    let b = fixture.bytes;
    assert!(decode(&b, &request()).is_ok());
    for at in fixture.reject_offsets {
        let mut bad = b.clone();
        bad[at] ^= 0xff;
        assert!(decode(&bad, &request()).is_err(), "offset {at}");
    }
    for sample in 0..8 {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut bad = b.clone();
            let at = fixture.pcm_start + sample * 4;
            bad[at..at + 4].copy_from_slice(&value.to_bits().to_le_bytes());
            assert_eq!(
                decode(&bad, &request()).unwrap_err(),
                "non-finite processed PCM refused"
            );
        }
    }
    let mut bad = b;
    bad[fixture.status..fixture.status + 4].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(
        decode(&bad, &request()).unwrap_err(),
        "process status refused"
    );
}
#[test]
fn platform_tag_lengths_do_not_move_the_negative_control_out_of_pcm() {
    for platform in ["linux-x86_64", "macos-arm64"] {
        let fixture = packet_for(platform);
        assert_eq!(
            &fixture.bytes[fixture.pcm_start..fixture.pcm_start + 4],
            &1f32.to_le_bytes()
        );
        assert_eq!(
            &fixture.bytes[fixture.pcm_start + 16..fixture.pcm_start + 20],
            &(-1f32).to_le_bytes()
        );
    }
    assert_eq!(
        packet_for("linux-x86_64").pcm_start,
        packet_for("macos-arm64").pcm_start + 1
    );
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
    let fixture = packet_for(hex_clap_scan::platform());
    let mut b = fixture.bytes;
    b[fixture.status..fixture.status + 4].copy_from_slice(&5u32.to_le_bytes());
    assert_eq!(decode(&b, &r).unwrap_err(), "invalid sleep skip refused");
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
struct MidiFixture {
    bytes: Vec<u8>,
    pcm: usize,
    platform_data: usize,
    profile: usize,
    owner: usize,
    owner_version: usize,
    port: usize,
    abi: usize,
    final_gain: usize,
    event_count: usize,
    event: usize,
    event_end: usize,
}
fn midi_packet_for(platform: &str) -> MidiFixture {
    let mut p = vec![];
    u(&mut p, 2, 2);
    u(&mut p, 7, 8);
    let mut platform_data = 0;
    for text in [SDK, platform, MIDI_PLUGIN_ID] {
        if text == platform {
            platform_data = 16 + p.len();
        }
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
    let pcm = 12 + p.len();
    for v in [0.5_f32, 0.5, 0.5, 0.5, -0.5, -0.5, -0.5, -0.5] {
        u(&mut p, u64::from(v.to_bits()), 4);
    }
    blob(&mut p, &original_state(0x43433702, 0.5, 4));
    u(&mut p, 1, 4);
    blob(&mut p, b"unload main");
    u(&mut p, 0, 4);
    u(&mut p, 1, 4);
    u(&mut p, 1, 4);
    let profile = 16 + p.len();
    blob(&mut p, MIDI_PROFILE.as_bytes());
    let owner_version = 12 + p.len();
    u(&mut p, 2, 4);
    let owner = 16 + p.len();
    blob(&mut p, "0".repeat(64).as_bytes());
    let port = 12 + p.len();
    for v in [MIDI_PORT_ID, 2, 2] {
        u(&mut p, u64::from(v), 4);
    }
    blob(&mut p, b"original cc7 input");
    let abi = 12 + p.len();
    for v in [24, 56, 64] {
        u(&mut p, v, 4);
    }
    u(&mut p, 8192, 8);
    u(&mut p, 32768, 8);
    let final_gain = 12 + p.len();
    u(&mut p, 0.5_f64.to_bits(), 8);
    let event_count = 12 + p.len();
    u(&mut p, 1, 4);
    let event = 12 + p.len();
    for v in [0, 0, 2, GAIN_ID] {
        u(&mut p, u64::from(v), 4);
    }
    u(&mut p, 0, 8);
    u(&mut p, 2.0_f64.to_bits(), 8);
    let event_end = 12 + p.len();
    let mut b = b"HCOFFR02".to_vec();
    u(&mut b, p.len() as u64, 4);
    b.extend(p);
    MidiFixture {
        bytes: b,
        pcm,
        platform_data,
        profile,
        owner,
        owner_version,
        port,
        abi,
        final_gain,
        event_count,
        event,
        event_end,
    }
}
fn midi_packet() -> Vec<u8> {
    midi_packet_for(hex_clap_scan::platform()).bytes
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
    let fixture = midi_packet_for(hex_clap_scan::platform());
    let positions = [
        (fixture.profile, "MIDI state owner/profile refused"),
        (fixture.owner, "MIDI state owner/profile refused"),
        (fixture.owner_version, "MIDI state owner/profile refused"),
        (fixture.port, "MIDI port/profile refused"),
        (fixture.port + 4, "MIDI port/profile refused"),
        (fixture.port + 8, "MIDI port/profile refused"),
        (fixture.abi, "MIDI ABI reservation refused"),
        (fixture.abi + 4, "MIDI ABI reservation refused"),
        (fixture.abi + 8, "MIDI ABI reservation refused"),
        (fixture.abi + 12, "MIDI ABI reservation refused"),
        (fixture.abi + 20, "MIDI ABI reservation refused"),
        (fixture.event, "output block refused"),
        (fixture.event + 4, "MIDI output event refused"),
        (fixture.event + 8, "MIDI output event refused"),
        (fixture.event + 12, "MIDI output event refused"),
        (fixture.event + 16, "MIDI output frame refused"),
    ];
    for (at, error) in positions {
        let mut bad = fixture.bytes.clone();
        // ASCII-preserving string mutation reaches identity checks, not UTF8 errors.
        bad[at] = if at == fixture.profile || at == fixture.owner {
            b'z'
        } else {
            bad[at] ^ 0xff
        };
        assert_eq!(
            decode_midi(&bad, &midi_request(), &"0".repeat(64)).unwrap_err(),
            error,
            "{at}"
        );
    }
    for (at, error) in [
        (fixture.final_gain, "MIDI final gain refused"),
        (fixture.event + 24, "MIDI output event refused"),
    ] {
        let mut bad = fixture.bytes.clone();
        bad[at..at + 8].copy_from_slice(&f64::NAN.to_bits().to_le_bytes());
        assert_eq!(
            decode_midi(&bad, &midi_request(), &"0".repeat(64)).unwrap_err(),
            error
        );
    }
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
    let fixture = midi_packet_for(hex_clap_scan::platform());
    let mut b = fixture.bytes;
    let event = b[fixture.event..fixture.event_end].to_vec();
    assert_eq!(event.len(), 32); // fixed scalar output record ABI; independent of tag length.
    let count_at = fixture.event_count;
    b[count_at..count_at + 4].copy_from_slice(&129u32.to_le_bytes());
    for _ in 0..128 {
        b.extend(&event);
    }
    let len = b.len() - 12;
    b[8..12].copy_from_slice(&(len as u32).to_le_bytes());
    assert_eq!(
        decode_midi(&b, &midi_request(), &"0".repeat(64)).unwrap_err(),
        "MIDI block output capacity refused"
    );
}

#[test]
fn gain_and_midi_locations_for_both_supported_tags_and_foreign_tag_refusal() {
    for platform in ["linux-x86_64", "macos-arm64"] {
        let gain = packet_for(platform);
        for end in 0..gain.bytes.len() {
            assert!(decode(&gain.bytes[..end], &request()).is_err());
        }
        assert_eq!(
            &gain.bytes[gain.platform_data..gain.platform_data + platform.len()],
            platform.as_bytes()
        );
        assert_eq!(
            &gain.bytes[gain.status..gain.status + 4],
            &1u32.to_le_bytes()
        );
        let midi = midi_packet_for(platform);
        for end in 0..midi.bytes.len() {
            assert!(decode_midi(&midi.bytes[..end], &midi_request(), &"0".repeat(64)).is_err());
        }
        assert_eq!(
            &midi.bytes[midi.platform_data..midi.platform_data + platform.len()],
            platform.as_bytes()
        );
        for (at, word) in [
            (midi.owner_version, 2u32),
            (midi.port, MIDI_PORT_ID),
            (midi.abi, 24),
            (midi.abi + 4, 56),
            (midi.abi + 8, 64),
            (midi.event_count, 1),
            (midi.event, 0),
            (midi.event + 8, 2),
            (midi.event + 12, GAIN_ID),
        ] {
            assert_eq!(&midi.bytes[at..at + 4], &word.to_le_bytes());
        }
        assert_eq!(midi.event_end - midi.event, 32);
        assert_eq!(&midi.bytes[midi.pcm..midi.pcm + 4], &0.5f32.to_le_bytes());
        assert_eq!(
            &midi.bytes[midi.profile..midi.profile + MIDI_PROFILE.len()],
            MIDI_PROFILE.as_bytes()
        );
        if platform != hex_clap_scan::platform() {
            assert_eq!(
                decode(&gain.bytes, &request()).unwrap_err(),
                "offline identity/capability refused"
            );
            assert_eq!(
                decode_midi(&midi.bytes, &midi_request(), &"0".repeat(64)).unwrap_err(),
                "offline identity/capability refused"
            );
        }
    }
    assert_eq!(
        decode(&packet_for("unsupported").bytes, &request()).unwrap_err(),
        "offline identity/capability refused"
    );
    assert_eq!(
        decode_midi(
            &midi_packet_for("unsupported").bytes,
            &midi_request(),
            &"0".repeat(64)
        )
        .unwrap_err(),
        "offline identity/capability refused"
    );
}
