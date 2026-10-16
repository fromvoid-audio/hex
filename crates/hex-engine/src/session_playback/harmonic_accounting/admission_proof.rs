//! Private proof only: no codec, factory, pool, publisher or callback hook.
#[path = "inventory.rs"]
mod inventory;
#[allow(dead_code)] // Layout fields reserve real bytes; no sampling law is implemented.
#[path = "payload.rs"]
mod payload;
#[path = "planner.rs"]
mod planner;
#[path = "tests.rs"]
mod tests;
