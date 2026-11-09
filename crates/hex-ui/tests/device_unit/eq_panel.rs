// Runtime consumers use more methods than this unit-only target.
use hex_engine::eq::{self, NodeId};

#[allow(dead_code)]
#[path = "../../src/editor/device_panel/eq_panel/selection.rs"]
mod selection;
#[allow(dead_code)]
#[path = "../../src/editor/device_panel/eq_panel/slope.rs"]
mod slope;
#[allow(dead_code)]
#[path = "../../src/editor/device_panel/eq_panel/state_edit.rs"]
mod state_edit;

#[allow(dead_code)]
#[path = "../../src/editor/device_panel/eq_panel/group_move.rs"]
mod group_move;
