//! Disconnected inline candidate; no production publication or callback hooks.
#[path = "inline_candidate/enable.rs"]
pub(super) mod enable;
#[path = "inline_candidate/enable_tests.rs"]
mod enable_tests;
#[path = "inline_candidate/ownership_tests.rs"]
mod ownership_tests;
#[path = "inline_candidate/store.rs"]
mod store;

#[path = "inline_candidate/root_leases.rs"]
mod root_leases;
#[path = "inline_candidate/root_refs.rs"]
#[allow(dead_code)] // Layout proof; no production store hooks.
mod root_refs;
#[path = "inline_candidate/root_tests.rs"]
mod root_tests;

#[path = "inline_candidate/root_budget_tests.rs"]
mod root_budget_tests;
