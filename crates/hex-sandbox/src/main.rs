//! # Purpose
//! Run the disposable native widget and device-layout catalog using the real
//! production `hex-ui-kit` and `hex-ui-model` owners.
//!
//! # Dependencies
//! Uses GPUI, `hex-ui-kit`, `hex-ui-model`, `hex-widgets`, `hex-preferences`,
//! `hex-project`, and `hex-devices` for prepared device EQ queries.
//!
//! # Does not own
//! Does not open projects, initialize audio, write preferences, or own
//! application state.

mod gallery;
mod launch;
mod sandbox;

fn main() {
    launch::run();
}
