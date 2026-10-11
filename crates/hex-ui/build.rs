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
    // Intentionally absent: refresh identity on every Cargo invocation, including
    // commits that change only another workspace crate or an untracked source.
    println!("cargo:rerun-if-changed=.hex-build-identity-refresh");
    let revision = git(&["rev-parse", "--short=8", "HEAD"]).filter(|value| {
        value.len() >= 8 && value.len() <= 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    let status = git(&["status", "--porcelain", "--untracked-files=normal"]);
    let identity = match (revision, status) {
        (Some(revision), Some(status)) if status.is_empty() => revision[..8].into(),
        (Some(revision), Some(_)) => format!("{} dirty", &revision[..8]),
        _ => "unknown".into(),
    };
    println!("cargo:rustc-env=HEX_BUILD_IDENTITY={identity}");
}
