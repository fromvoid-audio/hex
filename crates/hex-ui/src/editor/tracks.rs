#[path = "tracks/clone_source_window.rs"]
mod clone_source_window;
#[path = "tracks/group_overlap.rs"]
mod group_overlap;
#[path = "tracks/narrow_clips.rs"]
pub(super) mod narrow_clips;

pub(super) use group_overlap::group_overlap_spans;

#[cfg(all(test, not(feature = "ui-test-support")))]
#[path = "tracks/midi_trim_unit_tests.rs"]
mod midi_trim_unit_tests;

use super::clip_boundary::clip_boundary_shell;
use super::drag::{self, ClipDrag, ClipPosition, LaneBounds};
use super::time_move_preview::{
    clip_fragments_outside, clip_intersection, clip_intersects_exclusions, move_exclusions,
    passive_audio_fragment, passive_midi_fragment, time_fragments_outside,
};
use super::trim::{TrimDrag, loop_handles};
use super::viewport::Viewport;
use super::waveform::{
    ClipTooltip, button, fade_envelope_samples, toggle_button_with_style,
    waveform_source_segment_view_cached, waveform_summary_view_cached,
};
use super::{EditorView, ViewState};
use crate::theme;
use gpui::{
    Context, Div, MouseButton, PathBuilder, canvas, div, point, prelude::*, px, relative, rgb, rgba,
};
use hex_project::{BeatTime, Clip, MidiClip, Track};

use clone_source_window::clone_source_window;

use super::clip_name;

mod clip_paint;
mod drag_ghost;
mod header;

#[cfg(all(test, feature = "ui-test-support"))]
use clip_paint::midi_note_body;
use clip_paint::{ClipPaintSource, clip_fade_layer};
pub(super) use clip_paint::{
    aligned_clip_grid_layer, clip_grid_layer, midi_note_canvas, midi_trim_note_shapes,
};
pub(super) use drag_ghost::{ClipDragGhost, alias_drag_badge, clip_waveform_body};
use drag_ghost::{ClipDragSource, clip_drag_span, drop_overlap_spans};

pub(super) const TRACK_ROW_HEIGHT: f32 = 2.0 * super::track_layout::HEADER_ONLY_HEIGHT;
/// Reserve a readable label beside the compact mixer and meter, even at the
/// minimum header width. The remaining workspace stays available to the lane.
pub(super) const TRACK_HEADER_WIDTH: f32 = 256.0;
pub(super) const DEFAULT_TRACK_HEADER_WIDTH: f32 = TRACK_HEADER_WIDTH;

pub(super) struct ClipViewTimeline<'a> {
    pub(super) viewport: Viewport,
    pub(super) grid: &'a std::rc::Rc<[super::musical_time::GridLine]>,
    pub(super) scale_factor: f32,
    // Present only when the content owner has the lane resolved in this prepaint.
    pub(super) caption_rem_size: Option<f32>,
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "tracks/tests.rs"]
mod overlap_tests;
