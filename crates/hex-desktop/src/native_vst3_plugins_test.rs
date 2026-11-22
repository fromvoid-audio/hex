//! Opt-in native macOS arm64 VST3 SDK integration test target.
//!
//! Kept out of `src/main.rs` so default `--bins` and `allocation-probe`
//! builds do not compile or link the 273 ignored native SDK tests under
//! `tests/native_vst3_plugins/`. Run explicitly with:
//! `cargo test -p hex-desktop --features ui-test-support --test native_vst3_plugins --locked -- --ignored <filter>`
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#![allow(dead_code, unused_imports)]

mod audio_policy;
mod granular_sources;
mod native_audio;
mod native_sources;
mod startup_failure;
mod streamed_export;

pub(crate) use native_audio::tests::*;

#[path = "../tests/native_vst3_plugins/mod.rs"]
mod plugins;
