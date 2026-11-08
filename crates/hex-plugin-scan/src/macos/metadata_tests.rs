//! Copied optional factory metadata; no native code or process ownership.
use crate::{PUBLIC_SDK, SDK, decode_packet_for_platform};
fn text(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
    bytes.extend_from_slice(value);
}
fn packet(marker: u8, categories: &[u8]) -> Vec<u8> {
    packet_with_classes(marker, categories, &[b"Audio Module Class"])
}
fn packet_with_classes(marker: u8, categories: &[u8], classes: &[&[u8]]) -> Vec<u8> {
    let mut body = vec![2, 0];
    body.extend_from_slice(&1_u64.to_le_bytes());
    for value in [SDK, PUBLIC_SDK, "darwin-arm64"] {
        text(&mut body, value.as_bytes());
    }
    body.extend_from_slice(&0_u32.to_le_bytes());
    for value in ["factory vendor", "", ""] {
        text(&mut body, value.as_bytes());
    }
    body.extend_from_slice(&(classes.len() as u32).to_le_bytes());
    for class in classes {
        body.extend_from_slice(&[33; 16]);
        body.extend_from_slice(&1_i32.to_le_bytes());
        text(&mut body, class);
        text(&mut body, b"original class");
        body.push(marker);
        if marker == 1 {
            body.extend_from_slice(&0x8000_0000_u32.to_le_bytes());
            for value in [categories, b"class vendor", b"1.2.3", b"VST 3.8"] {
                text(&mut body, value);
            }
        }
    }
    let mut bytes = b"HVSCAN01".to_vec();
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes
}
#[test]
fn optional_metadata_preserves_bytes_and_refuses_invalid_extension_frames() {
    let good = packet(1, b"Fx|Instrument");
    let discovery = decode_packet_for_platform(&good, 1, "darwin-arm64").unwrap();
    let class = &discovery.classes[0];
    assert_eq!(class.subcategories.as_deref(), Some("Fx|Instrument"));
    assert_eq!(class.class_vendor.as_deref(), Some("class vendor"));
    assert_eq!(class.class_flags, Some(0x8000_0000));
    assert_eq!(class.class_version.as_deref(), Some("1.2.3"));
    assert_eq!(class.class_sdk_version.as_deref(), Some("VST 3.8"));
    let absent = decode_packet_for_platform(&packet(0, b""), 1, "darwin-arm64").unwrap();
    assert!(absent.classes[0].subcategories.is_none());
    for n in 0..good.len() {
        assert!(decode_packet_for_platform(&good[..n], 1, "darwin-arm64").is_err());
    }
    for malformed in [
        packet(2, b""),
        packet(1, &[0xff]),
        packet(1, b"Fx\0Instrument"),
        packet(1, &[b'x'; 129]),
    ] {
        assert!(decode_packet_for_platform(&malformed, 1, "darwin-arm64").is_err());
    }
    let mut obsolete = good.clone();
    obsolete[12] = 1;
    assert!(decode_packet_for_platform(&obsolete, 1, "darwin-arm64").is_err());
    assert!(decode_packet_for_platform(&good, 1, "linux-x86_64").is_err());
}

#[test]
fn compatibility_service_does_not_claim_the_processor_identity() {
    let processor = b"Audio Module Class".as_slice();
    let compatibility = b"Plugin Compatibility Class".as_slice();
    for classes in [[processor, compatibility], [compatibility, processor]] {
        let bytes = packet_with_classes(1, b"Fx|Distortion", &classes);
        let discovery = decode_packet_for_platform(&bytes, 1, "darwin-arm64").unwrap();
        assert_eq!(discovery.classes.len(), 1);
        assert_eq!(discovery.classes[0].cid, [33; 16]);
        assert_eq!(discovery.classes[0].category, "Audio Module Class");
        assert_eq!(
            discovery.classes[0].subcategories.as_deref(),
            Some("Fx|Distortion")
        );
    }
    for classes in [
        [processor, processor],
        [processor, b"Component Controller Class"],
    ] {
        let bytes = packet_with_classes(1, b"", &classes);
        assert_eq!(
            decode_packet_for_platform(&bytes, 1, "darwin-arm64").unwrap_err(),
            "invalid or duplicate class identity"
        );
    }
}
