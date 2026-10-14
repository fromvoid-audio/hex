use hex_clap_scan::{
    Catalog, SDK, ValidRecord, binary_identity, controlled_path, decode_packet, hash,
};
fn number(v: &mut Vec<u8>, n: u64, count: usize) {
    for i in 0..count {
        v.push((n >> (i * 8)) as u8);
    }
}
fn text(v: &mut Vec<u8>, s: &[u8]) {
    number(v, s.len() as u64, 4);
    v.extend(s);
}
struct PayloadFixture {
    bytes: Vec<u8>,
    sdk: usize,
    platform: usize,
    api: usize,
    count: usize,
    descriptor_api: Option<usize>,
}
fn payload_for(
    platform: &str,
    id: &str,
    name: &[u8],
    features: &[&[u8]],
    copies: u32,
) -> PayloadFixture {
    let mut p = vec![];
    number(&mut p, 1, 2);
    number(&mut p, 7, 8);
    let sdk = p.len() + 4;
    text(&mut p, SDK.as_bytes());
    let platform_at = p.len() + 4;
    text(&mut p, platform.as_bytes());
    let api = p.len();
    for n in [1, 2, 10] {
        number(&mut p, n as u64, 4);
    }
    let count = p.len();
    number(&mut p, copies.into(), 4);
    let descriptor_api = (copies > 0).then_some(p.len());
    for _ in 0..copies {
        for n in [1, 2, 10] {
            number(&mut p, n, 4);
        }
        text(&mut p, id.as_bytes());
        text(&mut p, name);
        for field in [
            b"hex".as_slice(),
            b"https://invalid.example",
            b"",
            b"",
            b"fixture-v1",
            b"metadata only",
        ] {
            text(&mut p, field);
        }
        number(&mut p, features.len() as u64, 4);
        for s in features {
            text(&mut p, s);
        }
    }
    PayloadFixture {
        bytes: p,
        sdk,
        platform: platform_at,
        api,
        count,
        descriptor_api,
    }
}
fn payload(id: &str, name: &[u8], features: &[&[u8]], copies: u32) -> Vec<u8> {
    payload_for(hex_clap_scan::platform(), id, name, features, copies).bytes
}
fn packet(p: Vec<u8>) -> Vec<u8> {
    let mut b = b"HCLSCAN1".to_vec();
    number(&mut b, p.len() as u64, 4);
    b.extend(p);
    b
}
fn good() -> Vec<u8> {
    packet(payload(
        "org.hex.original",
        b"original \"fixture\"",
        &[b"audio-effect", b"unknown-feature"],
        1,
    ))
}
#[test]
fn descriptors_are_owned_metadata_unknown_features_not_capabilities() {
    let d = decode_packet(&good(), 7).unwrap();
    assert_eq!(d.api, [1, 2, 10]);
    assert_eq!(d.descriptors[0].id, "org.hex.original");
    assert_eq!(d.descriptors[0].name, "original \"fixture\"");
    assert_eq!(
        d.descriptors[0].features,
        vec!["audio-effect", "unknown-feature"]
    );
    assert!(
        decode_packet(&packet(payload("", b"", &[], 0)), 7)
            .unwrap()
            .descriptors
            .is_empty()
    );
}
#[test]
fn all_truncation_header_length_version_request_api_source_and_trailing_refuse() {
    let bytes = good();
    for i in 0..bytes.len() {
        assert!(decode_packet(&bytes[..i], 7).is_err(), "truncation{i}");
    }
    let fixture = payload_for(
        hex_clap_scan::platform(),
        "org.hex.original",
        b"original \"fixture\"",
        &[b"audio-effect", b"unknown-feature"],
        1,
    );
    for (at, value, error) in [
        (0, b'X', "clap packet header/capacity refused"),
        (8, 0, "clap packet length/trailing refused"),
        (12, 2, "clap protocol version refused"),
        (14, 8, "stale request identity"),
        (12 + fixture.sdk, b'z', "clap source/platform/API refused"),
        (
            12 + fixture.platform,
            b'z',
            "clap source/platform/API refused",
        ),
        (12 + fixture.api, 2, "clap source/platform/API refused"),
        (
            12 + fixture.descriptor_api.unwrap(),
            2,
            "descriptor api refused",
        ),
    ] {
        let mut bad = bytes.clone();
        bad[at] = value;
        assert_eq!(decode_packet(&bad, 7).unwrap_err(), error, "offset{at}");
    }
    assert!(decode_packet(&bytes, 8).is_err());
    let mut bad = bytes;
    bad.push(0);
    assert!(decode_packet(&bad, 7).is_err());
}
#[test]
fn duplicate_blank_utf8_and_capacity_fields_refuse() {
    for (id, name, count) in [
        ("same", b"name".as_slice(), 2),
        ("", b"name".as_slice(), 1),
        ("id", b" ".as_slice(), 1),
        ("id", b"\xff".as_slice(), 1),
        ("id", b"name\0tail".as_slice(), 1),
    ] {
        assert!(decode_packet(&packet(payload(id, name, &[], count)), 7).is_err());
    }
    assert!(decode_packet(&packet(payload("id", &[b'x'; 513], &[], 1)), 7).is_err());
    assert!(
        decode_packet(
            &packet(payload("id", b"name", &[b"x".as_slice(); 65], 1)),
            7
        )
        .is_err()
    );
    assert!(decode_packet(&packet(payload("id", b"name", &[&[b'x'; 129]], 1)), 7).is_err());
    let mut fixture = payload_for(hex_clap_scan::platform(), "id", b"name", &[], 0);
    fixture.bytes[fixture.count..fixture.count + 4].copy_from_slice(&1025u32.to_le_bytes());
    assert_eq!(
        decode_packet(&packet(fixture.bytes), 7).unwrap_err(),
        "clap descriptor count refused"
    );
}
#[test]
fn both_platform_count_api_and_string_locations_and_foreign_tag_refusal() {
    for platform in ["linux-x86_64", "macos-arm64"] {
        let mut fixture = payload_for(platform, "id", b"name", &[], 0);
        assert_eq!(
            &fixture.bytes[fixture.platform..fixture.platform + platform.len()],
            platform.as_bytes()
        );
        assert_eq!(
            &fixture.bytes[fixture.api..fixture.api + 4],
            &1u32.to_le_bytes()
        );
        assert_eq!(fixture.bytes.len(), fixture.count + 4);
        assert_eq!(
            &fixture.bytes[fixture.count..fixture.count + 4],
            &0u32.to_le_bytes()
        );
        fixture.bytes[fixture.count..fixture.count + 4].copy_from_slice(&1025u32.to_le_bytes());
        assert_eq!(
            &fixture.bytes[fixture.count..fixture.count + 4],
            &1025u32.to_le_bytes()
        );
        if platform == hex_clap_scan::platform() {
            assert_eq!(
                decode_packet(&packet(fixture.bytes), 7).unwrap_err(),
                "clap descriptor count refused"
            );
        } else {
            assert_eq!(
                decode_packet(&packet(fixture.bytes), 7).unwrap_err(),
                "clap source/platform/API refused"
            );
        }
    }
    let foreign = payload_for("unsupported", "id", b"name", &[], 0);
    assert_eq!(
        decode_packet(&packet(foreign.bytes), 7).unwrap_err(),
        "clap source/platform/API refused"
    );
}

