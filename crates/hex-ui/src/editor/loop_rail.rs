//! Compact Arrangement loop-range rail with a control-side preview gesture.
use super::{
    EditorMode, EditorView, ViewState,
    musical_time::{GridLine, SnapGrid},
    viewport::Viewport,
};
use crate::theme;
use gpui::{
    Bounds, Context, CursorStyle, Div, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, canvas, div, prelude::*, px, relative, rgb,
};
use hex_project::LoopRegion;

mod geometry;
mod pending;

pub(super) use geometry::binary_duration;
use geometry::{EditMode, region_edit_mode, stepped_start};

#[derive(Clone, Copy, Debug)]
pub(super) struct PendingLoop {
    generation: u64,
    region: LoopRegion,
    toggle: bool,
    remaining: Option<usize>,
    // Selection intent for automatic focus after a successful section-loop ACK.
    focus_intent: Option<u64>,
    start_intent: Option<u64>,
    launch: Option<super::section_launch::CapturedSectionLaunch>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProjectionInput {
    frame: u64,
    grid: SnapGrid,
    unsnapped: bool,
    stepped: bool,
    moved: bool,
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
    pointer_down: gpui::Point<gpui::Pixels>,
    last_pointer: gpui::Point<gpui::Pixels>,
    last_projection: Option<ProjectionInput>,
    moved: bool,
    click_disqualified: bool,
    toggle_on_click: bool,
    select_extent_on_click: bool,
}

#[derive(Clone, Copy, Debug)]
enum Hover {
    Paint,
    Move,
    Resize,
}

impl EditorView {
    fn begin_loop_rail_gesture(
        &mut self,
        bounds: Bounds<gpui::Pixels>,
        position: gpui::Point<gpui::Pixels>,
        modifiers: Modifiers,
        viewport: Viewport,
        region: Option<LoopRegion>,
    ) -> bool {
        let x = f32::from(position.x);
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        if self.mode != EditorMode::Arrangement
            || self.edit_pending()
            || self.context_menu.is_some()
            || super::device_panel::picker_is_open(self)
            || self.settings.theme.open
            || self.settings.tuning.open
            || self.persistence.opening_project
            || self.persistence.close_prompt_open
            || self.persistence.save_dialog_open
            || self.persistence.export_dialog_open
            || self.active_clip_drag.is_some()
            || self.browser.active_audio_drag.is_some()
            || self.device_panel.active_native_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.midi_editor.active_trim_drag.is_some()
            || self.arrangement_pan.is_some()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
            || self.locked_play_start_drag_active()
            || modifiers.alt
            || modifiers.control
            || !x.is_finite()
            || !f32::from(position.y).is_finite()
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
            && let Some(mode) = region_edit_mode(region, viewport, bounds, x)
        {
            self.loop_rail_gesture = Some(LoopRailGesture {
                generation: self.project_generation,
                revision: state.revision,
                loop_region: Some(region),
                viewport,
                timeline_left,
                timeline_width,
                mode,
                preview: Some(region),
                pointer_down: position,
                last_pointer: position,
                last_projection: None,
                moved: false,
                click_disqualified: false,
                toggle_on_click: modifiers.platform,
                select_extent_on_click: super::time_selection::is_extent_selection_click(modifiers),
            });
            return true;
        }

        let Some(anchor) = snap_frame(
            self,
            &state.project,
            frame,
            viewport,
            timeline_width,
            modifiers.platform,
        ) else {
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
            pointer_down: position,
            last_pointer: position,
            last_projection: None,
            moved: false,
            click_disqualified: false,
            toggle_on_click: false,
            select_extent_on_click: false,
        });
        true
    }

