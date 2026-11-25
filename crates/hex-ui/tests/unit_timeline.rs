//! Unit tests for timeline geometry without a window or service.

#[path = "."]
mod editor {
    #[allow(unused_imports)]
    use hex_ui_model::{elapsed_time, musical_time, seek, signed_frame_viewport, viewport};

    #[path = "timeline_unit/drag_autoscroll.rs"]
    mod drag_autoscroll;

    #[path = "timeline_unit/drag.rs"]
    mod drag;

    #[path = "timeline_unit/loop_rail.rs"]
    mod loop_rail;

    #[path = "timeline_unit/midi_cue.rs"]
    mod midi_cue;
}
