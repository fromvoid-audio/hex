//! Compact Arrangement loop-range rail with a control-side preview gesture.
use super::{EditorMode, EditorView, ViewState, musical_time::GridLine, viewport::Viewport};
use crate::theme;
use gpui::{
    Bounds, Context, CursorStyle, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    canvas, div, prelude::*, px, relative, rgb,
};
use hex_project::{Command, LoopRegion};
use hex_session::service::Request;

const RAIL_HEIGHT: f32 = 18.0;
const BAR_INSET: f32 = 4.0;
const EDGE_HIT_PIXELS: f32 = 6.0;

#[derive(Clone, Copy, Debug)]
enum EditMode {
    Paint { anchor: u64 },
    ResizeStart { fixed_end: u64, enabled: bool },
    ResizeEnd { fixed_start: u64, enabled: bool },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LoopRailGesture {
    generation: u64,
    revision: u64,
    loop_region: Option<LoopRegion>,
    viewport: Viewport,
    timeline_left: f32,
    timeline_width: f32,
    mode: EditMode,
    preview: Option<LoopRegion>,
}

#[derive(Clone, Copy, Debug)]
enum Hover {
    Paint,
    Activate,
    Resize,
}

impl EditorView {
    fn begin_loop_rail_gesture(
        &mut self,
        bounds: Bounds<gpui::Pixels>,
        x: f32,
        free: bool,
        viewport: Viewport,
        region: Option<LoopRegion>,
    ) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        if self.mode != EditorMode::Arrangement
            || self.edit_pending()
            || self.context_menu.is_some()
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.arrangement_pan.is_some()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
            || !x.is_finite()
        {
            return false;
        }
        let timeline_left = f32::from(bounds.origin.x);
        let timeline_width = f32::from(bounds.size.width);
        let Some(frame) = super::seek::pointer_to_frame(x, timeline_left, timeline_width, viewport)
        else {
            return false;
        };

        if let Some(region) = region
            && let Some((start_x, end_x)) =
                visible_region_pixels(region, viewport, timeline_left, timeline_width)
        {
            let start_visible = region.start() >= viewport.start;
            let end_visible = viewport.end().is_some_and(|end| region.end() <= end);
            let start_distance = if start_visible {
                (x - start_x).abs()
            } else {
                f32::INFINITY
            };
            let end_distance = if end_visible {
                (x - end_x).abs()
            } else {
                f32::INFINITY
            };
            let mode = if start_distance <= EDGE_HIT_PIXELS && start_distance <= end_distance {
                Some(EditMode::ResizeStart {
                    fixed_end: region.end(),
                    enabled: region.enabled(),
                })
            } else if end_distance <= EDGE_HIT_PIXELS {
                Some(EditMode::ResizeEnd {
                    fixed_start: region.start(),
                    enabled: region.enabled(),
                })
            } else if frame >= region.start() && frame < region.end() {
                if !region.enabled() {
                    return self.enqueue(Request::Edit(Command::SetLoopRegion {
                        start: region.start(),
                        end: region.end(),
                        enabled: true,
                    }));
                }
                return true;
            } else {
                None
            };
            if let Some(mode) = mode {
                self.loop_rail_gesture = Some(LoopRailGesture {
                    generation: self.project_generation,
                    revision: state.revision,
                    loop_region: Some(region),
                    viewport,
                    timeline_left,
                    timeline_width,
                    mode,
                    preview: Some(region),
                });
                return true;
            }
        }

        let Some(anchor) = snap_frame(self, &state.project, frame, viewport, timeline_width, free)
        else {
            return false;
        };
        self.loop_rail_gesture = Some(LoopRailGesture {
            generation: self.project_generation,
            revision: state.revision,
            loop_region: region,
            viewport,
            timeline_left,
            timeline_width,
            mode: EditMode::Paint { anchor },
            preview: None,
        });
        true
    }

