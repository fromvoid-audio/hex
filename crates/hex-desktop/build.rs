//! Capture source provenance at the leaf desktop binary so unrelated commits
//! never force a rebuild of `hex-ui`. No Git or file I/O occurs at runtime.
use std::process::Command;

fn git(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=HEX_RELEASE_LABEL");
    let version =
        std::env::var("HEX_RELEASE_LABEL").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_owned());
    println!("cargo:rustc-env=HEX_DISPLAY_VERSION={version}");
    println!("cargo:rerun-if-env-changed=HEX_GUARDED_BUILD_SOURCE");
    // Supported entrypoints freeze this display input before invoking Cargo.
    // Raw Cargo retains conservative refresh on this leaf binary rather than
    // claiming a stale identity after untracked or unrelated workspace edits.
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
            let suffix = if parts[2] == "dirty" { " dirty" } else { "" };
            println!(
                "cargo:rustc-env=HEX_BUILD_IDENTITY={}{suffix}",
                &parts[0][..8]
            );
            println!("cargo:rustc-env=HEX_DIAGNOSTIC_SOURCE={source}");
            return;
        }
    }
    // Intentionally absent: refresh identity on every raw Cargo build of
    // hex-desktop without invalidating hex-ui.
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
