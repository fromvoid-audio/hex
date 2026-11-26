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

fn strip_toml_comment(line: &str) -> Result<&str, ()> {
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut in_double = false;
    let mut in_single = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_double {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == b'"' {
                in_double = false;
            }
        } else if in_single {
            if byte == b'\'' {
                in_single = false;
            }
        } else {
            match byte {
                b'"' => in_double = true,
                b'\'' => in_single = true,
                b'#' => return Ok(&line[..index]),
                _ => {}
            }
        }
        index += 1;
    }
    if in_double || in_single {
        return Err(());
    }
    Ok(line)
}

fn update_bracket_depth(text: &str, depth: &mut usize) -> Result<(), ()> {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut in_double = false;
    let mut in_single = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_double {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == b'"' {
                in_double = false;
            }
        } else if in_single {
            if byte == b'\'' {
                in_single = false;
            }
        } else {
            match byte {
                b'"' => in_double = true,
                b'\'' => in_single = true,
                b'[' | b'{' => *depth += 1,
                b']' | b'}' => {
                    if *depth == 0 {
                        return Err(());
                    }
                    *depth -= 1;
                }
                _ => {}
            }
        }
        index += 1;
    }
    if in_double || in_single {
        return Err(());
    }
    Ok(())
}

fn section_overrides_source(section: &str) -> bool {
    let normalized = section
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_ascii_lowercase();
    normalized == "source"
        || normalized.starts_with("source.")
        || normalized == "patch"
        || normalized.starts_with("patch.")
        || normalized == "registries.crates-io"
        || normalized.starts_with("registries.crates-io.")
        || normalized.starts_with("registries.\"crates-io\"")
        || normalized.starts_with("registries.'crates-io'")
        || normalized.contains("rtrb")
}

fn key_overrides_source(section: &str, key: &str) -> bool {
    let normalized = key
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_ascii_lowercase();
    if normalized == "paths"
        || normalized == "replace-with"
        || normalized.starts_with("source.")
        || normalized.starts_with("patch.")
        || normalized.starts_with("registries.crates-io")
        || normalized.contains("rtrb")
    {
        return true;
    }
    if section == "env" || section.starts_with("env.") || normalized.starts_with("env.") {
        return normalized.contains("cargo_source_")
            || normalized.contains("cargo_registries_crates_io_index");
    }
    false
}

/// Conservatively inspect a Cargo config file and refuse only if it is
/// unparseable/malformed TOML or declares a source/patch/registry/env override
/// that could replace `rtrb` or `crates-io`. Unrelated developer configs
/// (`[build]`, `[target.*]`, `[net]`, `[alias]`, `[term]`, `[profile.*]`) do
/// not disable the pinned `rtrb` source observation.
fn config_may_override_rtrb_source(contents: &str) -> bool {
    let mut current_section = String::new();
    let mut bracket_depth = 0usize;
    for raw_line in contents.lines() {
        // Refuse multiline quoted strings conservatively to avoid skipping
        // embedded overrides or mis-tracking quotes.
        if raw_line.contains("\"\"\"") || raw_line.contains("'''") {
            return true;
        }
        let Ok(uncommented) = strip_toml_comment(raw_line) else {
            return true;
        };
        let line = uncommented.trim();
        if line.is_empty() {
            continue;
        }
        if bracket_depth > 0 {
            if update_bracket_depth(line, &mut bracket_depth).is_err() {
                return true;
            }
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let inner = if line.starts_with("[[") && line.ends_with("]]") && line.len() >= 4 {
                line[2..line.len() - 2].trim()
            } else if line.len() >= 2 {
                line[1..line.len() - 1].trim()
            } else {
                return true;
            };
            if inner.is_empty() || inner.contains('[') || inner.contains(']') {
                return true;
            }
            if section_overrides_source(inner) {
                return true;
            }
            current_section = inner.to_ascii_lowercase();
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            // Non-TOML / unrecognized directive: refuse conservatively.
            return true;
        };
        let key = raw_key.trim();
        let value = raw_value.trim();
        if key.is_empty() || value.is_empty() {
            return true;
        }
        if key_overrides_source(&current_section, key) {
            return true;
        }
        if update_bracket_depth(value, &mut bracket_depth).is_err() {
            return true;
        }
    }
    bracket_depth != 0
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
                let Ok(contents) = fs::read_to_string(&path) else {
                    return;
                };
                if config_may_override_rtrb_source(&contents) {
                    return;
                }
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
