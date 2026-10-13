use hex_live_set::{Diagnostic, Error, Options, import_bytes, import_file};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
const ALS: &[u8] = include_bytes!("fixtures/minimal/hex-minimal-live-001.als");
fn base() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal")
}
fn consent() -> Options {
    Options {
        accept_hex_balance: true,
    }
}
fn xml() -> String {
    let mut s = String::new();
    std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(ALS), &mut s).unwrap();
    s
}
fn modified(from: &str, to: &str) -> String {
    let s = xml();
    assert!(s.contains(from), "mutation target missing");
    s.replacen(from, to, 1)
}
#[test]
fn native_fixture_imports_canonical_values_and_exact_provenance() {
    assert!(matches!(
        import_bytes(ALS, &base(), 48_000, Options::default()),
        Err(Error::Unsupported(_))
    ));
    let imported = import_bytes(ALS, &base(), 48_000, consent()).unwrap();
    assert_eq!(
        imported.diagnostics,
        vec![
            Diagnostic::OriginalDocumentRetained,
            Diagnostic::HexBalanceLaw,
            Diagnostic::DeviceFreeMidiIsSilent
        ]
    );
    let p = &imported.project;
    assert_eq!(p.tracks().len(), 2);
    let a = &p.tracks()[0];
    let m = &p.tracks()[1];
    assert_eq!(a.name(), "fixture audio");
    assert_eq!(a.gain(), (0.5011872053_f64 as f32));
    assert_eq!(a.balance(), -0.25);
    assert!(!a.muted());
    assert_eq!(m.name(), "fixture midi");
    assert_eq!(m.gain(), (0.251188606_f64 as f32));
    assert_eq!(m.balance(), 0.0);
    assert!(m.muted());
    assert!(a.instrument().is_none() && m.instrument().is_none());
    let snapshot = p.snapshot();
    assert_eq!(snapshot.tempo_points[0].tempo().micro_bpm(), 120_000_000);
    assert_eq!(snapshot.meter_points[0].meter().numerator(), 4);
    assert_eq!(snapshot.meter_points[0].meter().denominator(), 4);
    let clips = a.clips();
    assert_eq!(clips.len(), 1);
    assert_eq!(
        (
            clips[0].start(),
            clips[0].duration(),
            clips[0].source_offset(),
            clips[0].source_length()
        ),
        (96_000, 96_000, 24_000, 96_000)
    );
    let notes = m.midi_clips()[0].source().notes();
    assert_eq!(m.midi_clips()[0].start().raw(), 12u64 << 32);
    assert_eq!(m.midi_clips()[0].length().raw(), 4u64 << 32);
    assert_eq!(notes.len(), 2);
    assert_eq!(
        (
            notes[0].pitch(),
            notes[0].start().raw(),
            notes[0].duration().raw(),
            notes[0].velocity()
        ),
        (60, 0, 1u64 << 32, 64)
    );
    assert_eq!(
        (
            notes[1].pitch(),
            notes[1].start().raw(),
            notes[1].duration().raw(),
            notes[1].velocity()
        ),
        (67, 3u64 << 31, 1u64 << 31, 100)
    );
    assert_eq!(p.import_provenance().unwrap().original_bytes(), ALS);
    let reopened = hex_storage::decode(&hex_storage::encode(p).unwrap()).unwrap();
    assert_eq!(&reopened, p);
    assert_eq!(reopened.import_provenance().unwrap().original_bytes(), ALS);
    let plain = xml();
    let plain_import = import_bytes(plain.as_bytes(), &base(), 48_000, consent()).unwrap();
    assert_eq!(
        plain_import
            .project
            .import_provenance()
            .unwrap()
            .original_bytes(),
        plain.as_bytes()
    );
    let mut plain_snapshot = plain_import.project.snapshot();
    plain_snapshot.import_provenance = snapshot.import_provenance.clone();
    assert_eq!(plain_snapshot, snapshot);
    assert!(import_file(&base().join("hex-minimal-live-001.als"), 48_000, consent()).is_ok());
}

