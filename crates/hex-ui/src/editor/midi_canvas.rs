//! Compact piano-roll Canvas rendering and pointer interaction.
//!
//! Wheel navigation changes horizontal time only; a vertical drag on the key
//! gutter changes the centered pitch span (1–128 rows). Both are view-only.
//! Middle-drag or explicit pan mode moves both axes without editing.
//! Every paint, hit and captured edit context carries the same pitch span.
//! Passive hover reuses click/resize hits and never changes musical selection.
#[path = "midi_canvas/geometry.rs"]
mod geometry;
#[path = "midi_note_pointer.rs"]
mod note_pointer;

use super::midi_drag::{self, NotePreview};
use super::midi_draw::StrokeModifiers;
use super::{EditorView, midi::NoteField};
use crate::theme;
use geometry::{beat_at_x, beat_label, source_beat_label};
use gpui::{
    Bounds, Context, Corners, Div, Edges, MouseButton, MouseDownEvent, MouseMoveEvent,
    ScrollWheelEvent, Window, canvas, div, fill, point, prelude::*, px, relative, rgb, size,
};
use hex_project::{BeatTime, Command};
use hex_session::service::Request;

fn gesture_bounds_from(bounds: Bounds<gpui::Pixels>) -> [f32; 4] {
    [
        f32::from(bounds.origin.x),
        f32::from(bounds.origin.y),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
    ]
}

const MIDI_TOOLBAR_HEIGHT: f32 = 28.0;
const PANEL_TOP_BORDER: f32 = 1.0;
const NOTE_GRID_BORDER: f32 = 1.0;
pub(super) const NOTE_ROW_INSET: f32 = 0.5;
const NOTE_CORNER_RADIUS: f32 = 6.0;

mod gestures;
mod keys_bar;
mod paint;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_creation_ui_tests.rs"]
pub(super) mod creation_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_zoom_ui_tests.rs"]
mod zoom_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_note_overwrite_ui_tests.rs"]
mod overwrite_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_hover_ui_tests.rs"]
mod hover_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_marquee_ui_tests.rs"]
mod marquee_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_cursor_ui_tests.rs"]
mod cursor_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_step_ui_tests.rs"]
mod step_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_audition_ui_tests.rs"]
mod audition_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_created_selection_ui_tests.rs"]
mod created_selection_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_pan_ui_tests.rs"]
mod pan_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_resize_ui_tests.rs"]
mod resize_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_note_copy_ui_tests.rs"]
mod copy_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_input_refinement_ui_tests.rs"]
mod input_refinement_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_pending_drag_ui_tests.rs"]
mod pending_drag_tests;
