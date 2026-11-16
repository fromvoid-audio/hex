//! Capture source identity at compilation; no Git or file I/O occurs in the UI.
use std::process::Command;

fn git(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=HEX_RELEASE_CHANGELOG");
    println!("cargo:rerun-if-env-changed=HEX_RELEASE_LABEL");
    let version =
        std::env::var("HEX_RELEASE_LABEL").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_owned());
    println!("cargo:rustc-env=HEX_DISPLAY_VERSION={version}");
    let release_dataset = std::env::var_os("HEX_RELEASE_CHANGELOG");
    let dataset = release_dataset
        .as_deref()
        .map(std::path::Path::new)
        .unwrap_or_else(|| std::path::Path::new("../../changelog/generated.json"));
    let bytes = match std::fs::read(dataset) {
        Ok(bytes) if bytes.len() <= 1024 * 1024 => bytes,
        Ok(_) => panic!("changelog exceeds the validated dataset limit"),
        Err(error) if release_dataset.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
            Vec::new()
        }
        Err(error) => panic!("cannot embed changelog: {error}"),
    };
    let output =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo output directory"));
    let embedded = output.join("changelog.json");
    if std::fs::read(&embedded).ok().as_deref() != Some(bytes.as_slice()) {
        std::fs::write(embedded, bytes).expect("embed changelog");
    }
    println!("cargo:rerun-if-env-changed=HEX_GUARDED_BUILD_SOURCE");
    // Supported entrypoints freeze this display input before invoking Cargo.
    // Raw Cargo retains conservative refresh rather than claiming a stale
    // identity after untracked or unrelated workspace edits.
    if let Ok(source) = std::env::var("HEX_GUARDED_BUILD_SOURCE") {
        let parts: Vec<_> = source.split(':').collect();
        if parts.len() == 3
            && parts[0].len() == 40
            && parts[1].len() == 64
            && parts[..2]
                .iter()
                .all(|part| part.bytes().all(|byte| byte.is_ascii_hexdigit()))
            && matches!(parts[2], "clean" | "dirty")
        {
            if dataset.exists() {
                println!("cargo:rerun-if-changed={}", dataset.display());
            }
            let suffix = if parts[2] == "dirty" { " dirty" } else { "" };
            println!(
                "cargo:rustc-env=HEX_BUILD_IDENTITY={}{suffix}",
                &parts[0][..8]
            );
            println!("cargo:rustc-env=HEX_DIAGNOSTIC_SOURCE={source}");
            return;
        }
    }
    // Intentionally absent: refresh identity on every Cargo invocation, including
    // commits that change only another workspace crate or an untracked source.
    println!("cargo:rerun-if-changed=.hex-build-identity-refresh");
    let revision = git(&["rev-parse", "HEAD"]).filter(|value| {
        value.len() >= 8 && value.len() <= 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    let status = git(&["status", "--porcelain", "--untracked-files=normal"]);
    let diagnostic = match (&revision, &status) {
        (Some(revision), Some(status)) => format!(
            "{revision}:tree_digest_unavailable:{}",
            if status.is_empty() { "clean" } else { "dirty" }
        ),
        _ => "unknown".into(),
    };
    println!("cargo:rustc-env=HEX_DIAGNOSTIC_SOURCE={diagnostic}");
    let identity = match (revision, status) {
        (Some(revision), Some(status)) if status.is_empty() => revision[..8].into(),
        (Some(revision), Some(_)) => format!("{} dirty", &revision[..8]),
        _ => "unknown".into(),
    };
    println!("cargo:rustc-env=HEX_BUILD_IDENTITY={identity}");
}
