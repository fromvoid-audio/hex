use std::{
    fs,
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
#[test]
fn live_import_requires_balance_consent_and_preserves_existing_output() {
    let input = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../hex-live-set/tests/fixtures/minimal/hex-minimal-live-001.als");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hex-cli-live-{}-{stamp}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let output = dir.join("import.hex");
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_hex-cli"))
            .current_dir(input.parent().unwrap())
            .arg("import-live-set")
            .arg(input.file_name().unwrap())
            .arg(&output)
            .args(extra)
            .output()
            .unwrap()
    };
    let refused = run(&[]);
    assert!(!refused.status.success());
    assert!(!output.exists());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("nonneutral pan"));
    let accepted = run(&["--accept-hex-balance"]);
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let saved = fs::read(&output).unwrap();
    let project = hex_storage::load(&output).unwrap();
    assert_eq!(project.tracks().len(), 2);
    assert!(project.tracks()[1].instrument().is_none());
    assert_eq!(
        project.import_provenance().unwrap().original_bytes(),
        fs::read(&input).unwrap()
    );
    assert!(!run(&["--accept-hex-balance"]).status.success());
    assert_eq!(fs::read(&output).unwrap(), saved);
    let missing = dir.join("missing.als");
    let absent = dir.join("absent.hex");
    let failed = Command::new(env!("CARGO_BIN_EXE_hex-cli"))
        .arg("import-live-set")
        .arg(&missing)
        .arg(&absent)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(!absent.exists());
    fs::remove_dir_all(dir).unwrap();
}