#[test]
fn failed_attempt_retains_previous_descriptor_and_error() {
    let discovery = decode_packet(&good(), 7).unwrap();
    let prior = ValidRecord {
        module: "good".into(),
        binary_sha256: "0".repeat(64),
        helper_sha256: "1".repeat(64),
        native_source_sha256: "2".repeat(64),
        compiler: "original fixture".into(),
        discovery,
    };
    let mut c = Catalog::default();
    c.record(Ok(prior));
    let old = serde_json::to_value(c.last_valid.clone()).unwrap();
    c.record(Err("native initialization refused".into()));
    assert_eq!(serde_json::to_value(c.last_valid).unwrap(), old);
    assert_eq!(
        c.last_error.as_deref(),
        Some("native initialization refused")
    );
}
#[test]
fn static_architecture_digest_and_symlink_refuse_without_loader() {
    let root = std::env::temp_dir().join(format!("hex-clap-protocol-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("fixture.clap");
    let mut bytes = vec![0; 64];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    bytes[16] = 3;
    bytes[18] = 62;
    bytes[20] = 1;
    std::fs::write(&file, &bytes).unwrap();
    assert!(binary_identity(&file, &hash(&bytes)).is_ok());
    assert!(binary_identity(&file, &"0".repeat(64)).is_err());
    bytes[18] = 183;
    std::fs::write(&file, &bytes).unwrap();
    assert!(binary_identity(&file, &hash(&bytes)).is_err());
    assert!(controlled_path(&root, "../fixture.clap").is_err());
    #[cfg(unix)]
    {
        let link = root.join("link.clap");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(controlled_path(&root, "link.clap").is_err());
    }
    std::fs::remove_dir_all(root).unwrap();
}
