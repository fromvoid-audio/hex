// Compile the real axes and response owners within their production module path.
use hex_engine::eq::{self, EqState, Node, NodeId};
use hex_lattice_contract::NativeDeviceState;
use hex_ui_kit::eq_panel::sampling;
use hex_widgets::ResponseGraphModel;

#[path = "../../src/editor/device_panel/eq_panel/response.rs"]
mod response;