    fn update_loop_rail_gesture(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        modifiers: Modifiers,
    ) -> bool {
        let x = f32::from(position.x);
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
            || gesture.viewport != self.visible_viewport(state.timeline_frames)
            || modifiers.alt
            || modifiers.control
            || self.context_menu.is_some()
            || super::device_panel::picker_is_open(self)
            || self.persistence.opening_project
            || self.persistence.close_prompt_open
            || !x.is_finite()
            || !f32::from(position.y).is_finite()
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
            return false;
        };
        gesture.last_pointer = position;
        gesture.moved |= (x - f32::from(gesture.pointer_down.x)).abs() > 3.0;
        // Vertical travel disqualifies a click without qualifying a horizontal move.
        gesture.click_disqualified |= gesture.moved
            || (f32::from(position.y) - f32::from(gesture.pointer_down.y)).abs() > 3.0;
        let input = ProjectionInput {
            frame,
            grid: self.snap_grid,
            unsnapped: modifiers.platform,
            stepped: modifiers.shift,
            moved: gesture.moved,
        };
        if gesture.last_projection == Some(input) {
            self.loop_rail_gesture = Some(gesture);
            return false;
        }
        gesture.last_projection = Some(input);
        let previous = gesture.preview;
        if let EditMode::Move { original, anchor } = gesture.mode {
            if !gesture.moved {
                self.loop_rail_gesture = Some(gesture);
                return false;
            }
            let delta = i128::from(frame) - i128::from(anchor);
            let start = if modifiers.shift {
                stepped_start(original, delta)
            } else {
                let candidate = (i128::from(original.start()) + delta).clamp(
                    0,
                    i128::from(u64::MAX - (original.end() - original.start())),
                ) as u64;
                snap_frame(
                    self,
                    &state.project,
                    candidate,
                    gesture.viewport,
                    gesture.timeline_width,
                    modifiers.platform,
                )
                .map(|start| start.min(u64::MAX - (original.end() - original.start())))
            };
            gesture.preview = start.and_then(|start| {
                LoopRegion::new(
                    start,
                    start.checked_add(original.end() - original.start())?,
                    original.enabled(),
                )
                .ok()
            });
            let changed = gesture.preview != previous;
            self.loop_rail_gesture = Some(gesture);
            return changed;
        }
        let Some(frame) = snap_frame(
            self,
            &state.project,
            frame,
            gesture.viewport,
            gesture.timeline_width,
            modifiers.platform,
        ) else {
            self.loop_rail_gesture = Some(gesture);
            return false;
        };
        gesture.preview = match gesture.mode {
            EditMode::Move { .. } => gesture.preview,
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
        let changed = gesture.preview != previous;
        self.loop_rail_gesture = Some(gesture);
        changed
    }

    fn finish_loop_rail_gesture(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        modifiers: Modifiers,
    ) -> bool {
        if self.loop_rail_gesture.is_none() {
            return false;
        }
        self.update_loop_rail_gesture(position, modifiers);
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
        if !gesture.click_disqualified && gesture.select_extent_on_click {
            if let Some(region) = gesture.loop_region {
                self.select_project_time_range(
                    region.start(),
                    region.end(),
                    gesture.generation,
                    gesture.revision,
                );
            }
            return true;
        }
        if !gesture.click_disqualified && gesture.toggle_on_click && modifiers.platform {
            self.toggle_loop();
            return true;
        }
        if matches!(gesture.mode, EditMode::Move { .. }) && !gesture.moved {
            return true;
        }
        let Some(region) = gesture.preview else {
            self.error = Some("scope needs a non-empty time range".into());
            return true;
        };
        if Some(region) == gesture.loop_region {
            return true;
        }
        self.submit_loop_preview(gesture.generation, region);
        true
    }

    pub(super) fn cancel_loop_rail_gesture(&mut self) -> bool {
        self.loop_rail_gesture.take().is_some()
    }

    pub(super) fn refresh_loop_rail_modifiers(
        &mut self,
        modifiers: Modifiers,
        window: &gpui::Window,
    ) -> bool {
        if !self.focus.is_focused(window) {
            return self.cancel_loop_rail_gesture();
        }
        self.loop_rail_gesture
            .is_some_and(|gesture| self.update_loop_rail_gesture(gesture.last_pointer, modifiers))
    }

    fn loop_rail_hover(&self, x: f32, viewport: Viewport, bounds: Bounds<gpui::Pixels>) -> Hover {
        let Some(region) = self
            .state
            .as_ref()
            .and_then(|state| state.project.loop_region())
        else {
            return Hover::Paint;
        };
        match region_edit_mode(region, viewport, bounds, x) {
            Some(EditMode::Move { .. }) => Hover::Move,
            Some(EditMode::ResizeStart { .. } | EditMode::ResizeEnd { .. }) => Hover::Resize,
            Some(EditMode::Paint { .. }) | None => Hover::Paint,
        }
    }
}

pub(super) fn displayed_region(view: &EditorView, state: &ViewState) -> Option<LoopRegion> {
    let current_region = state.project.loop_region();
    let gesture = view.loop_rail_gesture.filter(|gesture| {
        gesture.generation == view.project_generation
            && gesture.revision == state.revision
            && gesture.loop_region == state.project.loop_region()
            && view.mode == EditorMode::Arrangement
    });
    gesture
        .and_then(|gesture| gesture.preview)
        .or_else(|| view.pending_loop_region())
        .or(current_region)
}

