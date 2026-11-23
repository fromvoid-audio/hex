//! Shared vertical scrollbar state, canvas painting, and pointer-capture drag driver
//! for virtualized `UniformList` views (`log_viewer`, `editor::browser`, `vst3_parameters`).
use crate::theme;
use gpui::{
    Context, CursorStyle, Div, ElementId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, UniformListScrollHandle, Window, canvas, div, fill, point,
    prelude::*, px, rgb,
};
use std::{cell::Cell, rc::Rc};

pub(crate) mod geometry;
pub(crate) use geometry::{Drag, Geometry};

/// Per-list scrollbar layout cache, active drag anchor, and optional local generation/row tracking.
#[derive(Default)]
pub(crate) struct State {
    pub(crate) geometry: Rc<Cell<Option<Geometry>>>,
    pub(crate) drag: Option<Drag>,
    generation: u64,
    rows: usize,
}

impl State {
    pub(crate) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.drag = None;
        self.geometry.set(None);
    }

    pub(crate) fn begin_render(&self) {
        self.geometry.set(None);
    }

    pub(crate) fn sync_rows(&mut self, rows: usize) {
        if self.rows != rows {
            self.invalidate();
            self.rows = rows;
        }
    }

    pub(crate) fn begin_render_rows(&mut self, rows: usize) {
        self.sync_rows(rows);
        self.geometry.set(None);
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn rows(&self) -> usize {
        self.rows
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(crate) fn test_geometry(
        &self,
    ) -> Option<(gpui::Bounds<Pixels>, gpui::Bounds<Pixels>, f32)> {
        self.geometry
            .get()
            .map(|geometry| (geometry.track, geometry.thumb, geometry.maximum))
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(crate) fn test_dragging(&self) -> bool {
        self.drag.is_some()
    }
}

/// Visual and identity parameters for one virtualized vertical scrollbar.
pub(crate) struct Config {
    pub(crate) id: ElementId,
    pub(crate) debug_selector: &'static str,
    pub(crate) generation: u64,
    pub(crate) rows: usize,
    pub(crate) paint_unscrollable_thumb: bool,
}

/// Build the 12px vertical scrollbar element that samples `scroll` during prepaint and paints
/// the track and thumb with active pointer-capture renewal.
pub(crate) fn element<V: 'static>(
    state: &State,
    scroll: UniformListScrollHandle,
    config: Config,
    cx: &mut Context<V>,
    on_mouse_down: impl Fn(&mut V, &MouseDownEvent, &mut Window, &mut Context<V>) + 'static,
) -> gpui::Stateful<Div> {
    let cache = Rc::clone(&state.geometry);
    let drag = state.drag;
    let owner = cx.entity_id();
    let Config {
        id,
        debug_selector,
        generation,
        rows,
        paint_unscrollable_thumb,
    } = config;
    div()
        .id(id)
        .debug_selector(move || debug_selector.into())
        .w(px(12.0))
        .h_full()
        .flex_shrink_0()
        .child(
            canvas(
                move |bounds, _, _| {
                    let geometry = scroll.0.try_borrow().ok().and_then(|scroll_state| {
                        Geometry::new(
                            bounds,
                            f32::from(scroll_state.base_handle.bounds().size.height),
                            f32::from(scroll_state.base_handle.max_offset().height),
                            f32::from(scroll_state.base_handle.offset().y),
                            rows,
                            generation,
                        )
                    });
                    cache.set(geometry);
                    geometry
                },
                move |bounds, geometry, window, _| {
                    window.paint_quad(fill(bounds, rgb(theme::color(theme::PaletteRole::Surface))));
                    let visible = geometry.filter(|g| paint_unscrollable_thumb || g.maximum > 0.0);
                    if let Some(geometry) = visible {
                        let active =
                            drag.is_some_and(|drag| drag.is_active_for(generation, geometry.track));
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
        .on_mouse_down(MouseButton::Left, cx.listener(on_mouse_down))
}

/// Borrowed scrollbar fields resolved from the owning view for drag start/move/cancel.
pub(crate) struct Target<'a> {
    pub(crate) state: &'a mut State,
    pub(crate) scroll: &'a UniformListScrollHandle,
    pub(crate) generation: u64,
    pub(crate) rows: usize,
}

/// Start pointer-captured scrollbar dragging for the target returned by `select`.
pub(crate) fn begin_drag<V: 'static>(
    view: &mut V,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<V>,
    select: impl Fn(&mut V) -> Option<Target<'_>> + Clone + 'static,
) {
    let drag = {
        let Some(target) = select(view) else {
            return;
        };
        let Some(drag) = Drag::begin(
            target.state.geometry.get(),
            position,
            target.generation,
            target.rows,
        ) else {
            return;
        };
        drag
    };
    let pointer = f32::from(position.y);
    let owner = cx.entity().downgrade();
    let cancelled = owner.clone();
    let select_move = select.clone();
    let select_cancel = select.clone();
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
            let select_move = select_move.clone();
            let _ = owner.update(cx, |view, cx| {
                move_drag(view, f32::from(position.y), cx, &select_move);
                if released.is_some() {
                    if let Some(target) = select_move(view) {
                        target.state.drag = None;
                    }
                    cx.notify();
                }
            });
        },
        move |_, cx| {
            let select_cancel = select_cancel.clone();
            let _ = cancelled.update(cx, |view, cx| {
                if let Some(target) = select_cancel(view) {
                    target.state.drag = None;
                }
                cx.notify();
            });
        },
        cx,
    ) {
        return;
    }
    if let Some(target) = select(view) {
        target.state.drag = Some(drag);
    }
    move_drag(view, pointer, cx, &select);
}

fn move_drag<V: 'static>(
    view: &mut V,
    pointer: f32,
    cx: &mut Context<V>,
    select: &impl Fn(&mut V) -> Option<Target<'_>>,
) {
    let Some(target) = select(view) else {
        return;
    };
    let Some(drag) = target.state.drag else {
        return;
    };
    let position = match drag.resolve_move(
        target.state.geometry.get(),
        pointer,
        target.generation,
        target.rows,
    ) {
        Ok(Some(position)) => position,
        Ok(None) => return,
        Err(()) => {
            target.state.drag = None;
            cx.notify();
            return;
        }
    };
    if let Ok(mut scroll_state) = target.scroll.0.try_borrow_mut() {
        scroll_state.deferred_scroll_to_item = None;
        let current = scroll_state.base_handle.offset();
        scroll_state
            .base_handle
            .set_offset(point(current.x, px(-position)));
    }
    cx.notify();
}
