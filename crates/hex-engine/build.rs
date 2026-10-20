//! Build-only declared-layout evidence; unsupported observations remain Unknown.
#[path = "build_support/document_token_gate.rs"]
mod document_token_gate;
#[path = "build_support/midi_gate.rs"]
mod midi_gate;
#[path = "build_support/pcm_gate.rs"]
mod pcm_gate;
#[path = "build_support/retained_job_box_gate.rs"]
mod retained_job_box_gate;
#[path = "build_support/runtime_box_gate.rs"]
mod runtime_box_gate;
#[path = "build_support/state_gate.rs"]
mod state_gate;
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
    println!("cargo:rerun-if-changed=build_support/midi_gate.rs");
    println!("cargo:rerun-if-changed=build_support/state_gate.rs");
    println!("cargo:rerun-if-changed=build_support/document_token_gate.rs");
    println!("cargo:rerun-if-changed=build_support/runtime_box_gate.rs");
    println!("cargo:rerun-if-changed=build_support/retained_job_box_gate.rs");
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
    let mut evidence = format!(
        "contract: pinned-std-arc-audiodata-retained-vec-v1\nhost: {host}\ntarget: {target}\n"
    );
    let mut state_decision = Err("missing_compiler_observation");
    let mut document_token_decision = Err("missing_compiler_observation");
    let mut midi_decision = Err("missing_compiler_observation");
    let mut box_decision = Err("missing_compiler_observation");
    let mut retained_job_decision = Err("missing_compiler_observation");
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
            let observed = pcm_gate::Observation {
                verbose: &verbose,
                host: &host,
                target: &target,
                ordinary_install: installed,
                unsupported_inputs: unsupported,
            };
            state_decision = state_gate::decide(observed);
            document_token_decision = document_token_gate::decide(observed);
            midi_decision = midi_gate::decide(observed);
            box_decision = runtime_box_gate::decide(observed);
            retained_job_decision = retained_job_box_gate::decide(observed);
            pcm_gate::decide(observed)
        });
    evidence.push_str(&format!("decision: {decision:?}\nrequested layout declaration only; no security/RSS/physical allocator claim\n"));
    let saved = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .is_some_and(|path| fs::write(path.join("pcm-arc-build-evidence.txt"), &evidence).is_ok());
    let known = decision.is_ok() && saved;
    let box_evidence = format!(
        "{evidence}\ncontract: {}\ncompiler: {}\nconstructor: sized Box::new(SamplerRuntime) -> exchange_malloc(size_of::<T>(), align_of::<T>())\ndecision: {box_decision:?}\nrequested allocation only; origin/custody/peak/publication/retirement separate\n",
        runtime_box_gate::FAMILY,
        pcm_gate::COMMIT
    );
    let box_saved = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .is_some_and(|path| {
            fs::write(
                path.join("sampler-runtime-box-build-evidence.txt"),
                box_evidence,
            )
            .is_ok()
        });
    println!(
        "cargo:rustc-env=HEX_SAMPLER_BOX_LAYOUT_KNOWN={}",
        if box_decision.is_ok() && saved && box_saved {
            "1"
        } else {
            "0"
        }
    );
    println!("cargo:rustc-env=HEX_SAMPLER_BOX_LAYOUT_TARGET={target}");
    println!(
        "cargo:rustc-env=HEX_SAMPLER_BOX_CONSTRUCTOR_FAMILY={}",
        runtime_box_gate::FAMILY
    );
    let retained_job_evidence = format!(
        "{evidence}\ncontract: {}\ncompiler: {}\nconstructor: fresh sized Box::new(TypedJob<T,E>) then unsize SAME Box<dyn Job> without allocation\nlayout: actual dynamic TypedJob body (inline T plus fn pointer and padding); no Box header/vtable heap\ndecision: {retained_job_decision:?}\nrequested allocation only; payload children/custody/peak/publication/retirement separate\n",
        retained_job_box_gate::FAMILY,
        pcm_gate::COMMIT
    );
    let retained_job_saved = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .is_some_and(|path| {
            fs::write(
                path.join("retained-control-job-box-build-evidence.txt"),
                retained_job_evidence,
            )
            .is_ok()
        });
    println!(
        "cargo:rustc-env=HEX_RETAINED_JOB_BOX_LAYOUT_KNOWN={}",
        if retained_job_decision.is_ok() && saved && retained_job_saved {
            "1"
        } else {
            "0"
        }
    );
    println!("cargo:rustc-env=HEX_RETAINED_JOB_BOX_LAYOUT_TARGET={target}");
    println!(
        "cargo:rustc-env=HEX_RETAINED_JOB_BOX_CONSTRUCTOR_FAMILY={}",
        retained_job_box_gate::FAMILY
    );
    let midi_evidence = format!(
        "{evidence}\ncontract: {}\ncompiler: {}\nconstructors: sized Arc::new; sized Arc::make_mut/clone_from_ref_in/UniqueArcUninit; unique get_mut retains allocation\ndecision: {midi_decision:?}\nrequested allocation only; no RSS/provenance/custody/retirement claim\n",
        midi_gate::FAMILY,
        pcm_gate::COMMIT
    );
    let midi_saved = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .is_some_and(|path| {
            fs::write(path.join("midi-arc-build-evidence.txt"), midi_evidence).is_ok()
        });
    println!(
        "cargo:rustc-env=HEX_MIDI_ARC_LAYOUT_KNOWN={}",
        if midi_decision.is_ok() && saved && midi_saved {
            "1"
        } else {
            "0"
        }
    );
    println!("cargo:rustc-env=HEX_MIDI_ARC_LAYOUT_TARGET={target}");
    println!(
        "cargo:rustc-env=HEX_MIDI_ARC_CONSTRUCTOR_FAMILY={}",
        midi_gate::FAMILY
    );
    let state_evidence = format!(
        "{evidence}\ncontract: {}\ncompiler: {}\nconstructors: Arc::from(Vec<u8>); Arc::from(Vec<DeviceAssetReference>); Arc::new([u8;0]) then unsize\nlayout: two atomic counters extended by Layout::array(element,len), padded to alignment; input Vec capacity remains separate construction peak\ndecision: {state_decision:?}\nrequested layout only; compiler selector is not allocator/RSS/custody attestation\n",
        state_gate::FAMILY,
        pcm_gate::COMMIT
    );
    let state_saved = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .is_some_and(|path| {
            fs::write(
                path.join("canonical-state-arc-build-evidence.txt"),
                state_evidence,
            )
            .is_ok()
        });
    println!(
        "cargo:rustc-env=HEX_CANONICAL_STATE_ARC_LAYOUT_KNOWN={}",
        if state_decision.is_ok() && saved && state_saved {
            "1"
        } else {
            "0"
        }
    );
    println!("cargo:rustc-env=HEX_CANONICAL_STATE_ARC_LAYOUT_TARGET={target}");
    println!(
        "cargo:rustc-env=HEX_CANONICAL_STATE_ARC_CONSTRUCTOR_FAMILY={}",
        state_gate::FAMILY
    );
    let document_token_evidence = format!(
        "{evidence}\ncontract: {}\ncompiler: {}\npinned source: library/alloc/src/sync.rs SHA256 {}\nconstructors: closed Storage FileToken Arc::from(Vec<u8>) and Arc::from(&[u8])\nlayout: ArcInner two atomic counters extended by actual byte-slice Layout and padded; old input Vec/recovery buffer and construction overlap remain separate\ndecision: {document_token_decision:?}\nrequested native layout only; no retrospective birth/custody/publication/allocator-RSS certificate\n",
        document_token_gate::FAMILY,
        pcm_gate::COMMIT,
        document_token_gate::SOURCE_SHA256,
    );
    let document_token_saved = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .is_some_and(|path| {
            fs::write(
                path.join("document-token-arc-build-evidence.txt"),
                document_token_evidence,
            )
            .is_ok()
        });
    println!(
        "cargo:rustc-env=HEX_DOCUMENT_TOKEN_ARC_LAYOUT_KNOWN={}",
        if document_token_decision.is_ok() && saved && document_token_saved {
            "1"
        } else {
            "0"
        }
    );
    println!("cargo:rustc-env=HEX_DOCUMENT_TOKEN_ARC_LAYOUT_TARGET={target}");
    println!(
        "cargo:rustc-env=HEX_DOCUMENT_TOKEN_ARC_CONSTRUCTOR_FAMILY={}",
        document_token_gate::FAMILY
    );
    // Always override any caller text; unsupported builds retain legacy Audio.
    println!(
        "cargo:rustc-env=HEX_PCM_ARC_LAYOUT_KNOWN={}",
        if known { "1" } else { "0" }
    );
    println!("cargo:rustc-env=HEX_PCM_ARC_LAYOUT_TARGET={target}");
    println!("cargo:rustc-env=HEX_PCM_ARC_CONSTRUCTOR_FAMILY=arc-audiodata-retained-vec-v1");
    if !known {
        println!(
            "cargo:warning=PCM declared Arc layout Unknown: {}",
            decision.err().unwrap_or("evidence_write_failed")
        );
    }
}
