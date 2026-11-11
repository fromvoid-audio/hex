// Compile the real axes and response owners within their production module path.
use hex_engine::eq::{self, EqState, Node, NodeId};
use hex_lattice_contract::NativeDeviceState;
use hex_widgets::ResponseGraphModel;

#[allow(dead_code)]
#[path = "../../src/editor/device_panel/eq_panel/axes.rs"]
mod axes;
#[path = "../../src/editor/device_panel/eq_panel/response.rs"]
mod response;
#[path = "../../src/editor/device_panel/eq_panel/sampling.rs"]
mod sampling;
