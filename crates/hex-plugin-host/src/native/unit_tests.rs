//! Fast non-SDK unit and allocation-probe tests for `hex_plugin_host::native`.
use super::*;
#[cfg(feature = "allocation-probe")]
use std::fs;

#[test]
fn editor_transport_tokens_leave_the_independent_cancellation_direction_intact() {
    use std::io::Write;
    let PreparationCancellation {
        token,
        mut receiver,
    } = PreparationCancellation::new_on_control().unwrap();
    assert!(!token.take_transport_space_on_control());
    receiver.write_all(&[1, 1]).unwrap();
    assert!(token.take_transport_space_on_control());
    assert!(token.take_transport_space_on_control());
    assert!(!token.take_transport_space_on_control());
    receiver.write_all(&[1]).unwrap();
    token.cancel_on_control();
    assert!(!token.take_transport_space_on_control());
    receiver
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    assert_eq!(receiver.read(&mut [0]).unwrap(), 0);
}

#[cfg(feature = "allocation-probe")]
#[test]
fn revalidation_paths_allocate_one_buffer_and_depth_refusal_has_no_heap_diagnostic() {
    let root =
        std::env::temp_dir().join(format!("hex-plugin-host-unit-reval-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("Contents/MacOS")).unwrap();
    fs::write(root.join("Contents/MacOS/fixture"), b"original").unwrap();
    let relative = "Contents/MacOS/fixture";
    let mut returned = None;
    let measured = allocation_counter::measure(|| {
        returned = Some(hex_plugin_scan::macos::identity::relative_file(&root, relative).unwrap());
    });
    let returned = returned.unwrap();
    assert_eq!((measured.count_total, measured.count_current), (1, 1));
    assert_eq!(
        (measured.bytes_total, measured.bytes_current),
        (returned.capacity() as u64, returned.capacity() as i64)
    );
    assert_eq!(
        returned.capacity(),
        root.as_os_str().len() + 1 + relative.len()
    );
    let too_deep = "d/".repeat(33);
    let mut refusal = None;
    let measured = allocation_counter::measure(|| {
        refusal =
            Some(hex_plugin_scan::macos::identity::relative_file(&root, &too_deep).unwrap_err());
    });
    let refusal = refusal.unwrap();
    assert!(matches!(
        refusal,
        hex_plugin_scan::macos::validation::Error::Invalid("discovery path depth refused")
    ));
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert_eq!((measured.bytes_total, measured.bytes_current), (0, 0));
    let _ = fs::remove_dir_all(&root);
}

#[cfg(feature = "allocation-probe")]
#[test]
fn revalidation_universal_macho_parser_uses_no_heap_scratch() {
    use hex_plugin_scan::macos::macho::{self, Image, Role};
    // Two independent thin headers in a big-endian universal container. This
    // tests the parser/data budget only; it is never loaded as executable code.
    let thin = |cpu, subtype| {
        [0xfeed_facf_u32, cpu, subtype, 8, 1, 8, 0, 0, 0, 8]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>()
    };
    let mut image = [
        0xcafe_babe_u32,
        2,
        0x0100_000c,
        0,
        64,
        40,
        3,
        0x0100_0007,
        3,
        104,
        40,
        3,
    ]
    .into_iter()
    .flat_map(u32::to_be_bytes)
    .collect::<Vec<_>>();
    image.resize(64, 0);
    image.extend(thin(0x0100_000c, 0));
    image.extend(thin(0x0100_0007, 3));
    let measured = allocation_counter::measure(|| {
        assert_eq!(
            macho::inspect(&image, Role::Module),
            Ok(Image::UniversalArm64)
        );
    });
    assert_eq!((measured.count_total, measured.bytes_total), (0, 0));
    assert_eq!((measured.count_current, measured.bytes_current), (0, 0));
}

#[cfg(feature = "allocation-probe")]
#[test]
fn original_failure_diagnostic_transfers_without_copying_or_destroying_backing() {
    let original = String::from("original selected-source diagnostic");
    let address = original.as_ptr();
    let mut failure = PreparationFailure::before_native(Error::Source(original));
    let mut returned = None;
    let measured = allocation_counter::measure(|| {
        returned = Some(failure.take_cause_on_control());
    });
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert_eq!((measured.bytes_total, measured.bytes_current), (0, 0));
    let Error::Source(original) = returned.unwrap() else {
        panic!("original typed source error");
    };
    assert_eq!(original.as_ptr(), address);
    assert!(matches!(
        failure.cause(),
        Error::Protocol("native preparation cause transferred")
    ));
    assert!(matches!(
        failure.poll_disposal_on_control().unwrap(),
        Some(PreparationDisposal::NoInstance)
    ));
    drop(failure);
    assert_eq!(original, "original selected-source diagnostic");
}

#[cfg(feature = "allocation-probe")]
#[test]
fn sticky_lost_custody_preserves_original_io_error_without_heap_diagnostic_copies() {
    let original = Arc::new(std::io::Error::from_raw_os_error(10));
    let mut custody = ChildCustody::Lost(Arc::clone(&original));
    let mut observed = None;
    let mut reaped = None;
    let measured = allocation_counter::measure(|| {
        observed = Some(custody.observe().unwrap_err());
        reaped = Some(custody.reap().unwrap_err());
    });
    assert_eq!((measured.count_total, measured.count_current), (0, 0));
    assert_eq!((measured.bytes_total, measured.bytes_current), (0, 0));
    for error in [observed.unwrap(), reaped.unwrap()] {
        let Error::Custody(crate::CustodyError::Lost(source)) = error else {
            panic!("original sticky OS failure");
        };
        assert!(Arc::ptr_eq(&source, &original));
        assert_eq!(source.raw_os_error(), Some(10));
    }
    assert!(matches!(custody, ChildCustody::Lost(_)));
}

#[test]
fn source_unchanged_fast_avoids_rehash_when_intact_and_refuses_tampering() {
    use std::io::Write;
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "hex-plugin-host-fast-reval-{}-{unique}.vst3",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Contents/MacOS")).unwrap();
    let info_path = root.join("Contents/Info.plist");
    let exec_path = root.join("Contents/MacOS/fixture");
    std::fs::write(
        &info_path,
        b"<?xml version=\"1.0\"?><plist><dict><key>CFBundleExecutable</key><string>fixture</string></dict></plist>",
    )
    .unwrap();
    // Minimal arm64 Mach-O bundle header (MH_MAGIC_64, CPU_TYPE_ARM64, MH_BUNDLE = 8, 1 cmd of 8 bytes).
    let mut macho: Vec<u8> = [0xfeed_facf_u32, 0x0100_000c, 0, 8, 1, 8, 0, 0, 0, 8]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    macho.resize(4096, 0xAB);
    std::fs::write(&exec_path, &macho).unwrap();

    let source = Source::inspect_on_control(
        &root,
        "Contents/MacOS/fixture",
        ClassId::new([0x11; 16]).unwrap(),
    )
    .unwrap();
    // Intact bundle/executable passes the O(1) stat fingerprint check without re-hashing.
    assert!(source.unchanged_fast().is_ok());

    // Tampering with Info.plist triggers full validation refusal.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&info_path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert!(matches!(source.unchanged_fast(), Err(Error::Validation(_))));

    // Restoring Info.plist and tampering with the Mach-O binary triggers validation refusal.
    std::fs::write(
        &info_path,
        b"<?xml version=\"1.0\"?><plist><dict><key>CFBundleExecutable</key><string>fixture</string></dict></plist>",
    )
    .unwrap();
    let source2 = Source::inspect_on_control(
        &root,
        "Contents/MacOS/fixture",
        ClassId::new([0x11; 16]).unwrap(),
    )
    .unwrap();
    assert!(source2.unchanged_fast().is_ok());
    macho[4095] ^= 0xFF;
    std::fs::write(&exec_path, &macho).unwrap();
    assert!(matches!(
        source2.unchanged_fast(),
        Err(Error::Validation(_))
    ));

    let _ = std::fs::remove_dir_all(&root);
}
