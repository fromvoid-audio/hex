//! # Purpose
//! Own reusable product-themed GPUI presentation primitives shared by the full
//! `hex-ui` editor and the standalone `hex-sandbox` widget/layout catalog:
//! bundled fonts and theme tokens, embedded SVG assets, popup context menus,
//! device panel composition shells, output meter presentation, and section
//! surface/hit/input primitives.
//!
//! # Dependencies
//! Uses pinned `gpui`, `hex-widgets`, `hex-devices` (EQ state/node types),
//! `hex-preferences`, `hex-project`, `hex-text`, and `hex-ui-model`.
//!
//! # Does not own
//! Does not own audio playback, DSP, session workers, storage I/O, project
//! mutation history, device implementations, or the full Arrangement/Session
//! editor view.
pub mod assets;
pub mod context_menu;
pub mod device_composition;
pub mod eq_panel;
pub mod output_meter;
pub mod section_hit;
pub mod section_input;
pub mod section_surface;
pub mod theme;
