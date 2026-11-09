//! Finite hostile-wire contracts; actual SDK behavior is tested separately.
use super::*;

#[test]
fn editor_receipt_requires_exact_controller_operation_and_discriminant() {
    let config = config();
    for (request, code) in [
        (ControllerRequest::ToggleEditor, 6),
        (ControllerRequest::DetachEditor, 9),
        (ControllerRequest::ReopenEditor, 10),
    ] {
        let wire = parameter_page_request(&config, 1, 7, request, false).unwrap();
        assert_eq!(&wire[..4], b"HXVM");
        assert_eq!(wire[82], code);
        assert!(wire[83..].iter().all(|byte| *byte == 0));
        let mut reply = Vec::new();
        stamp(&mut reply, config.binding);
        for value in [1_u64, u64::from(config.quantum), 7] {
            reply.extend(value.to_le_bytes());
        }
        reply.extend([4, 1]);
        assert_eq!(
            parameter_page(&reply, &config, 1, 7, request).unwrap(),
            ParameterPageProgress::Editor(PluginEditorState::Open)
        );
        assert!(parameter_page(&reply, &config, 1, 8, request).is_err());
        assert!(parameter_page(&reply, &config, 1, 7, ParameterPageQuery::default()).is_err());
        reply.push(0);
        assert!(parameter_page(&reply, &config, 1, 7, request).is_err());
    }
}
#[path = "protocol_tests/conversion.rs"]
mod conversion;
use crate::transport::{Events, MidiEvent};
use hex_plugin_state::{BinaryIdentity, ClassId, State};
pub(super) fn config() -> Configuration {
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

#[test]
fn live_capture_poll_binds_current_exchange_and_rejects_partial_or_stale_state() {
    let config = config();
    let state = hex_plugin_state::encode(State {
        identity: config.identity,
        component: Some(b"live component"),
        controller: Some(b"live controller"),
    })
    .unwrap();
    for poll in [false, true] {
        let request = live_capture_request(&config, 9, 2, poll).unwrap();
        assert_eq!(&request[..4], if poll { b"HXVP" } else { b"HXVL" });
        assert_eq!(&request[48..56], &9_u64.to_le_bytes());
        assert_eq!(&request[56..64], &72_u64.to_le_bytes());
        assert_eq!(&request[64..], &2_u64.to_le_bytes());
        assert!(live_capture_request(&config, 9, 0, poll).is_err());
    }
    let mut body = Vec::new();
    stamp(&mut body, config.binding);
    for value in [9_u64, 72, 2] {
        body.extend(value.to_le_bytes());
    }
    body.extend([0; 5]);
    assert_eq!(live_captured_state(&body, &config, 9, 2).unwrap(), None);
    body[58] = 1;
    body[59..63].copy_from_slice(&(state.payload().len() as u32).to_le_bytes());
    body.extend(state.payload());
    assert_eq!(
        live_captured_state(&body, &config, 9, 2).unwrap(),
        Some(state)
    );
    assert!(live_captured_state(&body, &config, 8, 2).is_err());
    assert!(live_captured_state(&body, &config, 9, 1).is_err());
    for end in 0..body.len() {
        assert!(live_captured_state(&body[..end], &config, 9, 2).is_err());
    }
    for offset in [2, 10, 18, 26, 34, 42, 50, 58, 59, 63, 67, 87, 119] {
        let mut invalid = body.clone();
        invalid[offset] ^= 128;
        assert!(
            live_captured_state(&invalid, &config, 9, 2).is_err(),
            "offset {offset}"
        );
    }
    body[58] = 0;
    assert!(
        live_captured_state(&body, &config, 9, 2).is_err(),
        "pending cannot carry bytes"
    );
}

#[test]
fn stopped_restore_changes_only_instrument_quiescence_version() {
    let mut config = config();
    let ordinary = quiesce(&config, 3).unwrap();
    assert_eq!(quiesce_for_stopped_restore(&config, 3).unwrap(), ordinary);
    config.input_channels = 0;
    assert_eq!(quiesce(&config, 3).unwrap(), ordinary);
    let mut release = ordinary;
    release[4..6].copy_from_slice(&2_u16.to_le_bytes());
    assert_eq!(quiesce_for_stopped_restore(&config, 3).unwrap(), release);
    assert!(quiesce_for_stopped_restore(&config, u64::MAX).is_err());
}

#[test]
fn quiescent_capture_binds_source_pcm_prefix_and_operation_before_copying_state() {
    let config = config();
    let state = hex_plugin_state::encode(State {
        identity: config.identity,
        component: Some(b"current component"),
        controller: Some(b"current controller"),
    })
    .unwrap();
    let request = capture_request(&config, 3, 2).unwrap();
    assert_eq!(&request[..4], b"HXVC");
    assert_eq!(&request[48..56], &3_u64.to_le_bytes());
    assert_eq!(&request[56..64], &24_u64.to_le_bytes());
    assert_eq!(&request[64..], &2_u64.to_le_bytes());
    assert!(capture_request(&config, 3, 0).is_err());
    assert!(capture_request(&config, u64::MAX, 1).is_err());
    let mut body = Vec::new();
    stamp(&mut body, config.binding);
    for word in [3_u64, 24, 2] {
        body.extend(word.to_le_bytes());
    }
    body.extend((state.payload().len() as u32).to_le_bytes());
    body.extend(state.payload());
    assert_eq!(captured_state(&body, &config, 3, 2).unwrap(), state);
    assert!(captured_state(&body, &config, 3, 1).is_err());
    assert!(captured_state(&body, &config, 2, 2).is_err());
    let mut other = config.clone();
    other.binding.epoch += 1;
    assert!(captured_state(&body, &other, 3, 2).is_err());
    other = config.clone();
    other.identity.binary.info_sha256[0] ^= 1;
    assert!(captured_state(&body, &other, 3, 2).is_err());
    for end in 0..body.len() {
        assert!(captured_state(&body[..end], &config, 3, 2).is_err());
    }
    body.push(0);
    assert!(captured_state(&body, &config, 3, 2).is_err());
    let absent = hex_plugin_state::encode(State {
        identity: config.identity,
        component: None,
        controller: Some(b"controller only"),
    })
    .unwrap();
    body.truncate(58);
    body.extend((absent.payload().len() as u32).to_le_bytes());
    body.extend(absent.payload());
    assert!(
        captured_state(&body, &config, 3, 2).is_err(),
        "never replace confirmed component state with absence"
    );
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
    ready_body_with_state(config, &state)
}
fn ready_body_with_state(config: &Configuration, state: &NativeDeviceState) -> Vec<u8> {
    let mut out = Vec::new();
    stamp(&mut out, config.binding);
    out.extend(config.identity.class.bytes());
    out.extend(config.sample_rate.to_le_bytes());
    out.extend(config.quantum.to_le_bytes());
    out.extend([config.input_channels, config.output_channels]);
    out.extend(u32::MAX.to_le_bytes());
    out.extend(0_u32.to_le_bytes());
    out.extend([1, 1]); // applied, actual contributions supplied
    out.extend(77_u32.to_le_bytes());
    out.extend(91_u32.to_le_bytes());
    out.push(u8::from(config.restore.is_some()));
    out.push(1); // component state exists
    out.extend((state.payload().len() as u32).to_le_bytes());
    out.extend(state.payload());
    out
}

#[cfg(feature = "allocation-probe")]
#[test]
fn protocol_frames_allocate_once_with_exact_bounded_capacity_and_capture_fits() {
    let mut config = config();
    config.bundle = "b".repeat(4096);
    config.executable = "e".repeat(4096);
    let component = vec![0; hex_plugin_state::MAX_OPAQUE_BYTES];
    let state = hex_plugin_state::encode(State {
        identity: config.identity,
        component: Some(&component),
        controller: None,
    })
    .unwrap();
    assert_eq!(state.payload().len(), hex_plugin_state::MAX_ENVELOPE_BYTES);
    config.restore = Some(state.clone());
    let body = vec![0; CONTROL_LIMIT];
    for presentation in [None, known()] {
        for kind in 0..5 {
            let mut frame = None;
            let measured = allocation_counter::measure(|| {
                frame = Some(
                    match kind {
                        0 => config.encode(),
                        1 => config.startup_request(),
                        2 => presentation_request(config.binding, presentation),
                        3 => resume_request(
                            &config,
                            Binding {
                                epoch: config.binding.epoch + 1,
                                ..config.binding
                            },
                            presentation,
                        ),
                        _ => control(*b"HXVIPR01", &body),
                    }
                    .unwrap(),
                );
            });
            let frame = frame.unwrap();
            assert_eq!(frame.capacity(), frame.len());
            assert!(frame.capacity() <= PACKET_LIMIT);
            assert_eq!((measured.count_total, measured.count_current), (1, 1));
            assert_eq!(
                (measured.bytes_total, measured.bytes_current),
                (frame.capacity() as u64, frame.capacity() as i64)
            );
        }
    }
    let ready_body = ready_body_with_state(&config, &state);
    let mut ready = None;
    let measured = allocation_counter::measure(|| {
        ready = Some(Ready::decode(&ready_body, &config, known()).unwrap());
    });
    let ready = ready.unwrap();
    assert_eq!(ready.initial_state.payload(), state.payload());
    // The Vec copy is reclaimed while the single captured Arc group remains.
    // Arc header/allocator layout is not guessed as DATA bytes.
    assert_eq!((measured.count_total, measured.count_current), (2, 1));
    assert_eq!(
        i64::try_from(measured.bytes_total).unwrap() - measured.bytes_current,
        state.payload().len() as i64
    );
    assert!(CONFIG_LIMIT + 2 * state.payload().len() <= MAX_WORKING_DATA_BYTES);
}

#[cfg(feature = "allocation-probe")]
#[test]
fn protocol_trailing_ready_and_oversized_frames_refuse_before_allocating() {
    let config = config();
    let mut body = ready_body(&config);
    body.push(0);
    let oversized = vec![0; CONTROL_LIMIT + 1];
    let measured = allocation_counter::measure(|| {
        assert!(matches!(
            Ready::decode(&body, &config, known()),
            Err(Error::Protocol("trailing control bytes"))
        ));
        assert!(control(*b"HXVIPR01", &oversized).is_err());
        assert!(buffer(usize::MAX).is_err());
    });
    assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
    assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
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
        0, 2, 34, 50, 58, 60, 61, 70, 71, 72, 76, 80, 81, 82, 84, 92, 108, 140,
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
    absent[70] = 0;
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
        parameters: crate::transport::Parameters::default(),
        timing: None,
        timing_changes: Default::default(),
    };
    config.validate_block(&block, 0).unwrap();
    let mut bytes = Vec::new();
    crate::transport::wire::write(&mut bytes, &block).unwrap();
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
            note_id: None,
        })
        .unwrap();
    assert!(config.validate_block(&event, 0).is_err());
    let mut instrument = config.clone();
    instrument.input_channels = 0;
    assert!(
        instrument.validate_block(&event, 0).is_err(),
        "instrument PCM input must be zero"
    );
    event.pcm.fill([0.0; 2]);
    instrument.validate_block(&event, 0).unwrap();
    event.events = Events::default();
    event
        .events
        .push(MidiEvent {
            frame: 0,
            bytes: [0xb0, 64, 127],
            len: 3,
            note_id: None,
        })
        .unwrap();
    assert!(instrument.validate_block(&event, 0).is_err());
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
    body.extend(0_u32.to_le_bytes());
    body.push(1);
    body.extend([0; 5]);
    assert_eq!(
        Facts::decode(&body, &config).unwrap(),
        Facts {
            algorithmic_latency_frames: u32::MAX,
            tail_frames: 0,
            presentation_available: true,
            restored: false
        }
    );
    for offset in [0, 2, 34, 50, 82, 114, 122, 124, 125, 134, 135, 136, 137] {
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
    let unknown = presentation_request(config.binding, PresentationPlan::AllowUnknown).unwrap();
    assert_eq!(unknown.len(), absent.len());
    assert_ne!(unknown, absent);
    let mut receipt = ready_body(&config);
    receipt[70..72].copy_from_slice(&[2, 0]);
    receipt.drain(72..80);
    assert_eq!(
        Ready::decode(&receipt, &config, PresentationPlan::AllowUnknown)
            .unwrap()
            .presentation,
        Presentation::Unknown
    );
    assert!(Ready::decode(&receipt, &config, None).is_err());
    assert!(Ready::decode(&receipt, &config, known()).is_err());
}