    fn update_loop_rail_gesture(&mut self, x: f32, free: bool) -> bool {
        let Some(mut gesture) = self.loop_rail_gesture.take() else {
            return false;
        };
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        if self.mode != EditorMode::Arrangement
            || gesture.generation != self.project_generation
            || gesture.revision != state.revision
            || gesture.loop_region != state.project.loop_region()
            || self.context_menu.is_some()
            || self.opening_project
            || self.close_prompt_open
            || !x.is_finite()
        {
            return true;
        }
        let Some(frame) = super::seek::pointer_to_frame(
            x,
            gesture.timeline_left,
            gesture.timeline_width,
            gesture.viewport,
        ) else {
            self.loop_rail_gesture = Some(gesture);
            return true;
        };
        let Some(frame) = snap_frame(
            self,
            &state.project,
            frame,
            gesture.viewport,
            gesture.timeline_width,
            free,
        ) else {
            self.loop_rail_gesture = Some(gesture);
            return true;
        };
        gesture.preview = match gesture.mode {
            EditMode::Paint { anchor } => {
                let start = anchor.min(frame);
                let end = anchor.max(frame);
                LoopRegion::new(start, end, false).ok()
            }
            EditMode::ResizeStart { fixed_end, enabled } => {
                let candidate = frame.min(fixed_end.saturating_sub(1));
                LoopRegion::new(candidate, fixed_end, enabled).ok()
            }
            EditMode::ResizeEnd {
                fixed_start,
                enabled,
            } => {
                let candidate = frame.max(fixed_start.saturating_add(1));
                LoopRegion::new(fixed_start, candidate, enabled).ok()
            }
        };
        self.loop_rail_gesture = Some(gesture);
        true
    }

    fn finish_loop_rail_gesture(&mut self, x: f32, free: bool) -> bool {
        if self.loop_rail_gesture.is_none() {
            return false;
        }
        self.update_loop_rail_gesture(x, free);
        let Some(gesture) = self.loop_rail_gesture.take() else {
            return true;
        };
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        if gesture.generation != self.project_generation
            || gesture.revision != state.revision
            || gesture.loop_region != state.project.loop_region()
            || self.mode != EditorMode::Arrangement
        {
            return true;
        }
        let Some(region) = gesture.preview else {
            self.error = Some("loop region needs a non-empty time range".into());
            return true;
        };
        let _ = self.enqueue(Request::Edit(Command::SetLoopRegion {
            start: region.start(),
            end: region.end(),
            enabled: region.enabled(),
        }));
        true
    }

    fn cancel_loop_rail_gesture(&mut self) -> bool {
        self.loop_rail_gesture.take().is_some()
    }

    fn loop_rail_hover(&self, x: f32, viewport: Viewport, bounds: Bounds<gpui::Pixels>) -> Hover {
        let Some(region) = self
            .state
            .as_ref()
            .and_then(|state| state.project.loop_region())
        else {
            return Hover::Paint;
        };
        let left = f32::from(bounds.origin.x);
        let width = f32::from(bounds.size.width);
        let Some((start_x, end_x)) = visible_region_pixels(region, viewport, left, width) else {
            return Hover::Paint;
        };
        let start_edge = region.start() >= viewport.start && (x - start_x).abs() <= EDGE_HIT_PIXELS;
        let end_edge = viewport.end().is_some_and(|end| region.end() <= end)
            && (x - end_x).abs() <= EDGE_HIT_PIXELS;
        if start_edge || end_edge {
            Hover::Resize
        } else if super::seek::pointer_to_frame(x, left, width, viewport)
            .is_some_and(|frame| frame >= region.start() && frame < region.end())
        {
            Hover::Activate
        } else {
            Hover::Paint
        }
    }
}

