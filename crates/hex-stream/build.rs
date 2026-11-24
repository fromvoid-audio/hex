//! Supported-construction declaration for the fixed repository Cargo invocation.
//! Observed source files are not general resolved-source or runtime attestation.
use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};
// SHA output arrays no longer implement LowerHex; keep durable 64-byte text.
fn digest_hex(digest: [u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    output
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-env-changed=CARGO_HOME");
    println!("cargo:rerun-if-env-changed=HOME");
    #[cfg(windows)]
    println!("cargo:rerun-if-env-changed=USERPROFILE");
    println!("cargo:rerun-if-env-changed=HEX_GUARDED_CARGO_CONFIG_FINGERPRINT");
    println!("cargo:rustc-check-cfg=cfg(hex_stream_rtrb_source_observation_matches)");
    let cargo_home = env::var_os("CARGO_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let home = env::var_os("HOME").filter(|p| !p.is_empty());
            #[cfg(windows)]
            let home = home.or_else(|| env::var_os("USERPROFILE").filter(|p| !p.is_empty()));
            home.map(|p| PathBuf::from(p).join(".cargo"))
        });
    let Some(source) = cargo_home.map(|p| p.join("registry/src")) else {
        return;
    };
    // Refuse observed configurations that can change the supported constructor.
    // Cargo does not expose every CLI --config/source override to build scripts:
    // this observation is valid only with the fixed repository invocation and
    // exact required CI, not arbitrary Cargo invocations. Runtime still works.
    let Some(home) = source.parent().and_then(|p| p.parent()) else {
        return;
    };
    let mut config_dirs = vec![home.to_path_buf()];
    let Ok(cwd) = env::current_dir() else { return };
    config_dirs.extend(cwd.ancestors().map(|p| p.join(".cargo")));
    for directory in config_dirs {
        for name in ["config", "config.toml"] {
            let path = directory.join(name);
            // Supported entrypoints fingerprint absent config paths via
            // HEX_GUARDED_CARGO_CONFIG_FINGERPRINT before and after Cargo.
            // Never emit nonexistent rerun-if-changed paths (or recursive
            // ~/.cargo watches), which force every raw Cargo invocation to
            // recompile hex-stream -> hex-audio -> hex-ui.
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
                return;
            }
        }
    }
    for name in [
        "CARGO_SOURCE_CRATES_IO_REPLACE_WITH",
        "CARGO_REGISTRIES_CRATES_IO_INDEX",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
        if env::var_os(name).is_some_and(|v| !v.is_empty()) {
            return;
        }
    }
    if env::vars_os().any(|(key, _)| key.to_string_lossy().starts_with("CARGO_SOURCE_")) {
        return;
    }

    let Ok(entries) = fs::read_dir(source) else {
        return;
    };
    let candidates: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|p| p.path().join("rtrb-0.3.4"))
        .filter(|p| p.is_dir())
        .collect();
    if candidates.len() != 1 {
        return;
    }
    let files = [
        (
            "src/lib.rs",
            "d40fd482f8fd2bd08cb7b30077289e44995b348f6fe31a85b946627660e2f320",
        ),
        (
            "src/cache_padded.rs",
            "707618530d2b6907e63f212575774552c888ff9c979318a6d4f5a913259f2b5e",
        ),
        (
            "src/chunks.rs",
            "f86b041da9ca092f5ca082ece4446cc9939ca2c7a579bce81803ff118168e091",
        ),
    ];
    for (file, pin) in files {
        let path = candidates[0].join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let Ok(bytes) = fs::read(path) else { return };
        if digest_hex(Sha256::digest(bytes).into()) != pin {
            return;
        }
    }
    let Ok(lock) = fs::read_to_string("../../Cargo.lock") else {
        return;
    };
    let lock = lock.replace("\r\n", "\n");
    if !lock.contains("name = \"rtrb\"\nversion = \"0.3.4\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"4ade083ccbb4bf536df69d1f6432cc23deb7acccff86b183f3923a6fd56a1153\"") {
        return;
    }
    println!("cargo:rustc-cfg=hex_stream_rtrb_source_observation_matches");
}
