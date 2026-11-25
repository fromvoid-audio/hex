//! Shared EQ panel geometry, coordinate, sampling, and selection helpers over
//! `hex-devices::eq`, `hex-widgets`, and `hex-ui-model::eq_panel`.
use hex_devices::eq::{self, EqState, NodeId};

pub mod axes;
pub mod group_move;
pub mod node_geometry;
pub mod sampling;
pub mod selection;
pub use hex_ui_model::eq_panel::slope;