pub(super) fn view(
    view: &EditorView,
    state: &ViewState,
    viewport: Viewport,
    grid: &[GridLine],
    cx: &mut Context<EditorView>,
) -> Div {
    let current_region = state.project.loop_region();
    let gesture = view.loop_rail_gesture.filter(|gesture| {
        gesture.generation == view.project_generation
            && gesture.revision == state.revision
            && gesture.loop_region == state.project.loop_region()
            && view.mode == EditorMode::Arrangement
    });
    let shown_region = gesture
        .and_then(|gesture| gesture.preview)
        .or(current_region);
    let entity = cx.entity();
    let rail = div()
        .h(px(RAIL_HEIGHT))
        .flex_shrink_0()
        .flex()
        .bg(rgb(theme::color(theme::PaletteRole::Background)))
        .border_b_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
        .child(
            div()
                .id("arrangement-loop-rail")
                .debug_selector(|| "arrangement-loop-rail".to_owned())
                .relative()
                .h_full()
                .flex_1()
                .min_w_0()
                .cursor(view.loop_rail_cursor)
                .tooltip(|_, cx| {
                    cx.new(|_| super::transport::MiniTooltip {
                        description: "drag to set loop · click range to enable · l toggles · cmd for free placement".into(),
                    })
                    .into()
                })
                .child(loop_grid_layer(grid, viewport))
                .children(
                    shown_region
                        .and_then(|region| loop_region_bar(region, viewport)),
                )
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            let down_entity = entity.clone();
                            let current_region = current_region;
                            window.on_mouse_event(
                                move |event: &MouseDownEvent, phase, window, cx| {
                                    if !phase.capture()
                                        || event.button != MouseButton::Left
                                        || !bounds.contains(&event.position)
                                    {
                                        return;
                                    }
                                    let started = down_entity.update(cx, |view, _| {
                                        view.begin_loop_rail_gesture(
                                            bounds,
                                            f32::from(event.position.x),
                                            event.modifiers.platform,
                                            viewport,
                                            current_region,
                                        )
                                    });
                                    if started {
                                        cx.stop_propagation();
                                        window.refresh();
                                    }
                                },
                            );
                            let move_entity = entity.clone();
                            let move_bounds = bounds;
                            window.on_mouse_event(
                                move |event: &MouseMoveEvent, phase, window, cx| {
                                    if !phase.capture() {
                                        return;
                                    }
                                    if event.pressed_button != Some(MouseButton::Left) {
                                        let hover = move_entity.update(cx, |view, _| {
                                            let hover = if move_bounds.contains(&event.position) {
                                                match view.loop_rail_hover(
                                                    f32::from(event.position.x),
                                                    viewport,
                                                    move_bounds,
                                                ) {
                                                    Hover::Paint => CursorStyle::Crosshair,
                                                    Hover::Activate => CursorStyle::PointingHand,
                                                    Hover::Resize => CursorStyle::ResizeLeftRight,
                                                }
                                            } else {
                                                CursorStyle::Crosshair
                                            };
                                            if view.loop_rail_cursor == hover {
                                                return false;
                                            }
                                            view.loop_rail_cursor = hover;
                                            true
                                        });
                                        if hover {
                                            window.refresh();
                                        }
                                        return;
                                    }
                                    let changed = move_entity.update(cx, |view, _| {
                                        view.update_loop_rail_gesture(
                                            f32::from(event.position.x),
                                            event.modifiers.platform,
                                        )
                                    });
                                    if changed {
                                        cx.stop_propagation();
                                        window.refresh();
                                    }
                                },
                            );
                            let up_entity = entity.clone();
                            window.on_mouse_event(
                                move |event: &MouseUpEvent, phase, window, cx| {
                                    if !phase.capture() || event.button != MouseButton::Left {
                                        return;
                                    }
                                    let finished = up_entity.update(cx, |view, _| {
                                        view.finish_loop_rail_gesture(
                                            f32::from(event.position.x),
                                            event.modifiers.platform,
                                        )
                                    });
                                    if finished {
                                        cx.stop_propagation();
                                        window.refresh();
                                    }
                                },
                            );
                        },
                    )
                    .absolute()
                    .inset_0(),
                ),
        )
        .child(div().w(px(super::tracks::TRACK_HEADER_WIDTH)).h_full());
    #[cfg(feature = "ui-test-support")]
    let rail = rail.debug_selector(|| "arrangement-loop-rail-row".into());
    rail
}

