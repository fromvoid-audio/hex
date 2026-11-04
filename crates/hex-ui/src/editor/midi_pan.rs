//! View-only, captured two-axis piano-roll navigation. The time origin has a
//! checked numeric bound independent of source length; dragging never edits data.
//! Pointer deltas are resolved from the captured viewport, avoiding rounding drift.
//! Window owns held input; its read-only canvas witness never retains pending edits.
use super::{EditorView, midi_scene, midi_stroke::CanvasBounds};
use gpui::{Context, MouseButton, MouseMoveEvent, MouseUpEvent, Window};
use hex_project::{BeatTime, ClipId};

#[derive(Default)]
pub(super) struct State {
    pub(super) enabled: bool,
    pub(super) gesture: Option<Gesture>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Viewport {
    start: u64,
    span: u64,
    low: u8,
    rows: u8,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Gesture {
    generation: u64,
    revision: u64,
    clip: ClipId,
    length: u64,
    bounds: CanvasBounds,
    origin: [f32; 2],
    viewport: Viewport,
    button: MouseButton,
}

impl Gesture {
    fn resolve(&self, pointer: [f32; 2]) -> Option<Viewport> {
        if !pointer.iter().all(|value| value.is_finite()) {
            return None;
        }
        let dx = (f64::from(pointer[0]) - f64::from(self.origin[0])) / f64::from(self.bounds.width);
        let dy =
            (f64::from(pointer[1]) - f64::from(self.origin[1])) / f64::from(self.bounds.height);
        let ticks = (dx * self.viewport.span as f64)
            .round()
            .clamp(-(u64::MAX as f64), u64::MAX as f64) as i128;
        let start = (i128::from(self.viewport.start) - ticks).clamp(0, i128::from(u64::MAX)) as u64;
        let pitches = (dy * f64::from(self.viewport.rows))
            .round()
            .clamp(-128.0, 128.0) as i16;
        Some(Viewport {
            start: midi_scene::clamp_start(start, self.viewport.span),
            low: (i16::from(self.viewport.low) + pitches)
                .clamp(0, 128 - i16::from(self.viewport.rows)) as u8,
            ..self.viewport
        })
    }
}

impl EditorView {
    /// Retire capture after a canonical context change without resetting navigation.
    pub(super) fn reconcile_piano_pan(&mut self) {
        if self.midi_pan.gesture.as_ref().is_some_and(|gesture| {
            gesture.generation != self.project_generation
                || self.detail_clip_id() != Some(gesture.clip)
                || self.state.as_ref().is_none_or(|state| {
                    state.revision != gesture.revision
                        || state
                            .project
                            .midi_source(gesture.clip)
                            .is_none_or(|source| source.source_length().raw() != gesture.length)
                })
        }) {
            self.midi_pan.gesture = None;
        }
    }

    pub(super) fn begin_piano_pan(
        &mut self,
        bounds: CanvasBounds,
        pointer: [f32; 2],
        button: MouseButton,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.context_menu.is_some()
            || !bounds.is_valid()
            || !pointer.iter().all(|value| value.is_finite())
            || !(button == MouseButton::Middle
                || (button == MouseButton::Left && self.midi_pan.enabled))
        {
            return false;
        }
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some(clip) = self.detail_clip_id() else {
            return false;
        };
        let Some(source) = state.project.midi_source(clip) else {
            return false;
        };
        let length = source.source_length().raw();
        let viewport = Viewport {
            start: self.piano_beat_start.raw(),
            span: self.piano_beat_span.raw(),
            low: self.piano_pitch_low,
            rows: self.piano_pitch_rows,
        };
        if length == 0
            || viewport.span == 0
            || viewport.span > midi_scene::zoom_limit(length)
            || midi_scene::clamp_start(viewport.start, viewport.span) != viewport.start
            || !midi_scene::valid_pitch_range(viewport.low, viewport.rows)
        {
            return false;
        }
        let gesture = Gesture {
            generation: self.project_generation,
            revision: state.revision,
            clip,
            length,
            bounds,
            origin: pointer,
            viewport,
            button,
        };
        if !self.capture_piano_pan(gesture, window, cx) {
            // A secondary middle press cannot replace Window's original left
            // press. Still retire its unleased note edit: a later left release
            // must not commit the creation that the navigation request cancelled.
            // Another leased owner never dispatches this press to the canvas.
            if button == MouseButton::Middle && self.cancel_midi_note_gesture() {
                cx.notify();
            }
            return false;
        }
        self.cancel_midi_note_gesture();
        self.midi_hover = None;
        self.midi_pan.gesture = Some(gesture);
        true
    }

    fn capture_piano_pan(&self, gesture: Gesture, window: &mut Window, cx: &Context<Self>) -> bool {
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        window.capture_pointer(
            cx.entity_id(),
            gesture.button,
            gpui::CursorStyle::ClosedHand,
            move |event, window, cx| {
                let moved = event.downcast_ref::<MouseMoveEvent>();
                let released = event
                    .downcast_ref::<MouseUpEvent>()
                    .filter(|e| e.button == gesture.button);
                let Some(position) = moved
                    .map(|e| e.position)
                    .or_else(|| released.map(|e| e.position))
                else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if view.midi_pan.gesture.as_ref() != Some(&gesture) {
                        return;
                    }
                    let changed = if !view.clip_view_open
                        || view.device_panel_active
                        || !view.midi_focus.is_focused(window)
                    {
                        view.midi_pan.gesture.take().is_some()
                    } else if released.is_some() {
                        view.finish_piano_pan(
                            gesture.bounds,
                            [f32::from(position.x), f32::from(position.y)],
                            gesture.button,
                        )
                    } else {
                        view.update_piano_pan(
                            gesture.bounds,
                            [f32::from(position.x), f32::from(position.y)],
                            moved.and_then(|e| e.pressed_button),
                        )
                    };
                    if changed {
                        cx.notify();
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    if view.midi_pan.gesture.as_ref() == Some(&gesture) {
                        view.midi_pan.gesture = None;
                        cx.notify();
                    }
                });
            },
            cx,
        )
    }
    /// Canonical reconciliation runs before the canvas. Witness its held bounds
    /// and actual rendered source without another project/source lookup.
    pub(super) fn renew_piano_pan_capture(
        &self,
        bounds: CanvasBounds,
        clip: ClipId,
        owner: gpui::EntityId,
        window: &mut Window,
    ) {
        if self.clip_view_open
            && !self.device_panel_active
            && self.midi_focus.is_focused(window)
            && let Some(g) = self.midi_pan.gesture.as_ref()
            && g.bounds == bounds
            && g.clip == clip
            && g.generation == self.project_generation
            && self
                .state
                .as_ref()
                .is_some_and(|s| s.revision == g.revision)
        {
            window.renew_pointer_capture(owner, gpui::CursorStyle::ClosedHand);
        }
    }

    pub(super) fn update_piano_pan(
        &mut self,
        bounds: CanvasBounds,
        pointer: [f32; 2],
        button: Option<MouseButton>,
    ) -> bool {
        let Some(gesture) = self.midi_pan.gesture.as_ref() else {
            return false;
        };
        if button != Some(gesture.button)
            || gesture.bounds != bounds
            || gesture.generation != self.project_generation
            || self.detail_clip_id() != Some(gesture.clip)
            || self.state.as_ref().is_none_or(|state| {
                state.revision != gesture.revision
                    || state
                        .project
                        .midi_source(gesture.clip)
                        .is_none_or(|source| source.source_length().raw() != gesture.length)
            })
        {
            self.midi_pan.gesture = None;
            return true;
        }
        let Some(next) = gesture.resolve(pointer) else {
            self.midi_pan.gesture = None;
            return true;
        };
        let changed = next.start != self.piano_beat_start.raw() || next.low != self.piano_pitch_low;
        self.piano_beat_start = BeatTime::from_raw(next.start);
        self.piano_pitch_low = next.low;
        changed
    }

    pub(super) fn finish_piano_pan(
        &mut self,
        bounds: CanvasBounds,
        pointer: [f32; 2],
        button: MouseButton,
    ) -> bool {
        if self
            .midi_pan
            .gesture
            .as_ref()
            .is_none_or(|gesture| gesture.button != button)
        {
            return false;
        }
        self.update_piano_pan(bounds, pointer, Some(button));
        self.midi_pan.gesture = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};

    fn gesture(length: u64, viewport: Viewport) -> Gesture {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 2).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack { name: "pan".into() })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        Gesture {
            generation: 0,
            revision: 0,
            clip,
            length,
            bounds: CanvasBounds {
                x: 100.0,
                y: 200.0,
                width: 400.0,
                height: 240.0,
            },
            origin: [300.0, 320.0],
            viewport,
            button: MouseButton::Middle,
        }
    }

    #[test]
    fn captured_deltas_preserve_fractional_time_and_pitch_without_rounding_drift() {
        let beat = BeatTime::ONE.raw();
        let g = gesture(
            beat / 4,
            Viewport {
                start: 2 * beat,
                span: 4 * beat,
                low: 48,
                rows: 24,
            },
        );
        let expected = Viewport {
            start: 3 * beat,
            low: 54,
            ..g.viewport
        };
        assert_eq!(g.resolve([200.0, 380.0]), Some(expected));
        for _ in 0..50 {
            assert_eq!(g.resolve([200.0, 380.0]), Some(expected));
        }
        assert_eq!(g.resolve(g.origin), Some(g.viewport));
        assert!(g.resolve([f32::NAN, 0.0]).is_none());
        assert!(g.resolve([0.0, f32::INFINITY]).is_none());
    }

    #[test]
    fn extreme_coordinates_remain_bounded_at_minimum_and_maximum_spans() {
        for (length, span, rows) in [(1, 1, 1), (u64::MAX, u64::MAX - 1, 128)] {
            let g = gesture(
                length,
                Viewport {
                    start: 0,
                    span,
                    low: 0,
                    rows,
                },
            );
            let first = g.resolve([f32::MAX, f32::MAX]).unwrap();
            assert_eq!(first.start, 0);
            assert_eq!(first.low, 128 - rows);
            let last = g.resolve([-f32::MAX, -f32::MAX]).unwrap();
            assert_eq!(last.start, u64::MAX - span);
            assert_eq!(last.low, 0);
            assert!(last.start.checked_add(last.span).is_some());
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_pan_capture_tests.rs"]
mod capture_tests;
