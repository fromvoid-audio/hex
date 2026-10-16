//! Disconnected typed proposal. No codec, runtime/provider registration or hooks.
#[path = "capture_proof/adapter.rs"]
mod adapter;
#[path = "capture_proof/layout.rs"]
#[allow(dead_code)] // Concrete layout-only fields are measured, not runtime hooks.
mod layout;
#[path = "capture_proof/leases.rs"]
mod leases;
#[path = "capture_proof/ownership_tests.rs"]
mod ownership_tests;
#[path = "capture_proof/tests.rs"]
mod tests;

#[path = "capture_proof/inline_candidate.rs"]
mod inline_candidate;
