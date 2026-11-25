//! Piano-roll geometry unit tests, without a window or service.

#[path = "."]
mod editor {
    #[allow(unused_imports)]
    use hex_ui_model::midi_pitch_viewport::PitchViewport;
    #[allow(unused_imports)]
    use hex_ui_model::{
        midi_drag, midi_group_drag, midi_pitch_viewport, midi_scene, midi_selection, midi_stroke,
        midi_velocity, midi_viewport_grid,
    };

    #[allow(dead_code)]
    #[path = "../src/editor/midi_source_controls.rs"]
    mod midi_source_controls;
    #[allow(dead_code)]
    #[path = "../src/editor/midi_wheel_mapping.rs"]
    mod midi_wheel_mapping;

    #[path = "piano_roll_unit/midi_canvas.rs"]
    mod midi_canvas;

    #[path = "piano_roll_unit/midi_ruler.rs"]
    mod midi_ruler;
}
