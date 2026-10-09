use std::{fs, process::Command};

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