fn snap_frame(
    view: &EditorView,
    project: &hex_project::Project,
    frame: u64,
    viewport: Viewport,
    width: f32,
    free: bool,
) -> Option<u64> {
    let snap = if free {
        super::musical_time::SnapGrid::Off
    } else {
        view.snap_grid
    };
    snap.snap_frame(project, frame, viewport, width)
}

fn visible_region_pixels(
    region: LoopRegion,
    viewport: Viewport,
    left: f32,
    width: f32,
) -> Option<(f32, f32)> {
    let viewport_end = viewport.end()?;
    if viewport.length == 0 || width <= 0.0 {
        return None;
    }
    let start = region.start().max(viewport.start);
    let end = region.end().min(viewport_end);
    if start >= end {
        return None;
    }
    let start_x = left + ((start - viewport.start) as f64 / viewport.length as f64) as f32 * width;
    let end_x = left + ((end - viewport.start) as f64 / viewport.length as f64) as f32 * width;
    Some((start_x, end_x))
}

fn loop_grid_layer(lines: &[GridLine], viewport: Viewport) -> Div {
    let lines = if viewport.length == 0 {
        Vec::new()
    } else {
        lines
            .iter()
            .filter_map(|line| {
                let offset = line.frame.checked_sub(viewport.start)?;
                if offset > viewport.length {
                    return None;
                }
                Some(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative((offset as f64 / viewport.length as f64) as f32))
                        .w(px(if line.meter_marker { 1.5 } else { 1.0 }))
                        .bg(rgb(theme::color(if line.meter_marker {
                            theme::PaletteRole::Primary
                        } else {
                            theme::PaletteRole::Grid
                        })))
                        .opacity(0.48),
                )
            })
            .collect()
    };
    div().absolute().inset_0().children(lines)
}

fn loop_region_bar(region: LoopRegion, viewport: Viewport) -> Option<Div> {
    let viewport_end = viewport.end()?;
    if viewport.length == 0 {
        return None;
    }
    let start = region.start().max(viewport.start);
    let end = region.end().min(viewport_end);
    if start >= end {
        return None;
    }
    let left = (start - viewport.start) as f64 / viewport.length as f64;
    let width = (end - start) as f64 / viewport.length as f64;
    Some(
        div()
            .debug_selector(|| "loop-rail-range".into())
            .absolute()
            .top(px(BAR_INSET))
            .bottom(px(BAR_INSET))
            .left(relative(left as f32))
            .w(relative(width as f32))
            .bg(rgb(theme::color(if region.enabled() {
                theme::PaletteRole::Primary
            } else {
                theme::PaletteRole::Muted
            })))
            .opacity(if region.enabled() { 0.94 } else { 0.62 }),
    )
}

pub(super) fn cancel_stale(
    view: &mut EditorView,
    generation: u64,
    revision: u64,
    loop_region: Option<LoopRegion>,
) -> bool {
    if view.loop_rail_gesture.is_some_and(|gesture| {
        gesture.generation != generation
            || gesture.revision != revision
            || gesture.loop_region != loop_region
    }) {
        return view.cancel_loop_rail_gesture();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::visible_region_pixels;
    use crate::editor::viewport::Viewport;
    use hex_project::LoopRegion;

    #[test]
    fn visible_range_clips_loop_edges_to_zoomed_viewport() {
        let viewport = Viewport {
            start: 100,
            length: 100,
        };
        let region = LoopRegion::new(50, 150, true).expect("valid loop");
        assert_eq!(
            visible_region_pixels(region, viewport, 10.0, 200.0),
            Some((10.0, 110.0))
        );
    }
}
