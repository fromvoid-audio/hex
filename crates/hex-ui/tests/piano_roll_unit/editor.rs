// These source modules share pure geometry contracts. Their production callers
// use more methods than this unit-only target, so unused items are expected here.
#[allow(dead_code)]
#[path = "../../src/editor/midi_drag.rs"]
mod midi_drag;
#[allow(dead_code)]
#[path = "../../src/editor/midi_group_drag.rs"]
mod midi_group_drag;
#[allow(dead_code)]
#[path = "../../src/editor/midi_scene.rs"]
mod midi_scene;
#[allow(dead_code)]
#[path = "../../src/editor/midi_selection.rs"]
mod midi_selection;
#[allow(dead_code)]
#[path = "../../src/editor/midi_source_controls.rs"]
mod midi_source_controls;
#[allow(dead_code)]
#[path = "../../src/editor/midi_stroke.rs"]
mod midi_stroke;
#[allow(dead_code)]
#[path = "../../src/editor/midi_velocity.rs"]
mod midi_velocity;
#[allow(dead_code)]
#[path = "../../src/editor/midi_viewport_grid.rs"]
mod midi_viewport_grid;
#[allow(dead_code)]
#[path = "../../src/editor/midi_wheel_mapping.rs"]
mod midi_wheel_mapping;
