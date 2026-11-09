use std::{fs, process::Command};

#[test]
fn edit_demo_renders_the_restored_gain_and_trim() {
    let directory = std::env::temp_dir().join(format!("hex-cli-edit-demo-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let output = directory.join("edited.wav");
    let baseline = directory.join("baseline.wav");
    let original = Command::new(env!("CARGO_BIN_EXE_hex-cli"))
        .arg("demo")
        .arg(&baseline)
        .output()
        .unwrap();
    assert!(original.status.success(), "{:?}", original.stderr);
    let result = Command::new(env!("CARGO_BIN_EXE_hex-cli"))
        .arg("edit-demo")
        .arg(&output)
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result.stderr);
    let bytes = fs::read(&output).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    // The trimmed clip starts at two beats (48,000 stereo PCM16 frames).
    let first_clip_byte = 44 + 48_000 * 4;
    assert!(bytes[44..first_clip_byte].iter().all(|byte| *byte == 0));
    assert!(bytes[first_clip_byte..].iter().any(|byte| *byte != 0));
    let original = fs::read(&baseline).unwrap();
    let original_clip_byte = 44 + 24_000 * 4;
    // Both clips start at source zero; the edit's restored track gain is 0.5.
    for frame in 0..1_000 {
        let at = original_clip_byte + frame * 4;
        let edited_at = first_clip_byte + frame * 4;
        let original_sample = i16::from_le_bytes([original[at], original[at + 1]]) as i32;
        let edited_sample = i16::from_le_bytes([bytes[edited_at], bytes[edited_at + 1]]) as i32;
        // Each independently positioned PCM16 sample has TPDF dither.
        assert!((2 * edited_sample - original_sample).abs() <= 4);
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn imports_actual_wav_and_refuses_overwrite_or_bad_media() {
    let directory = std::env::temp_dir().join(format!("hex-cli-import-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let input = directory.join("MiXeD.wav");
    let output = directory.join("render.wav");
    let invalid = directory.join("invalid.wav");
    let rejected = directory.join("must-not-exist.wav");
    let binary = env!("CARGO_BIN_EXE_hex-cli");
    let generated = Command::new(binary)
        .args(["demo"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(generated.status.success(), "{:?}", generated.stderr);
    let rendered = Command::new(binary)
        .arg("render-wav")
        .arg(&input)
        .arg(&output)
        .output()
        .unwrap();
    assert!(rendered.status.success(), "{:?}", rendered.stderr);
    let source = fs::read(&input).unwrap();
    let target = fs::read(&output).unwrap();
    assert_eq!(&source[..44], &target[..44]);
    // Integer PCM -> f32 -> PCM16 differs by at most one LSB with this export law.
    for (a, b) in source[44..]
        .chunks_exact(2)
        .zip(target[44..].chunks_exact(2))
    {
        let a = i16::from_le_bytes([a[0], a[1]]) as i32;
        let b = i16::from_le_bytes([b[0], b[1]]) as i32;
        assert!((a - b).abs() <= 1);
    }
    let overwrite = Command::new(binary)
        .arg("render-wav")
        .arg(&input)
        .arg(&output)
        .output()
        .unwrap();
    assert!(!overwrite.status.success());
    assert_eq!(fs::read(&output).unwrap(), target);
    fs::write(&invalid, b"broken").unwrap();
    let bad = Command::new(binary)
        .arg("render-wav")
        .arg(&invalid)
        .arg(&rejected)
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(!rejected.exists());
    fs::remove_dir_all(directory).unwrap();
}
