//! Finite hostile-wire contracts; actual SDK behavior is tested separately.
use super::*;
use hex_audio::plugin_transport::{Events, MidiEvent};
use hex_plugin_state::{BinaryIdentity, ClassId, State};
fn config() -> Configuration {
    Configuration {
        binding: Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        },
        identity: Identity {
            class: ClassId::new([
                0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
            ])
            .unwrap(),
            binary: BinaryIdentity {
                executable_sha256: [11; 32],
                info_sha256: [12; 32],
            },
        },
        sample_rate: 48000.0,
        quantum: 8,
        input_channels: 2,
        output_channels: 2,
        bundle: "/owned.vst3".into(),
        executable: "Contents/MacOS/fixture".into(),
        restore: None,
    }
}
fn hex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn configuration_matches_independent_little_endian_wire_and_refuses_bad_profile() {
    // Independent Python struct.pack fixture, not constructed through this codec.
    let golden = hex(
        "4858564943463033ab00000001000100000000000000020000000000000003000000000000000400000000000000484558500102030405060708090a0b0c0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c000000000070e74008000202000000000b0000002f6f776e65642e7673743316000000436f6e74656e74732f4d61634f532f66697874757265",
    );
    let config = config();
    assert_eq!(config.encode().unwrap(), golden);
    let mut zero = config.clone();
    zero.binding.root = 0;
    assert!(zero.encode().is_err());
    let mut bad_path = config.clone();
    bad_path.bundle.push('\0');
    assert!(bad_path.encode().is_err());
    for rate in [0.0, -1.0, f64::NAN, f64::INFINITY, 768001.0] {
        let mut bad = config.clone();
        bad.sample_rate = rate;
        assert!(bad.encode().is_err());
    }
    for quantum in [0, 257, u16::MAX] {
        let mut bad = config.clone();
        bad.quantum = quantum;
        assert!(bad.encode().is_err());
    }
}
fn ready_body(config: &Configuration) -> Vec<u8> {
    let state = hex_plugin_state::encode(State {
        identity: config.identity,
        component: Some(&[1, 2, 3]),
        controller: None,
    })
    .unwrap();
    let mut out = Vec::new();
    stamp(&mut out, config.binding);
    out.extend(config.identity.class.bytes());
    out.extend(config.sample_rate.to_le_bytes());
    out.extend(config.quantum.to_le_bytes());
    out.extend([config.input_channels, config.output_channels]);
    out.extend(u32::MAX.to_le_bytes());
    out.extend([1, 1]); // applied, actual contributions supplied
    out.extend(77_u32.to_le_bytes());
    out.extend(91_u32.to_le_bytes());
    out.push(u8::from(config.restore.is_some()));
    out.push(1); // component state exists
    out.extend((state.payload().len() as u16).to_le_bytes());
    out.extend(state.payload());
    out
}
#[test]
fn ready_requires_complete_source_config_and_state_presence_without_sizing_from_latency() {
    let config = config();
    let body = ready_body(&config);
    let ready = Ready::decode(&body, &config, known()).unwrap();
    assert_eq!(ready.algorithmic_latency_frames, u32::MAX);
    assert_eq!(
        hex_plugin_state::decode(&ready.initial_state)
            .unwrap()
            .component,
        Some([1, 2, 3].as_slice())
    );
    for offset in [
        0, 2, 34, 50, 58, 60, 61, 66, 67, 68, 72, 76, 77, 78, 80, 88, 104, 136,
    ] {
        let mut wrong = body.clone();
        wrong[offset] ^= 0x80;
        assert!(
            Ready::decode(&wrong, &config, known()).is_err(),
            "field{offset}"
        );
    }
    for length in [0, 1, 34, 66, 69, body.len() - 1] {
        assert!(Ready::decode(&body[..length], &config, known()).is_err());
    }
    let mut trailing = body.clone();
    trailing.push(0);
    assert!(Ready::decode(&trailing, &config, known()).is_err());
    let mut absent = body.clone();
    absent[66] = 0;
    assert_eq!(
        Ready::decode(&absent, &config, known())
            .unwrap()
            .presentation,
        Presentation::Absent
    );
}
#[test]
fn immutable_request_bounds_and_failed_reply_never_fabricate_pcm() {
    let config = config();
    let block = Block {
        binding: config.binding,
        sequence: 0,
        first_frame: 0,
        frames: 8,
        pcm: [[0.5; 2]; 256],
        events: Events::default(),
    };
    config.validate_block(&block, 0).unwrap();
    let mut bytes = Vec::new();
    hex_audio::plugin_transport::wire::write(&mut bytes, &block).unwrap();
    assert_eq!(pcm(&bytes, &config, 0).unwrap().pcm[..8], block.pcm[..8]);
    let mut wrong = bytes.clone();
    wrong[16] ^= 1;
    assert!(pcm(&wrong, &config, 0).is_err());
    assert!(pcm(&bytes[..bytes.len() - 1], &config, 0).is_err());
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(pcm(&extra, &config, 0).is_err());
    let mut event = block;
    event
        .events
        .push(MidiEvent {
            frame: 0,
            bytes: [0x90, 60, 100],
            len: 3,
        })
        .unwrap();
    assert!(config.validate_block(&event, 0).is_err());
    let mut bad = config.clone();
    bad.quantum = u16::MAX;
    assert!(bad.validate_block(&block, 0).is_err());
    assert!(pcm(&bytes, &bad, 0).is_err());
    let mut nonfinite = block;
    nonfinite.pcm[0][0] = f32::NAN;
    assert!(config.validate_block(&nonfinite, 0).is_err());
}

