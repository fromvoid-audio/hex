//! View-only, captured two-axis piano-roll navigation. The time origin has a
//! checked numeric bound independent of source length; dragging never edits data.
//! Pointer deltas are resolved from the captured viewport, avoiding rounding drift.
//! Window owns held input; its read-only canvas witness never retains pending edits.
use super::{EditorView, midi_scene, midi_stroke::CanvasBounds};
use gpui::{Context, MouseButton, MouseMoveEvent, MouseUpEvent, Window};
use hex_project::ClipId;

#[derive(Default)]
pub(super) struct State {
    pub(super) enabled: bool,
    pub(super) gesture: Option<Capture>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Viewport {
    start: i128,
    span: u64,
    pitch: super::PitchViewport,
}

#[derive(Clone, Copy, PartialEq)]
struct Gesture {
    generation: u64,
    revision: u64,
    clip: ClipId,
    length: u64,
    bounds: CanvasBounds,
    origin: [f32; 2],
    viewport: Viewport,
    button: MouseButton,
}

pub(super) struct Capture {
    origin: Gesture,
    applied_pitch: super::PitchViewport,
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
        let start =
            (self.viewport.start - ticks).clamp(-i128::from(u64::MAX), i128::from(u64::MAX));
        let pitches = (dy * self.viewport.pitch.span())
            .round()
            .clamp(-128.0, 128.0) as i16;
        Some(Viewport {
            start: midi_scene::clamp_start(start, self.viewport.span),
            pitch: self.viewport.pitch.scroll_rows(pitches).0,
            ..self.viewport
        })
    }
}

impl EditorView {
    /// Retire capture after a canonical context change without resetting navigation.
    pub(super) fn reconcile_piano_pan(&mut self) {
        if self
            .midi_editor
            .pan
            .gesture
            .as_ref()
            .is_some_and(|capture| {
                let gesture = capture.origin;
                self.midi_editor.piano_pitch != capture.applied_pitch
                    || gesture.generation != self.project_generation
                    || self.detail_clip_id() != Some(gesture.clip)
                    || self.state.as_ref().is_none_or(|state| {
                        state.revision != gesture.revision
                            || state
                                .project
                                .midi_source(gesture.clip)
                                .is_none_or(|source| source.source_length().raw() != gesture.length)
                    })
            })
        {
            self.midi_editor.pan.gesture = None;
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
                || (button == MouseButton::Left && self.midi_editor.pan.enabled))
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
            start: self.midi_editor.piano_beat_start,
            span: self.midi_editor.piano_beat_span.raw(),
            pitch: self.midi_editor.piano_pitch,
        };
        if length == 0
            || viewport.span == 0
            || viewport.span > midi_scene::zoom_limit(length)
            || midi_scene::clamp_start(viewport.start, viewport.span) != viewport.start
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
        self.midi_editor.hover = None;
        self.midi_editor.pan.gesture = Some(Capture {
            origin: gesture,
            applied_pitch: viewport.pitch,
        });
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
                    if view.midi_editor.pan.gesture.as_ref().map(|c| c.origin) != Some(gesture) {
                        return;
                    }
                    let changed = if !view.audio_detail.clip_view_open
                        || view.device_panel.active
                        || !view.midi_editor.focus.is_focused(window)
                    {
                        view.midi_editor.pan.gesture.take().is_some()
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
                    if view.midi_editor.pan.gesture.as_ref().map(|c| c.origin) == Some(gesture) {
                        view.midi_editor.pan.gesture = None;
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
        if self.audio_detail.clip_view_open
            && !self.device_panel.active
            && self.midi_editor.focus.is_focused(window)
            && let Some(capture) = self.midi_editor.pan.gesture.as_ref()
            && self.midi_editor.piano_pitch == capture.applied_pitch
            && capture.origin.bounds == bounds
            && capture.origin.clip == clip
            && capture.origin.generation == self.project_generation
            && self
                .state
                .as_ref()
                .is_some_and(|s| s.revision == capture.origin.revision)
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
        let Some(capture) = self.midi_editor.pan.gesture.as_ref() else {
            return false;
        };
        let gesture = capture.origin;
        if self.midi_editor.piano_pitch != capture.applied_pitch
            || button != Some(gesture.button)
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
            self.midi_editor.pan.gesture = None;
            return true;
        }
        let Some(next) = gesture.resolve(pointer) else {
            self.midi_editor.pan.gesture = None;
            return true;
        };
        let changed = next.start != self.midi_editor.piano_beat_start
            || next.pitch != self.midi_editor.piano_pitch;
        self.midi_editor.piano_beat_start = next.start;
        self.midi_editor.piano_pitch = next.pitch;
        self.midi_editor.pan.gesture = Some(Capture {
            origin: gesture,
            applied_pitch: next.pitch,
        });
        changed
    }

    pub(super) fn finish_piano_pan(
        &mut self,
        bounds: CanvasBounds,
        pointer: [f32; 2],
        button: MouseButton,
    ) -> bool {
        if self
            .midi_editor
            .pan
            .gesture
            .as_ref()
            .is_none_or(|capture| capture.origin.button != button)
        {
            return false;
        }
        self.update_piano_pan(bounds, pointer, Some(button));
        self.midi_editor.pan.gesture = None;
        true
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;
    use hex_project::BeatTime;
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
                start: i128::from(2 * beat),
                span: 4 * beat,
                pitch: crate::editor::PitchViewport::DEFAULT,
            },
        );
        let expected = Viewport {
            start: i128::from(3 * beat),
            pitch: g.viewport.pitch.scroll_rows(6).0,
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
                    pitch: crate::editor::PitchViewport::from_rows(0, rows).unwrap(),
                },
            );
            let first = g.resolve([f32::MAX, f32::MAX]).unwrap();
            assert_eq!(first.start, -i128::from(u64::MAX));
            assert_eq!(first.pitch.low(), f64::from(128 - rows));
            let last = g.resolve([-f32::MAX, -f32::MAX]).unwrap();
            assert_eq!(last.start, i128::from(u64::MAX - span));
            assert_eq!(last.pitch.low(), 0.0);
            assert!(last.start.checked_add(i128::from(last.span)).is_some());
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "midi_pan_capture_tests.rs"]
mod capture_tests;