#[test]
fn startup_restore_requires_current_source_and_exact_facts_and_final_acknowledgement() {
    let mut config = config();
    let component = vec![42; 65_536];
    let state = hex_plugin_state::encode(State {
        identity: config.identity,
        component: Some(&component),
        controller: Some(&[7, 8]),
    })
    .unwrap();
    let original = state.clone();
    config.restore = Some(state);
    let request = config.startup_request().unwrap();
    assert_eq!(&request[..8], b"HXVIST02");
    assert_eq!(&request[46..51], &[1, 98, 0, 1, 0]); // presence + full source envelope size
    assert_eq!(&request[51..], original.payload());
    let mut facts = Vec::new();
    stamp(&mut facts, config.binding);
    facts.extend(config.identity.class.bytes());
    facts.extend(config.identity.binary.executable_sha256);
    facts.extend(config.identity.binary.info_sha256);
    facts.extend(config.sample_rate.to_le_bytes());
    facts.extend(config.quantum.to_le_bytes());
    facts.extend([config.input_channels, config.output_channels]);
    facts.extend(3_u32.to_le_bytes());
    facts.extend(0_u32.to_le_bytes());
    facts.push(1); // real optional presentation interface
    facts.extend(&request[46..]);
    assert!(Facts::decode(&facts, &config).unwrap().restored);
    let mut epoch = facts.clone();
    epoch.extend(3_u64.to_le_bytes());
    epoch.extend(24_u64.to_le_bytes());
    assert!(quiescent_facts(&epoch, &config, 3).unwrap().restored);
    let last = facts.len() - 1;
    facts[last] ^= 1;
    assert!(
        Facts::decode(&facts, &config).is_err(),
        "a different restored envelope cannot acknowledge the requested one"
    );
    let ready = ready_body(&config);
    assert!(Ready::decode(&ready, &config, known()).unwrap().restored);
    let mut missing_ack = ready;
    missing_ack[80] = 0;
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

#[test]
fn epoch_requests_match_independent_headers_and_refuse_stale_or_changed_owners() {
    let config = config();
    // Independent Python struct.pack fixtures, not this encoder's output.
    assert_eq!(
        quiesce(&config, 3).unwrap().as_slice(),
        hex(
            "48585651010000000000000000000000010000000000000002000000000000000300000000000000040000000000000003000000000000001800000000000000"
        )
    );
    let next = Binding {
        graph: 7,
        epoch: 6,
        ..config.binding
    };
    assert_eq!(
        resume_request(&config, next, known()).unwrap(),
        hex(
            "485856520100000000000100000000000100000000000000020000000000000003000000000000000400000000000000070000000000000006000000000000004d0000005b000000"
        )
    );
    assert_eq!(resume_request(&config, next, None).unwrap().len(), 64);
    for bad in [
        config.binding,
        Binding { root: 9, ..next },
        Binding {
            instance: 9,
            ..next
        },
        Binding { graph: 2, ..next },
        Binding { epoch: 0, ..next },
        Binding { epoch: 3, ..next },
    ] {
        assert!(resume_request(&config, bad, known()).is_err());
    }
    assert!(quiesce(&config, u64::MAX).is_err());
}
fn epoch_body(config: &Configuration, sequence: u64) -> Vec<u8> {
    let mut body = Vec::new();
    stamp(&mut body, config.binding);
    body.extend(config.identity.class.bytes());
    body.extend(config.identity.binary.executable_sha256);
    body.extend(config.identity.binary.info_sha256);
    body.extend(config.sample_rate.to_le_bytes());
    body.extend(config.quantum.to_le_bytes());
    body.extend([config.input_channels, config.output_channels]);
    body.extend(3_u32.to_le_bytes());
    body.extend(0_u32.to_le_bytes());
    body.push(1);
    body.extend(&config.startup_request().unwrap()[46..]);
    body.extend(sequence.to_le_bytes());
    body.extend((sequence * u64::from(config.quantum)).to_le_bytes());
    body
}
#[test]
fn reactivation_binds_source_completed_prefix_and_unique_operation() {
    let config = config();
    let request = reactivate(&config, 3, 2).unwrap();
    assert_eq!(&request[..4], b"HXVA");
    assert_eq!(
        &request[48..],
        &[
            3_u64.to_le_bytes(),
            24_u64.to_le_bytes(),
            2_u64.to_le_bytes()
        ]
        .concat()
    );
    assert!(reactivate(&config, 3, 0).is_err());
    assert!(reactivate(&config, u64::MAX, 2).is_err());
    let mut body = epoch_body(&config, 3);
    body.extend(2_u64.to_le_bytes());
    assert_eq!(
        reactivated_facts(&body, &config, 3, 2)
            .unwrap()
            .algorithmic_latency_frames,
        3
    );
    for end in 0..body.len() {
        assert!(reactivated_facts(&body[..end], &config, 3, 2).is_err());
    }
    for offset in [0, 32, 48, 80, 112, 120, 134, 135, 138, 146, 154] {
        let mut bad = body.clone();
        bad[offset] ^= 128;
        assert!(
            reactivated_facts(&bad, &config, 3, 2).is_err(),
            "offset {offset}"
        );
    }
    assert!(reactivated_facts(&body, &config, 2, 2).is_err());
    assert!(reactivated_facts(&body, &config, 3, 1).is_err());
    assert!(reactivated_facts(&body, &config, 3, 0).is_err());
    body.push(0);
    assert!(reactivated_facts(&body, &config, 3, 2).is_err());
}
#[test]
fn epoch_acknowledgements_bind_exact_source_prefix_facts_and_presentation() {
    let mut config = config();
    let body = epoch_body(&config, 3);
    let facts = quiescent_facts(&body, &config, 3).unwrap();
    assert_eq!(facts.algorithmic_latency_frames, 3);
    for at in [2, 34, 50, 82, 114, 122, 134, 135, 138, 146] {
        let mut bad = body.clone();
        bad[at] ^= 0x80;
        assert!(quiescent_facts(&bad, &config, 3).is_err());
    }
    assert!(quiescent_facts(&body, &config, 2).is_err());
    config.binding.epoch += 1;
    assert!(quiescent_facts(&body, &config, 3).is_err());
    let mut ready = epoch_body(&config, 3);
    ready.extend([1, 1]);
    ready.extend(77_u32.to_le_bytes());
    ready.extend(91_u32.to_le_bytes());
    assert_eq!(
        resumed(&ready, &config, 3, facts, known()).unwrap(),
        Presentation::Applied
    );
    assert!(resumed(&ready, &config, 3, facts, None).is_err());
    assert!(
        resumed(
            &ready,
            &config,
            3,
            Facts {
                algorithmic_latency_frames: 4,
                ..facts
            },
            known()
        )
        .is_err()
    );
    let mut unknown = epoch_body(&config, 3);
    unknown.extend([1, 0]); // applied cannot acknowledge unknown contributions
    assert!(resumed(&unknown, &config, 3, facts, None).is_err());
    ready.push(0);
    assert!(resumed(&ready, &config, 3, facts, known()).is_err());
}

#[test]
fn tail_facts_preserve_finite_zero_and_infinite_reports_and_fence_resumption() {
    let config = config();
    for tail in [0_u32, 96_000, u32::MAX] {
        let mut body = epoch_body(&config, 3);
        body[130..134].copy_from_slice(&tail.to_le_bytes());
        let facts = quiescent_facts(&body, &config, 3).unwrap();
        assert_eq!(facts.tail_frames, tail);
        let mut ready = ready_body(&config);
        ready[66..70].copy_from_slice(&tail.to_le_bytes());
        assert_eq!(
            Ready::decode(&ready, &config, known()).unwrap().tail_frames,
            tail
        );
        body.extend([1, 1]);
        body.extend(77_u32.to_le_bytes());
        body.extend(91_u32.to_le_bytes());
        assert!(resumed(&body, &config, 3, facts, known()).is_ok());
        assert!(
            resumed(
                &body,
                &config,
                3,
                Facts {
                    tail_frames: tail ^ 1,
                    ..facts
                },
                known()
            )
            .is_err()
        );
        // An old body with no tail field is not a current preparation or epoch.
        let mut old = body.clone();
        old.drain(130..134);
        assert!(quiescent_facts(&old, &config, 3).is_err());
    }
}

#[test]
fn stopped_flush_request_and_ack_bind_source_prefix_operation_and_complete_values() {
    use crate::transport::{ParameterPoint, Parameters};
    let config = config();
    let mut points = Parameters::default();
    points
        .push(ParameterPoint {
            frame: 0,
            id: 0,
            normalized: 0.75,
        })
        .unwrap();
    let request = flush_request(&config, 3, 2, &points).unwrap();
    assert_eq!(request.len(), 88);
    assert_eq!(&request[..12], b"HXVF\x02\0\0\0\0\0\x01\0");
    assert_eq!(
        &request[48..72],
        &[
            3_u64.to_le_bytes(),
            24_u64.to_le_bytes(),
            2_u64.to_le_bytes()
        ]
        .concat()
    );
    assert_eq!(&request[80..88], &0.75_f64.to_le_bytes());
    let mut body = Vec::new();
    stamp(&mut body, config.binding);
    body.extend(3_u64.to_le_bytes());
    body.extend(24_u64.to_le_bytes());
    body.extend(2_u64.to_le_bytes());
    body.push(1);
    body.extend(1_u16.to_le_bytes());
    body.extend(0_u32.to_le_bytes());
    body.extend(0.75_f64.to_le_bytes());
    let result = flushed_parameters(&body, &config, 3, 2, &points).unwrap();
    assert_eq!(result.disposition, FlushDisposition::Applied);
    assert_eq!(
        result.as_slice(),
        &[FlushedParameter {
            id: 0,
            normalized: 0.75
        }]
    );
    for end in 0..body.len() {
        assert!(flushed_parameters(&body[..end], &config, 3, 2, &points).is_err());
    }
    for (offset, value) in [
        (2, 9),
        (34, 4),
        (42, 25),
        (50, 3),
        (58, 0),
        (59, 129),
        (64, 128),
    ] {
        let mut bad = body.clone();
        bad[offset] = value;
        assert!(
            flushed_parameters(&bad, &config, 3, 2, &points).is_err(),
            "field{offset}"
        );
    }
    let mut bad = body.clone();
    bad[65..73].copy_from_slice(&f64::NAN.to_le_bytes());
    assert!(flushed_parameters(&bad, &config, 3, 2, &points).is_err());
    let mut bad = body.clone();
    bad[59..61].copy_from_slice(&2_u16.to_le_bytes());
    bad.extend_from_slice(&body[61..]);
    assert!(
        flushed_parameters(&bad, &config, 3, 2, &points).is_err(),
        "duplicate ID"
    );
    let empty = Parameters::default();
    body[58] = 0;
    body[59..61].fill(0);
    body.truncate(61);
    assert_eq!(
        flushed_parameters(&body, &config, 3, 2, &empty)
            .unwrap()
            .disposition,
        FlushDisposition::Empty
    );
    assert!(flushed_parameters(&body, &config, 3, 2, &points).is_err());
    assert!(flush_request(&config, 0, 0, &empty).is_err());
    let mut late = Parameters::default();
    late.push(ParameterPoint {
        frame: 1,
        id: 7,
        normalized: 0.5,
    })
    .unwrap();
    assert!(flush_request(&config, 0, 1, &late).is_err());
}

#[test]
fn recovery_edit_receipts_keep_exact_prefix_and_versioned_ordinal() {
    let config = config();
    let request = recovery_edit_request(&config, 2, 3).unwrap();
    assert_eq!(&request[..6], b"HXVG\x02\0");
    assert_eq!(&request[6..], &edit_request(&config, 2, 3).unwrap()[6..]);
    assert!(recovery_edit_request(&config, 2, 0).is_err());
    let mut body = Vec::new();
    stamp(&mut body, config.binding);
    for field in [2_u64, 16, 3] {
        body.extend(field.to_le_bytes());
    }
    body.extend([0; 6]);
    body.extend(9_u64.to_le_bytes());
    let response = recovery_edits(&body, &config, 2, 3, (0, 0)).unwrap();
    assert!(response.edits.as_slice().is_empty());
    assert_eq!(response.pending_latency_restart.unwrap().get(), 9);
    for end in 0..body.len() {
        assert!(recovery_edits(&body[..end], &config, 2, 3, (0, 0)).is_err());
    }
    for offset in [2, 10, 18, 26, 34, 42, 50, 58, 60, 62] {
        let mut bad = body.clone();
        bad[offset] ^= 128;
        assert!(recovery_edits(&bad, &config, 2, 3, (0, 0)).is_err());
    }
    assert!(parameter_edits(&body, &config, 2, 3, (0, 0)).is_err());
    body[64..].fill(0);
    assert!(
        recovery_edits(&body, &config, 2, 3, (0, 0))
            .unwrap()
            .pending_latency_restart
            .is_none()
    );
}

#[test]
fn ordered_sdk_group_markers_are_distinct_from_raw_zero() {
    let config = config();
    let mut body = Vec::new();
    stamp(&mut body, config.binding);
    for value in [2_u64, 16, 3] {
        body.extend(value.to_le_bytes());
    }
    for value in [0_u16, 0, 5] {
        body.extend(value.to_le_bytes());
    }
    let rows = [
        (1_u64, 1_u64, 0.0_f64, 4_u32, 0_u64),
        (2, 2, 0.0, 1, 0),
        (3, 2, 0.75, 2, 8),
        (4, 2, 0.0, 3, 16),
        (5, 1, 0.0, 5, 16),
    ];
    for (sequence, gesture, value, kind, frame) in rows {
        body.extend(sequence.to_le_bytes());
        body.extend(gesture.to_le_bytes());
        body.extend(value.to_le_bytes());
        body.extend(0_u32.to_le_bytes());
        body.extend(kind.to_le_bytes());
        body.extend(frame.to_le_bytes());
    }
    let parsed = parameter_edits(&body, &config, 2, 3, (0, 0)).unwrap();
    let rows = parsed.as_slice();
    assert_eq!(rows[0].kind, ParameterEditKind::GroupBegin);
    assert_eq!(rows[2].kind, ParameterEditKind::Perform);
    assert_eq!((rows[2].id, rows[2].normalized), (0, 0.75));
    assert_eq!(rows[4].kind, ParameterEditKind::GroupEnd);
    assert_eq!(rows[0].gesture, rows[4].gesture);
    assert_ne!(rows[0].gesture, rows[2].gesture);
    let mut malformed = body.clone();
    // The first marker must have an empty ParamID field.
    malformed[64 + 24..64 + 28].copy_from_slice(&11_u32.to_le_bytes());
    assert!(parameter_edits(&malformed, &config, 2, 3, (0, 0)).is_err());
}

#[test]
fn edit_receipts_bind_source_prefix_operation_order_and_gesture_without_coalescing() {
    let config = config();
    let request = edit_request(&config, 2, 3).unwrap();
    assert_eq!(&request[..4], b"HXVG");
    let mut body = Vec::new();
    stamp(&mut body, config.binding);
    body.extend(2_u64.to_le_bytes());
    body.extend(16_u64.to_le_bytes());
    body.extend(3_u64.to_le_bytes());
    body.extend(0_u16.to_le_bytes());
    body.extend(0_u16.to_le_bytes());
    body.extend(3_u16.to_le_bytes());
    for (sequence, kind, value, frame) in [
        (4_u64, 1_u32, 0.0_f64, 0_u64),
        (5, 2, 0.75, 8),
        (6, 3, 0.0, 8),
    ] {
        body.extend(sequence.to_le_bytes());
        body.extend(4_u64.to_le_bytes());
        body.extend(value.to_le_bytes());
        body.extend(0_u32.to_le_bytes());
        body.extend(kind.to_le_bytes());
        body.extend(frame.to_le_bytes());
    }
    let result = parameter_edits(&body, &config, 2, 3, (0, 0)).unwrap();
    assert_eq!(result.as_slice().len(), 3);
    assert_eq!(result.as_slice()[1].kind, ParameterEditKind::Perform);
    assert_eq!(result.as_slice()[1].id, 0);
    let mut backlogged = body.clone();
    backlogged[58..60].copy_from_slice(&72_u16.to_le_bytes());
    let backlog = parameter_edits(&backlogged, &config, 2, 3, (0, 0)).unwrap();
    assert_eq!(backlog.pending, 72);
    for end in 0..body.len() {
        assert!(parameter_edits(&body[..end], &config, 2, 3, (0, 0)).is_err());
    }
    for (offset, value) in [
        (2, 2),
        (34, 1),
        (42, 15),
        (50, 2),
        (58, 73),
        (60, 1),
        (62, 9),
        (64, 0),
        (72, 0),
        (92, 7),
        (96, 17),
    ] {
        let mut bad = body.clone();
        bad[offset] = value;
        assert!(
            parameter_edits(&bad, &config, 2, 3, (0, 0)).is_err(),
            "field{offset}"
        );
    }
    assert!(
        parameter_edits(&body, &config, 2, 3, (4, 0)).is_err(),
        "replayed sequence"
    );
    assert!(
        parameter_edits(&body, &config, 2, 3, (0, 1)).is_err(),
        "time reversal across polls"
    );
    let mut bad = body;
    bad[80..88].copy_from_slice(&f64::NAN.to_le_bytes());
    assert!(parameter_edits(&bad, &config, 2, 3, (0, 0)).is_err());
}

#[test]
fn controller_pages_preserve_sdk_ids_text_and_explicit_conversion_states() {
    fn text(out: &mut Vec<u8>, value: &str) {
        let mut units = [0_u16; 128];
        for (slot, unit) in units.iter_mut().zip(value.encode_utf16()) {
            *slot = unit;
        }
        for unit in units {
            out.extend(unit.to_le_bytes());
        }
    }
    fn record(out: &mut Vec<u8>, id: u32, flags: u32) {
        for value in [id, flags, 3, 0] {
            out.extend(value.to_le_bytes());
        }
        out.extend(0.0_f64.to_le_bytes());
        out.extend(0.25_f64.to_le_bytes());
        out.push(1); // Explicit negative infinity, not unchecked numeric input.
        out.extend(0_u64.to_le_bytes());
        out.push(0);
        out.extend(1.0_f64.to_le_bytes());
        text(out, "gain 𝄞");
        text(out, "gain");
        text(out, "db");
        out.push(3);
        text(out, "−∞ db");
        text(out, "sdk default");
    }
    let config = config();
    let query = ParameterPageQuery {
        first: 0,
        revision: 9,
        conversion: None,
    };
    let request = parameter_page_request(&config, 1, 7, query, false).unwrap();
    assert_eq!(&request[..4], b"HXVM");
    assert_eq!(&request[64..72], &7_u64.to_le_bytes());
    assert_eq!(&request[72..74], &0_u16.to_le_bytes());
    assert_eq!(&request[74..82], &9_u64.to_le_bytes());
    assert!(request[82..].iter().all(|byte| *byte == 0));
    assert_eq!(
        &parameter_page_request(&config, 1, 7, query, true).unwrap()[..4],
        b"HXVN"
    );
    let mut prefix = Vec::new();
    stamp(&mut prefix, config.binding);
    for value in [1_u64, 8, 7] {
        prefix.extend(value.to_le_bytes());
    }
    let mut pending = prefix.clone();
    pending.push(0);
    assert_eq!(
        parameter_page(&pending, &config, 1, 7, query).unwrap(),
        ParameterPageProgress::Pending
    );
    let mut body = prefix.clone();
    body.push(1);
    body.extend(9_u64.to_le_bytes());
    body.extend(1_u16.to_le_bytes());
    body.extend(0_u16.to_le_bytes());
    body.push(1);
    record(&mut body, 0, 1);
    body.extend(0_u8..32);
    let selected = ControllerRequest::SelectedParameters { first: 8 };
    let encoded = parameter_page_request(&config, 1, 7, selected, false).unwrap();
    assert_eq!(&encoded[72..74], &8_u16.to_le_bytes());
    assert!(encoded[74..82].iter().all(|byte| *byte == 0));
    assert_eq!(encoded[82], 8);
    assert!(encoded[83..].iter().all(|byte| *byte == 0));
    let mut fresh = body.clone();
    fresh[59..67].copy_from_slice(&11_u64.to_le_bytes());
    fresh[67..69].copy_from_slice(&9_u16.to_le_bytes());
    fresh[69..71].copy_from_slice(&8_u16.to_le_bytes());
    let ParameterPageProgress::Page(selected_page) =
        parameter_page(&fresh, &config, 1, 7, selected).unwrap()
    else {
        panic!("fresh selected page")
    };
    assert_eq!((selected_page.revision, selected_page.first), (11, 8));
    assert_eq!(selected_page.as_slice()[0].id, 0);
    assert!(parameter_page(&fresh, &config, 1, 7, query).is_err());
    assert!(
        parameter_page(
            &fresh,
            &config,
            1,
            7,
            ControllerRequest::SelectedParameters { first: 4 }
        )
        .is_err()
    );
    assert!(
        parameter_page_request(
            &config,
            1,
            7,
            ControllerRequest::SelectedParameters {
                first: MAX_CONTROLLER_PARAMETERS + 1
            },
            false
        )
        .is_err()
    );
    let ParameterPageProgress::Page(page) = parameter_page(&body, &config, 1, 7, query).unwrap()
    else {
        panic!("completed metadata page")
    };
    assert_eq!((page.revision, page.total, page.first), (9, 1, 0));
    assert_eq!(page.descriptor_signature, std::array::from_fn(|n| n as u8));
    let value = page.as_slice()[0];
    assert_eq!(value.id, 0);
    assert!(value.is_editable());
    assert!(value.supports_authored_automation());
    assert_eq!(value.plain, ParameterPlainValue::NegativeInfinity);
    assert_eq!(value.default_plain, ParameterPlainValue::Finite(1.0));
    assert_eq!(
        value.title.as_utf16(),
        "gain 𝄞".encode_utf16().collect::<Vec<_>>()
    );
    assert_eq!(
        value.display.unwrap().as_utf16(),
        "−∞ db".encode_utf16().collect::<Vec<_>>()
    );
    let mut unknown = body.clone();
    unknown[76..80].copy_from_slice(&(1_u32 << 29).to_le_bytes());
    let ParameterPageProgress::Page(page) = parameter_page(&unknown, &config, 1, 7, query).unwrap()
    else {
        panic!("unknown flags remain observable")
    };
    assert_eq!(page.as_slice()[0].flags, 1 << 29);
    assert!(!page.as_slice()[0].is_editable());

    // Four records fit the existing control bound without truncating strings.
    let mut full = body[..72].to_vec();
    full[67..69].copy_from_slice(&4_u16.to_le_bytes());
    full[71] = 4;
    for id in [9, 0, 3, 17] {
        record(&mut full, id, 1);
    }
    full.extend(0_u8..32);
    assert!(full.len() < CONTROL_LIMIT);
    let ParameterPageProgress::Page(page) = parameter_page(&full, &config, 1, 7, query).unwrap()
    else {
        panic!("full page")
    };
    assert_eq!(
        page.as_slice().iter().map(|p| p.id).collect::<Vec<_>>(),
        [9, 0, 3, 17]
    );

    for end in 0..body.len() {
        assert!(parameter_page(&body[..end], &config, 1, 7, query).is_err());
    }
    let mut oversized = body.clone();
    oversized[67..69].copy_from_slice(&(MAX_CONTROLLER_PARAMETERS + 1).to_le_bytes());
    assert!(parameter_page(&oversized, &config, 1, 7, query).is_err());
    for (offset, value) in [
        (2, 8),
        (34, 2),
        (42, 7),
        (50, 8),
        (59, 8),
        (69, 1),
        (71, 5),
        (75, 128),
        (76, 3),
        (83, 128),
        (104, 4),
        (105, 1),
        (122, 0),
        (890, 4),
    ] {
        let mut bad = body.clone();
        bad[offset] = value;
        assert!(
            parameter_page(&bad, &config, 1, 7, query).is_err(),
            "field{offset}"
        );
    }
    for (offset, value) in [(88, f64::NAN), (96, f64::INFINITY), (96, 1.1)] {
        let mut bad = body.clone();
        bad[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        assert!(parameter_page(&bad, &config, 1, 7, query).is_err());
    }
    let mut invalid_utf16 = body.clone();
    invalid_utf16[122..124].copy_from_slice(&0xD800_u16.to_le_bytes());
    assert!(parameter_page(&invalid_utf16, &config, 1, 7, query).is_err());
    let mut changed = prefix;
    changed.push(2);
    changed.extend(10_u64.to_le_bytes());
    changed.extend(1_u16.to_le_bytes());
    assert_eq!(
        parameter_page(&changed, &config, 1, 7, query).unwrap(),
        ParameterPageProgress::Changed {
            revision: 10,
            total: 1
        }
    );
    changed[59..67].copy_from_slice(&9_u64.to_le_bytes());
    assert!(parameter_page(&changed, &config, 1, 7, query).is_err());
    assert!(parameter_page_request(&config, 1, 0, query, false).is_err());
    assert!(parameter_page_request(&config, u64::MAX, 7, query, false).is_err());
    assert!(
        parameter_page_request(
            &config,
            1,
            7,
            ParameterPageQuery {
                first: 1,
                revision: 0,
                conversion: None,
            },
            false
        )
        .is_err()
    );
}

fn refusal_prefix(config: &Configuration, sequence: u64, operation: u64, reason: u8) -> Vec<u8> {
    let mut body = Vec::new();
    stamp(&mut body, config.binding);
    let frames = sequence * u64::from(config.quantum);
    for word in [sequence, frames, operation] {
        body.extend(word.to_le_bytes());
    }
    body.push(reason);
    body
}

fn refusal_body(config: &Configuration, sequence: u64, operation: u64, observed: u32) -> Vec<u8> {
    let mut body = refusal_prefix(config, sequence, operation, 1);
    body.extend(observed.to_le_bytes());
    body
}

#[test]
fn capture_refusal_binds_exact_operation_and_reports_lowercase_limit() {
    let config = config();
    let limit = hex_plugin_state::MAX_OPAQUE_BYTES as u32;
    let body = refusal_body(&config, 3, 2, limit + 1);
    let refusal = capture_refusal(&body, &config, 3, 2).unwrap();
    assert_eq!(
        refusal,
        StateCaptureRefusal::ExceedsLimit {
            observed_bytes: limit + 1
        }
    );
    let message = crate::Error::CaptureRefused(refusal).to_string();
    assert_eq!(
        message,
        "plugin state exceeds the 524192-byte limit · not saved · plugin keeps running"
    );
    // Stale operation, PCM prefix, epoch and identity mismatches are malformed.
    assert!(capture_refusal(&body, &config, 3, 1).is_err());
    assert!(capture_refusal(&body, &config, 2, 2).is_err());
    assert!(capture_refusal(&body, &config, 3, 0).is_err());
    let mut other = config.clone();
    other.binding.epoch += 1;
    assert!(capture_refusal(&body, &other, 3, 2).is_err());
    for end in 0..body.len() {
        assert!(capture_refusal(&body[..end], &config, 3, 2).is_err());
    }
    let mut trailing = body.clone();
    trailing.push(0);
    assert!(capture_refusal(&trailing, &config, 3, 2).is_err());
    let mut reason = body.clone();
    reason[58] = 3;
    assert!(
        capture_refusal(&reason, &config, 3, 2).is_err(),
        "unknown reason"
    );
    for observed in [0, limit] {
        assert!(
            capture_refusal(&refusal_body(&config, 3, 2, observed), &config, 3, 2).is_err(),
            "a state that fits is never refused as oversized: {observed}"
        );
    }
}

#[test]
fn still_changing_refusal_is_exact_fieldless_and_versioned() {
    let config = config();
    let body = refusal_prefix(&config, 3, 2, 2);
    let refusal = capture_refusal(&body, &config, 3, 2).unwrap();
    assert_eq!(refusal, StateCaptureRefusal::StillChanging);
    assert_eq!(
        crate::Error::CaptureRefused(refusal).to_string(),
        "plugin state kept changing during capture · not saved · plugin keeps running"
    );
    // Reason 2 carries no fields; a stale operation or prefix is malformed.
    let mut trailing = body.clone();
    trailing.extend(0_u32.to_le_bytes());
    assert!(capture_refusal(&trailing, &config, 3, 2).is_err());
    assert!(capture_refusal(&body, &config, 3, 1).is_err());
    assert!(capture_refusal(&body, &config, 4, 2).is_err());
    // The version 01 frame (reason 1 only) is no longer a refusal frame.
    assert_eq!(&CAPTURE_REFUSAL_MAGIC, b"HXVICR02");
}

#[test]
fn live_capture_reply_separates_refusal_from_terminal_frames() {
    let config = config();
    let refusal = refusal_prefix(&config, 9, 2, 2);
    assert!(matches!(
        live_capture_reply(&CAPTURE_REFUSAL_MAGIC, &refusal, &config, 9, 2),
        Err(crate::Error::CaptureRefused(
            StateCaptureRefusal::StillChanging
        ))
    ));
    // A refusal for another operation is malformed, never a retirement.
    assert!(matches!(
        live_capture_reply(&CAPTURE_REFUSAL_MAGIC, &refusal, &config, 9, 3),
        Err(crate::Error::Protocol(_))
    ));
    assert!(matches!(
        live_capture_reply(b"HXVICR01", &refusal, &config, 9, 2),
        Err(crate::Error::Protocol(
            "missing live capture acknowledgement"
        ))
    ));
    let mut pending = Vec::new();
    stamp(&mut pending, config.binding);
    for word in [9_u64, 72, 2] {
        pending.extend(word.to_le_bytes());
    }
    pending.extend([0; 5]);
    assert!(matches!(
        live_capture_reply(b"HXVLCA02", &pending, &config, 9, 2),
        Ok(None)
    ));
}

#[test]
fn refusal_frame_reads_through_the_channel_and_keeps_its_successor() {
    use crate::channel::{Channel, Packet, end_after};
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    let config = config();
    let (stream, mut peer) = UnixStream::pair().unwrap();
    let refusal = refusal_body(&config, 9, 2, 524288);
    peer.write_all(&control(CAPTURE_REFUSAL_MAGIC, &refusal).unwrap())
        .unwrap();
    let mut pending = Vec::new();
    stamp(&mut pending, config.binding);
    for word in [9_u64, 72, 3] {
        pending.extend(word.to_le_bytes());
    }
    pending.extend([0; 5]);
    peer.write_all(&control(*b"HXVLCA02", &pending).unwrap())
        .unwrap();
    drop(peer);
    let mut channel = Channel::new(stream).unwrap();
    let mut packet = Packet::new().unwrap();
    let until = end_after(std::time::Duration::from_secs(2)).unwrap();
    channel.read(&mut packet, until, &mut || Ok(())).unwrap();
    assert_eq!(packet.magic(), CAPTURE_REFUSAL_MAGIC);
    assert_eq!(
        capture_refusal(packet.body().unwrap(), &config, 9, 2).unwrap(),
        StateCaptureRefusal::ExceedsLimit {
            observed_bytes: 524288
        }
    );
    channel.read(&mut packet, until, &mut || Ok(())).unwrap();
    assert_eq!(packet.magic(), b"HXVLCA02");
    assert_eq!(
        live_captured_state(packet.body().unwrap(), &config, 9, 3).unwrap(),
        None,
        "the next capture operation still exchanges on the same channel"
    );
}

#[test]
fn authored_parameter_permission_is_stricter_than_edit_permission() {
    for (flags, editable, automatable) in [
        (0, true, false),
        (1, true, true),
        (2, false, false),
        (2 | 16, false, false),
        (1 | 1 << 15, true, false),
        (1 | 1 << 16, true, true),
        (1 | 1 << 29, false, false),
    ] {
        let parameter = ControllerParameter {
            flags,
            ..ControllerParameter::default()
        };
        assert_eq!(parameter.is_editable(), editable, "flags {flags:#x}");
        assert_eq!(
            parameter.supports_authored_automation(),
            automatable,
            "flags {flags:#x}"
        );
    }
}
