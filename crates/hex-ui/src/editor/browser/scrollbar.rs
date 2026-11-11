//! Browser-only navigation of the current virtualized results. The UniformList owns
//! scroll offsets and virtualization; prepaint reads its latest layout metrics
//! after the list, so wheel, deferred navigation and resizing share one position.
use super::{EditorView, scrollbar_geometry::Geometry, theme};
use gpui::{
    Bounds, Context, CursorStyle, Div, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Window,
    canvas, div, fill, point, prelude::*, px, rgb,
};
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
pub(super) struct State {
    pub(super) geometry: Rc<Cell<Option<Geometry>>>,
    pub(super) drag: Option<Drag>,
    generation: u64,
    rows: usize,
}

impl State {
    pub(super) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.drag = None;
        self.geometry.set(None);
    }
    pub(super) fn begin_render(&self) {
        self.geometry.set(None);
    }
}

#[derive(Clone, Copy)]
pub(super) struct Drag {
    generation: u64,
    track: Bounds<Pixels>,
    grab: f32,
}

impl EditorView {
    pub(super) fn browser_results(
        &mut self,
        list: impl IntoElement,
        rows: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        if self.browser.scrollbar.rows != rows {
            self.browser.scrollbar.invalidate();
            self.browser.scrollbar.rows = rows;
        }
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .child(div().flex_1().min_w_0().h_full().child(list))
            .child(self.browser_scrollbar(cx))
    }

    pub(super) fn browser_scrollbar(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let cache = Rc::clone(&self.browser.scrollbar.geometry);
        let scroll = self.browser.scroll.clone();
        let generation = self.browser.scrollbar.generation;
        let rows = self.browser.scrollbar.rows;
        let drag = self.browser.scrollbar.drag;
        let owner = cx.entity_id();
        div()
            .id("browser-scrollbar")
            .debug_selector(|| "browser-scrollbar".into())
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
                        window.paint_quad(fill(
                            bounds,
                            rgb(theme::color(theme::PaletteRole::Surface)),
                        ));
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
                            window
                                .paint_quad(fill(geometry.thumb, rgb(color)).corner_radii(px(3.0)));
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
                cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    // Match other non-text browser controls without changing
                    // the selected row, query or audition.
                    view.browser.results_focus.focus(window);
                    window.prevent_default();
                    view.begin_browser_scroll(event.position, window, cx);
                }),
            )
    }

    fn begin_browser_scroll(
        &mut self,
        position: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(geometry) = self.browser.scrollbar.geometry.get().filter(|geometry| {
            geometry.generation == self.browser.scrollbar.generation
                && geometry.rows == self.browser.scrollbar.rows
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
                    view.move_browser_scroll(f32::from(position.y), cx);
                    if released.is_some() {
                        view.browser.scrollbar.drag = None;
                        cx.notify();
                    }
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    view.browser.scrollbar.drag = None;
                    cx.notify();
                });
            },
            cx,
        ) {
            return;
        }
        self.browser.scrollbar.drag = Some(Drag {
            generation: self.browser.scrollbar.generation,
            track: geometry.track,
            grab,
        });
        self.move_browser_scroll(pointer, cx);
    }

    fn move_browser_scroll(&mut self, pointer: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.browser.scrollbar.drag else {
            return;
        };
        let geometry = self.browser.scrollbar.geometry.get().filter(|geometry| {
            drag.generation == self.browser.scrollbar.generation
                && geometry.generation == self.browser.scrollbar.generation
                && geometry.rows == self.browser.scrollbar.rows
                && geometry.track == drag.track
        });
        let Some(geometry) = geometry else {
            self.browser.scrollbar.drag = None;
            cx.notify();
            return;
        };
        let Some(position) = geometry.position(pointer, drag.grab) else {
            return;
        };
        if let Ok(mut state) = self.browser.scroll.0.try_borrow_mut() {
            state.deferred_scroll_to_item = None;
            let current = state.base_handle.offset();
            state
                .base_handle
                .set_offset(point(current.x, px(-position)));
        }
        cx.notify();
    }
}
