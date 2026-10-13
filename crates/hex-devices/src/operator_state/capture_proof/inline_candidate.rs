//! Disconnected inline candidate; no production publication or callback hooks.
#[path = "inline_candidate/enable.rs"]
pub(super) mod enable;
#[path = "inline_candidate/enable_tests.rs"]
mod enable_tests;
#[path = "inline_candidate/ownership_tests.rs"]
mod ownership_tests;
#[path = "inline_candidate/store.rs"]
mod store;
