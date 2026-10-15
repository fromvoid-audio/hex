//! Build-only declared-layout evidence; unsupported observations remain Unknown.
#[path = "build_support/pcm_gate.rs"]
mod pcm_gate;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};
fn nonempty(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| !value.is_empty())
}
fn invoke(compiler: &Path, args: &[&str], evidence: &mut String) -> Option<String> {
    evidence.push_str(&format!("command: {compiler:?} {args:?}\n"));
    match Command::new(compiler).args(args).output() {
        Ok(output) => {
            evidence.push_str(&format!(
                "exit: {:?}\nstdout:\n{}\nstderr:\n{}\n",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
            output
                .status
                .success()
                .then(|| String::from_utf8(output.stdout).ok())
                .flatten()
        }
        Err(error) => {
            evidence.push_str(&format!("compiler command error: {error}\n"));
            None
        }
    }
}
fn ordinary_install(root: &Path, target: &str, evidence: &mut String) -> bool {
    let rustlib = root.join("lib/rustlib");
    let components = rustlib.join("components");
    let installer = rustlib.join("rust-installer-version");
    let manifest = rustlib.join(format!("manifest-rust-std-{target}"));
    for path in [&components, &installer, &manifest] {
        println!("cargo:rerun-if-changed={}", path.display());
        evidence.push_str(&format!("installation metadata: {path:?}\n"));
    }
    let Ok(components) = fs::read_to_string(components) else {
        return false;
    };
    let Ok(installer) = fs::read_to_string(installer) else {
        return false;
    };
    let Ok(manifest) = fs::read_to_string(manifest) else {
        return false;
    };
    let compiler = root.join("bin/rustc");
    evidence.push_str(&format!("sysroot compiler path: {compiler:?}\n"));
    compiler.is_file()
        && installer.trim() == "3"
        && components
            .lines()
            .any(|line| line == format!("rustc-{target}"))
        && components
            .lines()
            .any(|line| line == format!("rust-std-{target}"))
        && manifest
            .lines()
            .any(|line| line.starts_with(&format!("file:lib/rustlib/{target}/lib/libstd-")))
}
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support/pcm_gate.rs");
    let host = env::var("HOST").unwrap_or_default();
    let target = env::var("TARGET").unwrap_or_default();
    let target_flags = format!(
        "CARGO_TARGET_{}_RUSTFLAGS",
        target.replace('-', "_").to_uppercase()
    );
    let inputs = [
        "RUSTC",
        "CARGO_BUILD_RUSTC",
        "PATH",
        "HOST",
        "TARGET",
        "RUSTUP_TOOLCHAIN",
        "RUSTUP_HOME",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTC_BOOTSTRAP",
        "RUST_SYSROOT",
        "RUSTC_SYSROOT",
        "CARGO_BUILD_TARGET",
        "RUSTC_STAGE",
        "CARGO_UNSTABLE_BUILD_STD",
        "CARGO_UNSTABLE_BUILD_STD_FEATURES",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    ];
    for input in inputs
        .iter()
        .copied()
        .chain(std::iter::once(target_flags.as_str()))
    {
        println!("cargo:rerun-if-env-changed={input}");
    }
    let unsupported = [
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTC_BOOTSTRAP",
        "RUST_SYSROOT",
        "RUSTC_SYSROOT",
        "RUSTC_STAGE",
        "CARGO_UNSTABLE_BUILD_STD",
        "CARGO_UNSTABLE_BUILD_STD_FEATURES",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    ]
    .into_iter()
    .chain(std::iter::once(target_flags.as_str()))
    .any(nonempty);
    let compiler = env::var_os("RUSTC").map(PathBuf::from);
    let mut evidence =
        format!("contract: pinned-std-requested-layout-v1\nhost: {host}\ntarget: {target}\n");
    let decision = compiler
        .as_deref()
        .ok_or("missing_cargo_selected_compiler")
        .and_then(|compiler| {
            let verbose = invoke(compiler, &["-Vv"], &mut evidence)
                .ok_or("compiler_metadata_command_failed")?;
            let sysroot = invoke(compiler, &["--print", "sysroot"], &mut evidence)
                .ok_or("sysroot_command_failed")?;
            let root = Path::new(sysroot.trim());
            let installed = ordinary_install(root, &target, &mut evidence);
            pcm_gate::decide(pcm_gate::Observation {
                verbose: &verbose,
                host: &host,
                target: &target,
                ordinary_install: installed,
                unsupported_inputs: unsupported,
            })
        });
    evidence.push_str(&format!("decision: {decision:?}\nrequested layout declaration only; no security/RSS/physical allocator claim\n"));
    let saved = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .is_some_and(|path| fs::write(path.join("pcm-arc-build-evidence.txt"), evidence).is_ok());
    let known = decision.is_ok() && saved;
    // Always override any caller text; unsupported builds retain legacy Audio.
    println!(
        "cargo:rustc-env=HEX_PCM_ARC_LAYOUT_KNOWN={}",
        if known { "1" } else { "0" }
    );
    println!("cargo:rustc-env=HEX_PCM_ARC_LAYOUT_TARGET={target}");
    if !known {
        println!(
            "cargo:warning=PCM declared Arc layout Unknown: {}",
            decision.err().unwrap_or("evidence_write_failed")
        );
    }
}
