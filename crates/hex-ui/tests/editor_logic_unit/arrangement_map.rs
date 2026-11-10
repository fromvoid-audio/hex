#[allow(dead_code)]
#[path = "../../src/editor/arrangement_map/model.rs"]
mod model;

pub(super) use model::{Interval, append_track_intervals, merge};
