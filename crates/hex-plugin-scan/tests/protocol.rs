use hex_plugin_scan::*;
fn text(packet: &mut Vec<u8>, s: &str) {
    packet.extend_from_slice(&(s.len() as u32).to_le_bytes());
    packet.extend_from_slice(s.as_bytes());
}
fn packet(flags: u32) -> Vec<u8> {
    let mut p = vec![1, 0];
    p.extend_from_slice(&1_u64.to_le_bytes());
    text(&mut p, SDK);
    text(&mut p, PUBLIC_SDK);
    text(&mut p, platform());
    p.extend_from_slice(&flags.to_le_bytes());
    text(&mut p, "original \"fixture\"");
    text(&mut p, "invalid.example");
    text(&mut p, "fixture@invalid.example");
    p.extend_from_slice(&1_u32.to_le_bytes());
    p.extend_from_slice(&[33; 16]);
    p.extend_from_slice(&i32::MAX.to_le_bytes());
    text(&mut p, "Audio Module Class");
    text(&mut p, "factory only");
    let mut b = b"HVSCAN01".to_vec();
    b.extend_from_slice(&(p.len() as u32).to_le_bytes());
    b.extend_from_slice(&p);
    b
}
#[test]
fn strict_metadata_and_unknown_flags_are_records_not_hosting_capabilities() {
    let d = decode_packet(&packet(0), 1).unwrap();
    assert_eq!(d.vendor, "original \"fixture\"");
    assert_eq!(d.classes[0].cid, [33; 16]);
    assert!(d.cacheable());
    for flags in [1, 8, 0x80000000] {
        let d = decode_packet(&packet(flags), 1).unwrap();
        assert_eq!(d.factory_flags, flags);
        assert!(!d.cacheable());
    }
}
#[test]
fn every_truncation_version_stale_identity_trailing_and_length_corruption_refuse() {
    let good = packet(0);
    for n in 0..good.len() {
        assert!(decode_packet(&good[..n], 1).is_err(), "length {n}");
    }
    assert!(decode_packet(&good, 2).is_err());
    let mut bad = good.clone();
    bad[12] = 2;
    assert!(decode_packet(&bad, 1).is_err());
    bad = good.clone();
    bad[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode_packet(&bad, 1).is_err());
    bad = good.clone();
    bad.extend_from_slice(b"native logs cannot append metadata");
    assert!(decode_packet(&bad, 1).is_err());
    bad = good;
    bad[22] = 255;
    assert!(decode_packet(&bad, 1).is_err());
    assert!(decode_packet(&vec![0; MAX_PACKET + 13], 1).is_err());
}
#[test]
fn failed_attempt_preserves_prior_valid_discovery_and_retains_error() {
    let mut catalog = Catalog::default();
    let record = ValidRecord {
        module: "good".into(),
        binary_sha256: "a".repeat(64),
        helper_sha256: "b".repeat(64),
        native_source_sha256: source_hash(),
        compiler: "fixture compiler".into(),
        discovery: decode_packet(&packet(0), 1).unwrap(),
    };
    catalog.record(Ok(record.clone()));
    catalog.record(Err("changed controlled binary".into()));
    assert_eq!(catalog.last_valid.unwrap().discovery, record.discovery);
    assert_eq!(
        catalog.last_error.as_deref(),
        Some("changed controlled binary")
    );
}
#[test]
fn static_architecture_refuses_before_any_process_load() {
    let dir = std::env::temp_dir().join(format!("hex-scan-static-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("input");
    std::fs::write(&p, b"not ELF").unwrap();
    assert!(binary_identity(&p, &hash(b"not ELF")).is_err());
    let mut bytes = vec![0; 64];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    bytes[20] = 1;
    bytes[16] = 3;
    bytes[18] = 183;
    std::fs::write(&p, &bytes).unwrap();
    assert!(binary_identity(&p, &hash(&bytes)).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&p, dir.join("link")).unwrap();
        assert!(controlled_path(&dir, "link").is_err());
    }
    assert!(controlled_path(&dir, "../outside").is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
