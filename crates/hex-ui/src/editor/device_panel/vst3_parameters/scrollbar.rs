//! Pointer capture and geometry for the virtualized VST parameter list.
use super::{DeviceId, EditorView, Panel};
use crate::theme;
use gpui::{
    Bounds, Context, CursorStyle, Div, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Window,
    canvas, div, fill, point, prelude::*, px, rgb, size,
};
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
pub(super) struct State {
    geometry: Rc<Cell<Option<Geometry>>>,
    drag: Option<Drag>,
    generation: u64,
    rows: usize,
}
impl State {
    pub(super) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.drag = None;
        self.geometry.set(None);
    }
    fn begin_render(&mut self, rows: usize) {
        if self.rows != rows {
            self.invalidate();
            self.rows = rows;
        }
        self.geometry.set(None);
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn test_geometry(&self) -> Option<(Bounds<Pixels>, Bounds<Pixels>, f32)> {
        self.geometry
            .get()
            .map(|geometry| (geometry.track, geometry.thumb, geometry.maximum))
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn test_dragging(&self) -> bool {
        self.drag.is_some()
    }
}
#[derive(Clone, Copy)]
struct Drag {
    generation: u64,
    track: Bounds<Pixels>,
    grab: f32,
}
#[derive(Clone, Copy)]
struct Geometry {
    generation: u64,
    rows: usize,
    track: Bounds<Pixels>,
    thumb: Bounds<Pixels>,
    maximum: f32,
}
impl Geometry {
    fn new(
        bounds: Bounds<Pixels>,
        viewport: f32,
        maximum: f32,
        offset: f32,
        rows: usize,
        generation: u64,
    ) -> Option<Self> {
        let x = f32::from(bounds.left());
        let y = f32::from(bounds.top());
        let width = f32::from(bounds.size.width) - 4.0;
        let height = f32::from(bounds.size.height) - 4.0;
        if rows == 0
            || [x, y, width, height, viewport, maximum, offset]
                .iter()
                .any(|n| !n.is_finite())
            || width <= 0.0
            || height <= 0.0
            || viewport <= 0.0
            || maximum < 0.0
        {
            return None;
        }
        let total = viewport + maximum;
        if !total.is_finite() || !(x + 2.0 + width).is_finite() || !(y + 2.0 + height).is_finite() {
            return None;
        }
        let length = (height * (viewport / total)).clamp(20.0_f32.min(height), height);
        let ratio = if maximum > 0.0 {
            (-offset / maximum).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let track = Bounds::new(point(px(x + 2.0), px(y + 2.0)), size(px(width), px(height)));
        let thumb = Bounds::new(
            point(track.left(), track.top() + px((height - length) * ratio)),
            size(track.size.width, px(length)),
        );
        Some(Self {
            generation,
            rows,
            track,
            thumb,
            maximum,
        })
    }
    fn position(self, pointer: f32, grab: f32) -> Option<f32> {
        let travel = f32::from(self.track.size.height - self.thumb.size.height);
        if !pointer.is_finite() || !grab.is_finite() || self.maximum <= 0.0 || travel <= 0.0 {
            return None;
        }
        let top = pointer
            - f32::from(self.track.top())
            - grab.clamp(0.0, 1.0) * f32::from(self.thumb.size.height);
        Some((top / travel).clamp(0.0, 1.0) * self.maximum)
    }
}

pub(super) fn element(
    panel: &mut Panel,
    device: DeviceId,
    cx: &mut Context<EditorView>,
) -> gpui::Stateful<Div> {
    let rows = panel.matched.len();
    panel.scrollbar.begin_render(rows);
    let cache = Rc::clone(&panel.scrollbar.geometry);
    let scroll = panel.scroll.clone();
    let generation = panel.scrollbar.generation;
    let drag = panel.scrollbar.drag;
    let owner = cx.entity_id();
    div()
        .id(("vst3-parameter-scrollbar", device.value()))
        .debug_selector(|| "vst3-parameter-scrollbar".into())
        .w(px(12.0))
        .h_full()
        .flex_shrink_0()
        .child(
            canvas(
                move |bounds, _, _| {
                    let geometry = scroll.0.try_borrow().ok().and_then(|state| {
                        Geometry::new(
                            bounds,
                            f32::from(state.base_handle.bounds().size.height),
                            f32::from(state.base_handle.max_offset().height),
                            f32::from(state.base_handle.offset().y),
                            rows,
                            generation,
                        )
                    });
                    cache.set(geometry);
                    geometry
                },
                move |bounds, geometry, window, _| {
                    window.paint_quad(fill(bounds, rgb(theme::color(theme::PaletteRole::Surface))));
                    if let Some(geometry) = geometry.filter(|geometry| geometry.maximum > 0.0) {
                        let active = drag.is_some_and(|drag| {
                            drag.generation == generation && drag.track == geometry.track
                        });
                        let preferred = theme::color(if active {
                            theme::PaletteRole::Primary
                        } else {
                            theme::PaletteRole::Muted
                        });
                        let color = theme::readable_text(
                            theme::color(theme::PaletteRole::Surface),
                            preferred,
                        );
                        window.paint_quad(fill(geometry.thumb, rgb(color)).corner_radii(px(3.0)));
                        if active {
                            window.renew_pointer_capture(owner, CursorStyle::Arrow);
                        }
                    }
                },
            )
            .size_full(),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                cx.stop_propagation();
                window.prevent_default();
                begin(view, device, event.position, window, cx);
            }),
        )
}
fn begin(
    view: &mut EditorView,
    device: DeviceId,
    position: gpui::Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    let Some(panel) = view.device_panel.vst3_parameters.panels.get(&device) else {
        return;
    };
    let Some(geometry) = panel.scrollbar.geometry.get().filter(|geometry| {
        geometry.generation == panel.scrollbar.generation
            && geometry.rows == panel.scrollbar.rows
            && geometry.maximum > 0.0
    }) else {
        return;
    };
    let pointer = f32::from(position.y);
    if !pointer.is_finite() || !f32::from(position.x).is_finite() {
        return;
    }
    let grab = if geometry.thumb.contains(&position) {
        ((pointer - f32::from(geometry.thumb.top())) / f32::from(geometry.thumb.size.height))
            .clamp(0.0, 1.0)
    } else {
        0.5
    };
    let owner = cx.entity().downgrade();
    let cancelled = owner.clone();
    if !window.capture_pointer(
        cx.entity_id(),
        MouseButton::Left,
        CursorStyle::Arrow,
        move |event, _, cx| {
            let moved = event.downcast_ref::<MouseMoveEvent>();
            let released = event
                .downcast_ref::<MouseUpEvent>()
                .filter(|event| event.button == MouseButton::Left);
            let Some(position) = moved
                .map(|event| event.position)
                .or_else(|| released.map(|event| event.position))
            else {
                return;
            };
            let _ = owner.update(cx, |view, cx| {
                move_scroll(view, device, f32::from(position.y), cx);
                if released.is_some() {
                    if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
                        panel.scrollbar.drag = None;
                    }
                    cx.notify();
                }
            });
        },
        move |_, cx| {
            let _ = cancelled.update(cx, |view, cx| {
                if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
                    panel.scrollbar.drag = None;
                }
                cx.notify();
            });
        },
        cx,
    ) {
        return;
    }
    if let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) {
        panel.scrollbar.drag = Some(Drag {
            generation: panel.scrollbar.generation,
            track: geometry.track,
            grab,
        });
    }
    move_scroll(view, device, pointer, cx);
}
fn move_scroll(
    view: &mut EditorView,
    device: DeviceId,
    pointer: f32,
    cx: &mut Context<EditorView>,
) {
    let Some(panel) = view.device_panel.vst3_parameters.panels.get_mut(&device) else {
        return;
    };
    let Some(drag) = panel.scrollbar.drag else {
        return;
    };
    let geometry = panel.scrollbar.geometry.get().filter(|geometry| {
        drag.generation == panel.scrollbar.generation
            && geometry.generation == panel.scrollbar.generation
            && geometry.rows == panel.scrollbar.rows
            && geometry.track == drag.track
    });
    let Some(geometry) = geometry else {
        panel.scrollbar.drag = None;
        cx.notify();
        return;
    };
    let Some(position) = geometry.position(pointer, drag.grab) else {
        return;
    };
    if let Ok(mut state) = panel.scroll.0.try_borrow_mut() {
        state.deferred_scroll_to_item = None;
        let current = state.base_handle.offset();
        state
            .base_handle
            .set_offset(point(current.x, px(-position)));
    }
    cx.notify();
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;

    #[test]
    fn thumb_clamps_extents_and_rejects_invalid_geometry() {
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(12.0), px(204.0)));
        assert!(Geometry::new(bounds, 200.0, f32::NAN, 0.0, 10, 1).is_none());
        assert!(Geometry::new(bounds, 200.0, 0.0, 0.0, 0, 1).is_none());
        let geometry = Geometry::new(bounds, 200.0, 2_000_000.0, -2_000_000.0, 100_000, 1).unwrap();
        assert_eq!(geometry.thumb.bottom(), geometry.track.bottom());
        assert_eq!(geometry.position(-1000.0, 0.5), Some(0.0));
        assert_eq!(geometry.position(1000.0, 0.5), Some(geometry.maximum));
        assert!(geometry.position(f32::NAN, 0.5).is_none());
    }
}