fn known() -> Option<PresentationLatency> {
    Some(PresentationLatency {
        input_frames: 77,
        output_frames: 91,
    })
}
#[test]
fn facts_require_exact_source_format_and_interface_presence_and_do_not_acknowledge_state() {
    let config = config();
    let mut body = Vec::new();
    stamp(&mut body, config.binding);
    body.extend(config.identity.class.bytes());
    body.extend(config.identity.binary.executable_sha256);
    body.extend(config.identity.binary.info_sha256);
    body.extend(config.sample_rate.to_le_bytes());
    body.extend(config.quantum.to_le_bytes());
    body.extend([config.input_channels, config.output_channels]);
    body.extend(u32::MAX.to_le_bytes());
    body.push(1);
    body.extend([0, 0, 0]);
    assert_eq!(
        Facts::decode(&body, &config).unwrap(),
        Facts {
            algorithmic_latency_frames: u32::MAX,
            presentation_available: true,
            restored: false
        }
    );
    for offset in [0, 2, 34, 50, 82, 114, 122, 124, 125, 130, 131, 132, 133] {
        let mut bad = body.clone();
        bad[offset] ^= 0x80;
        assert!(Facts::decode(&bad, &config).is_err());
    }
    assert!(Facts::decode(&body[..130], &config).is_err());
    body.push(0);
    assert!(Facts::decode(&body, &config).is_err());
    let absent = presentation_request(config.binding, None).unwrap();
    let zero = presentation_request(
        config.binding,
        Some(PresentationLatency {
            input_frames: 0,
            output_frames: 0,
        }),
    )
    .unwrap();
    assert_eq!(absent.len(), 12 + 35);
    assert_eq!(zero.len(), 12 + 43);
    assert_ne!(absent, zero);
    assert!(Ready::decode(&ready_body(&config), &config, None).is_err());
}

#[test]
fn startup_restore_requires_current_source_and_exact_facts_and_final_acknowledgement() {
    let mut config = config();
    let state = hex_plugin_state::encode(State {
        identity: config.identity,
        component: Some(&[]),
        controller: Some(&[7, 8]),
    })
    .unwrap();
    let original = state.clone();
    config.restore = Some(state);
    let request = config.startup_request().unwrap();
    assert_eq!(&request[..8], b"HXVIST01");
    assert_eq!(&request[46..49], &[1, 98, 0]); // presence + full source envelope size
    assert_eq!(&request[49..], original.payload());
    let mut facts = Vec::new();
    stamp(&mut facts, config.binding);
    facts.extend(config.identity.class.bytes());
    facts.extend(config.identity.binary.executable_sha256);
    facts.extend(config.identity.binary.info_sha256);
    facts.extend(config.sample_rate.to_le_bytes());
    facts.extend(config.quantum.to_le_bytes());
    facts.extend([config.input_channels, config.output_channels]);
    facts.extend(3_u32.to_le_bytes());
    facts.push(1); // real optional presentation interface
    facts.extend(&request[46..]);
    assert!(Facts::decode(&facts, &config).unwrap().restored);
    let last = facts.len() - 1;
    facts[last] ^= 1;
    assert!(
        Facts::decode(&facts, &config).is_err(),
        "a different restored envelope cannot acknowledge the requested one"
    );
    let ready = ready_body(&config);
    assert!(Ready::decode(&ready, &config, known()).unwrap().restored);
    let mut missing_ack = ready;
    missing_ack[76] = 0;
    assert!(Ready::decode(&missing_ack, &config, known()).is_err());
    assert_eq!(
        config.restore.as_ref().unwrap(),
        &original,
        "a normalized final capture never overwrites the caller's canonical state"
    );
    let mut changed = config.clone();
    changed.identity.binary.executable_sha256[0] ^= 1;
    assert!(matches!(
        changed.startup_request(),
        Err(Error::State(hex_plugin_state::Error::BinaryChanged))
    ));
    let mut missing = config.clone();
    missing.restore = Some(
        hex_plugin_state::encode(State {
            identity: config.identity,
            component: None,
            controller: None,
        })
        .unwrap(),
    );
    assert!(matches!(
        missing.startup_request(),
        Err(Error::State(hex_plugin_state::Error::MissingComponent))
    ));
}
