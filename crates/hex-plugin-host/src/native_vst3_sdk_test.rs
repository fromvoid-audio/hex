//! Opt-in native macOS arm64 VST3 SDK integration test target.
//!
//! Relocated out of `src/lib.rs` so default `--lib` and `allocation-probe`
//! builds do not compile or link the 108 ignored native SDK tests under
//! `tests/native_vst3_sdk/`. Run explicitly with:
//! `cargo test -p hex-plugin-host --features vst3-sdk-tests --test native_vst3_sdk --locked -- --ignored <filter>`
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#![allow(dead_code, unused_imports)]

macro_rules! include_vst3_sdk_tests {
    ($($item:item)*) => {
        $($item)*
    };
}
pub(crate) use include_vst3_sdk_tests;

mod channel;
mod error;
pub mod native;
pub mod protocol;
pub mod transport;

pub(crate) use error::runtime_host_refusal;
pub use error::{CustodyError, Error, NATIVE_DIAGNOSTIC_DATA_BYTES, Result};
