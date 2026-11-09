// These source modules share pure geometry contracts. Their production callers
// use more methods than this unit-only target, so unused items are expected here.
#[allow(dead_code)]
#[path = "../../src/editor/midi_drag.rs"]
mod midi_drag;
#[allow(dead_code)]
#[path = "../../src/editor/midi_scene.rs"]
mod midi_scene;
#[allow(dead_code)]
#[path = "../../src/editor/midi_viewport_grid.rs"]
mod midi_viewport_grid;
