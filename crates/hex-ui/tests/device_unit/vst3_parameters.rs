// Compile the actual row projection and metadata cache without the native plugin or window owner.
#[allow(dead_code)]
#[path = "../../src/editor/device_panel/vst3_parameters/cache.rs"]
mod cache;
#[allow(dead_code)]
#[path = "../../src/editor/device_panel/vst3_parameters/row_projection.rs"]
mod row_projection;
