#[cfg(all(test, not(feature = "ui-test-support")))]
#[test]
fn timeline_shortcuts_parse() {
    use super::{
        FitTimeline, KeyBinding, PanTimelineLeft, PanTimelineRight, SaveAsProject, SaveProject,
        SelectAllMidiNotes, ToggleMidiDrawMode, ZoomIn, ZoomOut, escape_binding,
    };

    let _zoom_in = KeyBinding::new("cmd-=", ZoomIn, Some("hex_editor"));
    let _zoom_out = KeyBinding::new("cmd--", ZoomOut, Some("hex_editor"));
    let _fit = KeyBinding::new("cmd-0", FitTimeline, Some("hex_editor"));
    let _pan_left = KeyBinding::new("alt-left", PanTimelineLeft, Some("hex_editor"));
    let _pan_right = KeyBinding::new("alt-right", PanTimelineRight, Some("hex_editor"));
    let _save = KeyBinding::new("cmd-s", SaveProject, Some("hex_editor"));
    let _save_as = KeyBinding::new("cmd-shift-s", SaveAsProject, Some("hex_editor"));
    let _escape = escape_binding();
    let _select_all_midi = KeyBinding::new("cmd-a", SelectAllMidiNotes, Some("hex_midi"));
    let _toggle_midi_draw = KeyBinding::new("b", ToggleMidiDrawMode, Some("hex_midi"));
}