#[test]
fn unknown_nonmusical_metadata_is_retained_while_behavior_is_refused() {
    let unknown = modified(
        "<Annotation Value=\"\" />",
        "<Annotation Value=\"private unknown metadata\" /><UnknownVendorMetadata Value=\"opaque\" />",
    );
    let imported = import_bytes(unknown.as_bytes(), &base(), 48_000, consent()).unwrap();
    assert_eq!(
        imported
            .project
            .import_provenance()
            .unwrap()
            .original_bytes(),
        unknown.as_bytes()
    );
    let mut session = xml();
    let slot = session.find("<ClipSlot Id=\"0\">").unwrap();
    let value = slot + session[slot..].find("<Value />").unwrap();
    session.replace_range(
        value..value + "<Value />".len(),
        "<Value><UnknownSessionEvent /></Value>",
    );
    assert!(matches!(
        import_bytes(session.as_bytes(), &base(), 48_000, consent()),
        Err(Error::Unsupported(_))
    ));
    for (from, to) in [
        ("<Devices />", "<Devices><PluginDevice /></Devices>"),
        (
            "<SampleVolume Value=\"1\" />",
            "<SampleVolume Value=\"0.5\" />",
        ),
        ("<TimeSignature>", "<TimeSignature><UnknownMeterLaw />"),
        (
            "<IsWarped Value=\"false\" />",
            "<IsWarped Value=\"true\" />",
        ),
        ("<LoopOn Value=\"false\" />", "<LoopOn Value=\"true\" />"),
        ("<Fade Value=\"false\" />", "<Fade Value=\"true\" />"),
        ("<TrackDelay>", "<TrackDelay><UnknownTimingLaw />"),
        ("<Sends />", "<Sends><Send /></Sends>"),
        ("AudioOut/Main", "AudioOut/Track.999"),
        (
            "<Envelopes />",
            "<Envelopes><AutomationEnvelope /></Envelopes>",
        ),
        ("<EventLists />", "<EventLists><Expression /></EventLists>"),
        ("Velocity=\"64\"", "Velocity=\"64.5\""),
        ("Duration=\"1\"", "Duration=\"NaN\""),
        ("MinorVersion=\"12.0_12402\"", "MinorVersion=\"future\""),
    ] {
        let input = modified(from, to);
        assert!(
            import_bytes(input.as_bytes(), &base(), 48_000, consent()).is_err(),
            "{from}"
        );
    }
}
#[test]
fn malformed_resource_limits_and_external_references_are_refused() {
    let cases = [
        b"".as_slice(),
        b"<Ableton>",
        b"<!DOCTYPE Ableton [<!ENTITY x SYSTEM 'file:///private'>]><Ableton>&x;</Ableton>",
    ];
    for input in cases {
        assert!(import_bytes(input, &base(), 48_000, consent()).is_err());
    }
    assert!(import_bytes(&vec![b'x'; 1_048_577], &base(), 48_000, consent()).is_err());
    let deep = format!("{}{}", "<x>".repeat(65), "</x>".repeat(65));
    assert!(matches!(
        import_bytes(deep.as_bytes(), &base(), 48_000, consent()),
        Err(Error::Limit(_))
    ));
    let many = format!("<r>{}</r>", "<x/>".repeat(100_001));
    assert!(matches!(
        import_bytes(many.as_bytes(), &base(), 48_000, consent()),
        Err(Error::Limit(_))
    ));
    let attributes = (0..33).map(|n| format!("a{n}=\"0\" ")).collect::<String>();
    assert!(matches!(
        import_bytes(
            format!("<r {attributes}/>").as_bytes(),
            &base(),
            48_000,
            consent()
        ),
        Err(Error::Limit(_))
    ));
    let long_text = format!("<r>{}</r>", "x".repeat(16_385));
    assert!(matches!(
        import_bytes(long_text.as_bytes(), &base(), 48_000, consent()),
        Err(Error::Limit(_))
    ));
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&vec![b'x'; 4 * 1024 * 1024 + 1]).unwrap();
    assert!(matches!(
        import_bytes(&encoder.finish().unwrap(), &base(), 48_000, consent()),
        Err(Error::Limit(_))
    ));
    let mut trailing = ALS.to_vec();
    trailing.push(0);
    assert!(import_bytes(&trailing, &base(), 48_000, consent()).is_err());
    let mut multiple = ALS.to_vec();
    multiple.extend_from_slice(ALS);
    assert!(import_bytes(&multiple, &base(), 48_000, consent()).is_err());
    assert!(import_bytes(&ALS[..ALS.len() - 4], &base(), 48_000, consent()).is_err());
    for path in [
        "../private.wav",
        "/private.wav",
        "https://example.invalid/a.wav",
        "Samples/missing.wav",
    ] {
        let input = modified("Samples/Imported/original-markers-48000.wav", path);
        let error = match import_bytes(input.as_bytes(), &base(), 48_000, consent()) {
            Err(e) => e,
            Ok(_) => panic!("unsafe path accepted"),
        };
        assert!(!error.to_string().contains(path));
    }
}

#[test]
fn rendered_fixture_matches_original_pcm_window_under_declared_hex_mix() {
    let imported = import_bytes(ALS, &base(), 48_000, consent()).unwrap();
    let reopened = hex_storage::decode(&hex_storage::encode(&imported.project).unwrap()).unwrap();
    let compiled = hex_engine::compile::project(&reopened, &imported.media).unwrap();
    let mut renderer = hex_engine::Renderer::new(&compiled);
    renderer.play();
    let gain = 0.5011872053_f64 as f32;
    let mut position = 0usize;
    while position < 400_000 {
        let len = (400_000 - position).min(if position % 2 == 0 { 511 } else { 257 });
        let mut output = vec![[0.0; 2]; len];
        renderer.render(&mut output).unwrap();
        for (offset, actual) in output.iter().enumerate() {
            let frame = position + offset;
            let expected = if (96_000..192_000).contains(&frame) {
                let source = (frame - 96_000 + 24_000) % 4800;
                let left = match source {
                    0 => 0.25,
                    1 => -0.125,
                    _ => 0.0,
                };
                let right = match source {
                    2 => -0.1875,
                    3 => 0.0625,
                    _ => 0.0,
                };
                [left * gain, right * (gain * 0.75)]
            } else {
                [0.0; 2]
            };
            assert_eq!(*actual, expected, "frame {frame}");
        }
        position += len;
    }
}

#[cfg(unix)]
#[test]
fn asset_symlink_is_refused_without_reading_external_target() {
    use std::os::unix::fs::symlink;
    let dir = std::env::temp_dir().join(format!("hex-live-symlink-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    symlink(base().join("Samples"), dir.join("Samples")).unwrap();
    assert!(matches!(
        import_bytes(ALS, &dir, 48_000, consent()),
        Err(Error::Asset(_))
    ));
    std::fs::remove_dir_all(dir).unwrap();
}
