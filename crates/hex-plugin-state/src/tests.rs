use super::*;

fn identity() -> Identity {
    Identity {
        class: ClassId::new([0x19; 16]).unwrap(),
        binary: BinaryIdentity {
            executable_sha256: [0xab; 32],
            info_sha256: [0xcd; 32],
        },
    }
}
fn envelope(component: Option<&[u8]>, controller: Option<&[u8]>) -> NativeDeviceState {
    encode(State {
        identity: identity(),
        component,
        controller,
    })
    .unwrap()
}

#[test]
fn exact_maximum_chunks_and_host_metadata_remain_separate() {
    let bytes: Vec<_> = (0..MAX_OPAQUE_BYTES).map(|n| n as u8).collect();
    let state = envelope(Some(&bytes[..1999]), Some(&bytes[1999..]))
        .with_input_gain_millidb(-2500)
        .unwrap()
        .with_output_gain_millidb(3300)
        .unwrap();
    assert_eq!(state.payload().len(), MAX_NATIVE_DEVICE_STATE_BYTES);
    let decoded = available(&state, Some(identity())).unwrap();
    assert_eq!(decoded.identity, identity());
    assert_eq!(decoded.component, Some(&bytes[..1999]));
    assert_eq!(decoded.controller, Some(&bytes[1999..]));
    assert_eq!(encode(decoded).unwrap().payload(), state.payload());
    assert_eq!(state.input_gain_millidb(), -2500);
    assert_eq!(state.output_gain_millidb(), 3300);
}

#[test]
fn missing_and_present_empty_streams_are_distinct_and_refusals_retain_bytes() {
    for component in [None, Some(&[][..])] {
        for controller in [None, Some(&[][..])] {
            let state = envelope(component, controller);
            let before = state.clone();
            assert_eq!(
                decode(&state).unwrap(),
                State {
                    identity: identity(),
                    component,
                    controller
                }
            );
            if component.is_none() {
                assert_eq!(
                    available(&state, Some(identity())),
                    Err(Error::MissingComponent)
                );
            } else {
                assert!(available(&state, Some(identity())).is_ok());
                assert_eq!(available(&state, None), Err(Error::MissingBinary));
            }
            assert_eq!(state, before);
        }
    }
}

#[test]
fn binary_and_class_changes_never_rebind_state_by_name_path_or_row() {
    let state = envelope(Some(b"component"), Some(b"controller"));
    let before = state.clone();
    let mut changed = identity();
    changed.binary.executable_sha256[0] ^= 1;
    assert_eq!(available(&state, Some(changed)), Err(Error::BinaryChanged));
    changed = identity();
    changed.binary.info_sha256[0] ^= 1;
    assert_eq!(available(&state, Some(changed)), Err(Error::BinaryChanged));
    changed = identity();
    changed.class = ClassId::new([0x20; 16]).unwrap();
    assert_eq!(available(&state, Some(changed)), Err(Error::ClassChanged));
    assert_eq!(available(&state, None), Err(Error::MissingBinary));
    assert_eq!(state, before);
    assert_eq!(ClassId::new([0; 16]), Err(Error::InvalidClass));
}

#[test]
fn malformed_unknown_and_declared_oversize_envelopes_refuse_without_rewriting() {
    let state = envelope(Some(&[0, 255]), Some(&[3, 0, 17]));
    for length in 0..state.payload().len() {
        let truncated =
            NativeDeviceState::new(TYPE_ID, 1, state.payload()[..length].to_vec()).unwrap();
        assert!(decode(&truncated).is_err(), "prefix {length}");
    }
    for (offset, byte) in [(0, 0), (5, 4), (6, 1), (7, 1), (5, 0)] {
        let mut bytes = state.payload().to_vec();
        bytes[offset] = byte;
        let original = bytes.clone();
        let malformed = NativeDeviceState::new(TYPE_ID, 1, bytes).unwrap();
        assert_eq!(decode(&malformed), Err(Error::Malformed));
        assert_eq!(malformed.payload(), original);
    }
    for (offset, expected) in [(4, Error::UnsupportedPlatform(2)), (88, Error::TooLarge)] {
        let mut bytes = state.payload().to_vec();
        if offset == 4 {
            bytes[4] = 2;
        } else {
            bytes[88..92].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        let malformed = NativeDeviceState::new(TYPE_ID, 1, bytes.clone()).unwrap();
        assert_eq!(available(&malformed, Some(identity())), Err(expected));
        assert_eq!(malformed.payload(), bytes);
    }
    let future = NativeDeviceState::new(TYPE_ID, 77, state.payload().to_vec()).unwrap();
    assert_eq!(decode(&future), Err(Error::UnsupportedVersion(77)));
    let other =
        NativeDeviceState::new(DeviceTypeId::from_static("other.device"), 1, vec![]).unwrap();
    assert_eq!(decode(&other), Err(Error::WrongType));
    let mut bytes = state.payload().to_vec();
    bytes[8..24].fill(0);
    assert_eq!(
        decode(&NativeDeviceState::new(TYPE_ID, 1, bytes).unwrap()),
        Err(Error::InvalidClass)
    );
    let mut bytes = state.payload().to_vec();
    bytes.push(1);
    assert_eq!(
        decode(&NativeDeviceState::new(TYPE_ID, 1, bytes).unwrap()),
        Err(Error::Malformed)
    );
}

#[test]
fn oversize_capture_preserves_both_caller_buffers() {
    let component = vec![71; MAX_OPAQUE_BYTES];
    let controller = vec![19];
    assert_eq!(
        encode(State {
            identity: identity(),
            component: Some(&component),
            controller: Some(&controller)
        }),
        Err(Error::TooLarge)
    );
    assert_eq!(component, vec![71; MAX_OPAQUE_BYTES]);
    assert_eq!(controller, [19]);
    assert_eq!(
        encode(State {
            identity: identity(),
            component: Some(&controller),
            controller: Some(&component)
        }),
        Err(Error::TooLarge)
    );
}

#[test]
fn independent_wire_fixture_fixes_uid_bytes_presence_and_little_endian_lengths() {
    let mut expected = b"HXVS\x01\x03\x00\x00".to_vec();
    expected.extend(0_u8..16);
    expected.extend(32_u8..64);
    expected.extend(64_u8..96);
    expected.extend([2, 1, 0, 0, 3, 0, 0, 0]); // component258, controller3
    expected.extend((0..258).map(|n| n as u8));
    expected.extend([0, 255, 17]);
    let fixture = NativeDeviceState::new(TYPE_ID, 1, expected.clone()).unwrap();
    let decoded = decode(&fixture).unwrap();
    assert_eq!(
        decoded.identity.class.bytes(),
        std::array::from_fn(|n| n as u8)
    );
    assert_eq!(
        decoded.identity.binary.executable_sha256,
        std::array::from_fn(|n| n as u8 + 32)
    );
    assert_eq!(
        decoded.identity.binary.info_sha256,
        std::array::from_fn(|n| n as u8 + 64)
    );
    assert_eq!(
        decoded.component.unwrap(),
        (0..258).map(|n| n as u8).collect::<Vec<_>>()
    );
    assert_eq!(decoded.controller, Some(&[0, 255, 17][..]));
    assert_eq!(encode(decoded).unwrap().payload(), expected);
}