pub(super) fn view(
    view: &EditorView,
    state: &ViewState,
    viewport: Viewport,
    grid: &[GridLine],
    cx: &mut Context<EditorView>,
) -> Div {
    let current_region = state.project.loop_region();
    let shown_region = displayed_region(view, state);
    let entity = cx.entity();
    let rail = super::timeline_strip::loop_surface()
        .flex()
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
                    cx.new(|_| super::transport::MiniTooltip { kind: crate::tooltips::Kind::control("scope bar"),
                        description: "drag empty rail to set scope · drag scope bar to move · shift-click selects scope · shift-drag steps scope length · cmd-click toggles · cmd-drag is free".into(),
                    })
                    .into()
                })
                .child(loop_grid_layer(grid, viewport))
                .children(
                    shown_region
                        .and_then(|region| loop_region_bar(region, viewport, &state.project)),
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
                                        || !super::arrangement::bounds_or_leading_buffer_contains(
                                            bounds,
                                            event.position,
                                        )
                                    {
                                        return;
                                    }
                                    let started = down_entity.update(cx, |view, cx| {
                                        if view.track_name.is_editing() || view.clip_name_edit.is_some()
                                            || view.project_tempo_editing(cx) || view.gain_numeric_editing(cx)
                                            || view.clip_numeric_editing(cx) { return false; }
                                        let started = view.begin_loop_rail_gesture(
                                            bounds,
                                            event.position,
                                            event.modifiers,
                                            viewport,
                                            current_region,
                                        );
                                        if started { view.focus.focus(window); }
                                        started
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
                                            let cancelled = view.cancel_loop_rail_gesture();
                                            let hover = if super::arrangement::bounds_or_leading_buffer_contains(
                                                move_bounds,
                                                event.position,
                                            ) {
                                                match view.loop_rail_hover(
                                                    f32::from(event.position.x),
                                                    viewport,
                                                    move_bounds,
                                                ) {
                                                    Hover::Paint => CursorStyle::Crosshair,
                                                    Hover::Move => CursorStyle::OpenHand,
                                                    Hover::Resize => CursorStyle::ResizeLeftRight,
                                                }
                                            } else {
                                                CursorStyle::Crosshair
                                            };
                                            if view.loop_rail_cursor == hover {
                                                return cancelled;
                                            }
                                            view.loop_rail_cursor = hover;
                                            true
                                        });
                                        if hover {
                                            window.refresh();
                                        }
                                        return;
                                    }
                                    let (owned, changed) = move_entity.update(cx, |view, _| {
                                        let owned = view.loop_rail_gesture.is_some();
                                        let changed = if view.focus.is_focused(window) {
                                            view.update_loop_rail_gesture(event.position, event.modifiers)
                                        } else {
                                            view.cancel_loop_rail_gesture()
                                        };
                                        (owned, changed)
                                    });
                                    if owned {
                                        cx.stop_propagation();
                                    }
                                    if changed {
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
                                        if !view.focus.is_focused(window) { return view.cancel_loop_rail_gesture(); }
                                        view.finish_loop_rail_gesture(
                                            event.position,
                                            event.modifiers,
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
        .child(div().w(px(view.track_header_width())).h_full());
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
    if free {
        return Some(frame);
    }
    view.snap_grid.snap_frame(project, frame, viewport, width)
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

fn loop_region_bar(
    region: LoopRegion,
    viewport: Viewport,
    project: &hex_project::Project,
) -> Option<Div> {
    let viewport_end = viewport.end()?;
    if viewport.length == 0 {
        return None;
    }
    let start = region.start().max(viewport.start);
    let end = region.end().min(viewport_end);
    if start > end {
        return None;
    }
    let raw_start =
        ((i128::from(region.start()) - i128::from(viewport.start)) as f64) / viewport.length as f64;
    let raw_end =
        ((i128::from(region.end()) - i128::from(viewport.start)) as f64) / viewport.length as f64;
    Some(
        super::timeline_strip::loop_range(
            raw_start,
            raw_end,
            region.enabled(),
            [
                "arrangement-loop-start-marker",
                "arrangement-loop-end-marker",
            ],
        )?
        .debug_selector(|| "loop-rail-range".into())
        .children(
            (region.start() >= viewport.start && binary_duration(project, region)).then(|| {
                div()
                    .debug_selector(|| "loop-binary-length-marker".into())
                    .absolute()
                    .left(px(4.0))
                    .top_0()
                    .size(px(5.0))
                    .rounded_full()
                    .bg(rgb(theme::color(theme::PaletteRole::Background)))
            }),
        ),
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

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "loop_bar_ui_tests.rs"]
mod loop_bar_ui_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "loop_rail/pending_ui_tests.rs"]
mod pending_ui_tests;
