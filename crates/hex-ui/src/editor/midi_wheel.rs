//! Piano-roll wheel navigation, following Live's editor conventions.
//!
//! Plain vertical wheel scrolls pitch, horizontal (trackpad) or Shift-wheel pans
//! time, Cmd-wheel zooms time around the pointer and Alt-wheel zooms pitch.
//! Scrolling follows the platform's scroll-direction preference (`delta`); zoom
//! keeps the device direction (`physical_delta`) shared with Arrange.
//!
//! Navigation is view-only and never edits the project. A held note gesture owns
//! the pointer through Window capture, so wheel input cannot reach these handlers
//! while a drag is in flight; the drag survives and the viewport stays fixed.
use super::EditorView;
pub(super) use super::midi_wheel_mapping::{WheelNav, resolve};

impl EditorView {
    /// Scroll the piano-roll viewport by pixel deltas over `bounds`
    /// (`[x, y, width, height]`). Pitch accumulates fractional rows; time maps
    /// pixels through the current span. Returns whether the viewport changed.
    pub(super) fn scroll_piano_wheel(&mut self, bounds: [f32; 4], pitch: f32, time: f32) -> bool {
        if self.context_menu.is_some()
            || !pitch.is_finite()
            || !time.is_finite()
            || !bounds.iter().all(|value| value.is_finite())
            || bounds[2] <= 0.0
            || bounds[3] <= 0.0
        {
            return false;
        }
        let Some(_) = self
            .midi_clip()
            .map(|source| source.source_length().raw())
            .filter(|length| *length > 0)
        else {
            return false;
        };
        let mut viewport = self.piano_pitch;
        if pitch != 0.0 {
            let Some(row_height) = viewport.row_height_pixels(bounds[3]) else {
                return false;
            };
            self.piano_pitch_scroll_accum =
                (self.piano_pitch_scroll_accum + pitch).clamp(-bounds[3], bounds[3]);
            let steps = (self.piano_pitch_scroll_accum / row_height).trunc();
            self.piano_pitch_scroll_accum -= steps * row_height;
            let (target, clamped) = viewport.scroll_rows(steps as i16);
            if clamped {
                // Do not bank motion against a MIDI edge.
                self.piano_pitch_scroll_accum = 0.0;
            }
            viewport = target;
        }
        let span = self.piano_beat_span.raw();
        let mut start = self.piano_beat_start;
        if time != 0.0 && span > 0 {
            let ticks = (f64::from(time) / f64::from(bounds[2]) * span as f64)
                .round()
                .clamp(-(u64::MAX as f64), u64::MAX as f64) as i128;
            let next = start - ticks;
            start = super::midi_scene::clamp_start(next, span);
        }
        if (viewport, start) == (self.piano_pitch, self.piano_beat_start) {
            return false;
        }
        // Viewport-bound gesture state is retired, as for every other navigation.
        self.cancel_midi_note_gesture();
        self.piano_pitch = viewport;
        self.piano_beat_start = start;
        true
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_wheel_ui_tests.rs"]
mod ui_tests;
