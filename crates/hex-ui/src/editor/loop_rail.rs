//! Compact Arrangement loop-range rail with a control-side preview gesture.
use super::{EditorMode, EditorView, ViewState, musical_time::GridLine, viewport::Viewport};
use crate::theme;
use gpui::{
    Bounds, Context, CursorStyle, Div, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, canvas, div, prelude::*, px, relative, rgb,
};
use hex_project::LoopRegion;

mod pending;

#[derive(Clone, Copy, Debug)]
pub(super) struct PendingLoop {
    generation: u64,
    region: LoopRegion,
    toggle: bool,
    remaining: Option<usize>,
    // Selection intent for automatic focus after a successful section-loop ACK.
    focus_intent: Option<u64>,
    start_intent: Option<u64>,
    launch: Option<hex_session::arrangement_seek::SectionLaunch>,
}

const EDGE_HIT_PIXELS: f32 = 6.0;

#[derive(Clone, Copy, Debug)]
enum EditMode {
    Paint { anchor: u64 },
    Move { original: LoopRegion, anchor: u64 },
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
    pointer_down: gpui::Point<gpui::Pixels>,
    last_pointer: gpui::Point<gpui::Pixels>,
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
            || self.theme_settings.open
            || self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.active_clip_drag.is_some()
            || self.active_browser_audio_drag.is_some()
            || self.active_native_device_drag.is_some()
            || self.active_trim_drag.is_some()
            || self.active_fade_drag.is_some()
            || self.active_midi_trim_drag.is_some()
            || self.arrangement_pan.is_some()
            || self.time_selection_gesture.is_some()
            || self.time_range_move_gesture.is_some()
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
            || self.opening_project
            || self.close_prompt_open
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
            return true;
        };
        gesture.last_pointer = position;
        gesture.moved |= (x - f32::from(gesture.pointer_down.x)).abs() > 3.0;
        // Vertical travel disqualifies a click without qualifying a horizontal move.
        gesture.click_disqualified |= gesture.moved
            || (f32::from(position.y) - f32::from(gesture.pointer_down.y)).abs() > 3.0;
        if let EditMode::Move { original, anchor } = gesture.mode {
            if !gesture.moved {
                gesture.preview = Some(original);
                self.loop_rail_gesture = Some(gesture);
                return true;
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
            self.loop_rail_gesture = Some(gesture);
            return true;
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
            return true;
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
        self.loop_rail_gesture = Some(gesture);
        true
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
            self.error = Some("loop region needs a non-empty time range".into());
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
                    cx.new(|_| super::transport::MiniTooltip { kind: crate::tooltips::Kind::control("loop range"),
                        description: "drag empty rail to set loop · drag bar to move · shift-click selects range · shift-drag steps loop length · cmd-click toggles · cmd-drag is free".into(),
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
                                        || !bounds.contains(&event.position)
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
                                            let hover = if move_bounds.contains(&event.position) {
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
                                    let changed = move_entity.update(cx, |view, _| {
                                        if !view.focus.is_focused(window) { return view.cancel_loop_rail_gesture(); }
                                        view.update_loop_rail_gesture(
                                            event.position,
                                            event.modifiers,
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

// Share the exact hit budget between hover and capture. Narrow bars retain a
// central body; the full outside endpoint radius remains usable at every width.
fn region_edit_mode(
    region: LoopRegion,
    viewport: Viewport,
    bounds: Bounds<gpui::Pixels>,
    x: f32,
) -> Option<EditMode> {
    let left = f32::from(bounds.origin.x);
    let width = f32::from(bounds.size.width);
    let frame = super::seek::pointer_to_frame(x, left, width, viewport)?;
    let (start_x, end_x) = visible_region_pixels(region, viewport, left, width)?;
    let radius = if end_x > start_x && x >= start_x && x <= end_x {
        EDGE_HIT_PIXELS.min((end_x - start_x) / 3.0)
    } else {
        EDGE_HIT_PIXELS
    };
    let start_distance = if region.start() >= viewport.start {
        (x - start_x).abs()
    } else {
        f32::INFINITY
    };
    let end_distance = if viewport.end().is_some_and(|end| region.end() <= end) {
        (x - end_x).abs()
    } else {
        f32::INFINITY
    };
    if start_distance <= radius && start_distance <= end_distance {
        Some(EditMode::ResizeStart {
            fixed_end: region.end(),
            enabled: region.enabled(),
        })
    } else if end_distance <= radius {
        Some(EditMode::ResizeEnd {
            fixed_start: region.start(),
            enabled: region.enabled(),
        })
    } else if (frame >= region.start() && frame < region.end())
        || (end_x > start_x && x >= start_x && x <= end_x)
    {
        // A visible body remains a body when frame rounding reaches its end.
        Some(EditMode::Move {
            original: region,
            anchor: frame,
        })
    } else {
        None
    }
}

// Closed viewport edges retain point intersections for endpoint resize capture.
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
    if start > end {
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
                    .top(px(-6.0))
                    .size(px(5.0))
                    .rounded_full()
                    .bg(rgb(theme::color(theme::PaletteRole::Background)))
            }),
        ),
    )
}

/// Full-frame loop-length steps relative to the original start, including the
/// origin when a further full negative/positive step would exceed checked bounds.
fn stepped_start(region: LoopRegion, delta: i128) -> Option<u64> {
    let length = region.end().checked_sub(region.start())?;
    if length == 0 {
        return None;
    }
    let magnitude = (delta.unsigned_abs() + u128::from(length) / 2) / u128::from(length);
    let magnitude = i128::try_from(magnitude).ok()?;
    let steps = if delta < 0 { -magnitude } else { magnitude };
    let steps = steps.clamp(
        -i128::from(region.start() / length),
        i128::from((u64::MAX - region.end()) / length),
    );
    u64::try_from(i128::from(region.start()) + steps * i128::from(length)).ok()
}

/// Frame endpoints retain quantized musical intervals, not authored beat IDs.
/// Accept exactly one binary Q32.32 duration whose canonical forward rounding can
/// place its two beat endpoints at these stored frames (no pixel/epsilon test).
/// Arrangement time-selection markers share this same read-only predicate.
pub(super) fn binary_duration(project: &hex_project::Project, region: LoopRegion) -> bool {
    let map = project.tempo_map();
    let interval = |frame: u64| -> Option<(u128, u128)> {
        let first = map.beat_lower_bound_for_frame(frame).ok()?;
        let after = map.beat_lower_bound_for_frame(frame.checked_add(1)?).ok()?;
        (first < after && first < hex_project::TempoMap::BEAT_RANGE_SENTINEL)
            .then_some((first, after.checked_sub(1)?))
    };
    let Some((start_lo, start_hi)) = interval(region.start()) else {
        return false;
    };
    let Some((end_lo, end_hi)) = interval(region.end()) else {
        return false;
    };
    let minimum = end_lo.saturating_sub(start_hi).max(1);
    let maximum = end_hi.saturating_sub(start_lo);
    minimum
        .checked_next_power_of_two()
        .is_some_and(|candidate| {
            candidate <= maximum
                && candidate < hex_project::TempoMap::BEAT_RANGE_SENTINEL
                && candidate.checked_mul(2).is_none_or(|next| next > maximum)
        })
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

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::{binary_duration, stepped_start, visible_region_pixels};
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

    #[test]
    fn edge_point_ranges_keep_the_existing_resize_owner() {
        let viewport = Viewport {
            start: 100,
            length: 100,
        };
        let bounds = gpui::Bounds::new(
            gpui::point(gpui::px(10.0), gpui::px(0.0)),
            gpui::size(gpui::px(200.0), gpui::px(18.0)),
        );
        assert!(matches!(
            super::region_edit_mode(
                LoopRegion::new(50, 100, true).unwrap(),
                viewport,
                bounds,
                12.0
            ),
            Some(super::EditMode::ResizeEnd {
                fixed_start: 50,
                ..
            })
        ));
        assert!(matches!(
            super::region_edit_mode(
                LoopRegion::new(200, 250, true).unwrap(),
                viewport,
                bounds,
                208.0
            ),
            Some(super::EditMode::ResizeStart { fixed_end: 250, .. })
        ));
        for (start, end) in [(50, 99), (201, 250)] {
            assert!(
                super::region_edit_mode(
                    LoopRegion::new(start, end, true).unwrap(),
                    viewport,
                    bounds,
                    12.0
                )
                .is_none()
            );
        }
    }

    #[test]
    fn binary_marker_refuses_ambiguous_subframe_musical_durations() {
        let project = hex_project::Project::new(48_000).unwrap();
        assert!(!binary_duration(
            &project,
            LoopRegion::new(0, 1, false).unwrap()
        ));
    }

    #[test]
    fn musical_marker_uses_forward_rounding_across_tempo_points() {
        use hex_project::{BeatTime, Command, Editor, Project, Tempo};
        let mut editor = Editor::new(Project::new(44_100).unwrap(), 8).unwrap();
        for (beat, bpm) in [(0, 137_000_000), (2, 93_000_000)] {
            editor
                .execute(Command::SetTempoPoint {
                    beat: BeatTime::from_raw(beat << 32),
                    tempo: Tempo::new(bpm).unwrap(),
                })
                .unwrap();
        }
        let project = editor.project();
        for start_raw in [0, 1 << 31, 1 << 32] {
            for (length, expected) in [
                (1 << 30, true),
                (1 << 31, true),
                (2 << 32, true),
                (3 << 32, false),
                (4 << 32, true),
            ] {
                let start = project
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_raw(start_raw))
                    .unwrap();
                let end = project
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_raw(start_raw + length))
                    .unwrap();
                assert_eq!(
                    binary_duration(project, LoopRegion::new(start, end, true).unwrap()),
                    expected,
                    "start={start_raw}, duration={length}, frames={start}..{end}"
                );
            }
        }
        assert!(!binary_duration(
            project,
            LoopRegion::new(u64::MAX - 1, u64::MAX, false).unwrap()
        ));
    }

    #[test]
    fn hit_budget_preserves_outside_capture_clipped_bodies_and_wide_edges() {
        use super::{EditMode, region_edit_mode};
        use gpui::{point, px};
        let bounds = gpui::Bounds::new(point(px(100.0), px(0.0)), gpui::size(px(300.0), px(18.0)));
        let viewport = Viewport {
            start: 0,
            length: 300,
        };
        let kind = |region, viewport, x| match region_edit_mode(region, viewport, bounds, x) {
            Some(EditMode::Move { .. }) => "move",
            Some(EditMode::ResizeStart { .. }) => "start",
            Some(EditMode::ResizeEnd { .. }) => "end",
            Some(EditMode::Paint { .. }) | None => "none",
        };
        for width in [1, 8, 12, 18, 48] {
            let r = LoopRegion::new(100, 100 + width, true).unwrap();
            assert_eq!(kind(r, viewport, 200.0), "start");
            assert_eq!(kind(r, viewport, 200.0 + width as f32), "end");
            assert_eq!(kind(r, viewport, 194.0), "start");
            assert_eq!(kind(r, viewport, 193.9), "none");
            assert_eq!(kind(r, viewport, 206.0 + width as f32), "end");
            assert_eq!(kind(r, viewport, 206.1 + width as f32), "none");
            assert_eq!(kind(r, viewport, 200.0 + width as f32 / 2.0), "move");
        }
        let wide = LoopRegion::new(100, 148, false).unwrap();
        assert_eq!(kind(wide, viewport, 206.0), "start");
        assert_eq!(kind(wide, viewport, 206.1), "move");
        assert_eq!(kind(wide, viewport, 241.9), "move");
        assert_eq!(kind(wide, viewport, 242.0), "end");
        // A clipped-away start cannot steal the visible body, even when only 3px remain.
        let clipped = Viewport {
            start: 145,
            length: 300,
        };
        assert_eq!(kind(wide, clipped, 100.0), "move");
        assert_eq!(kind(wide, clipped, 103.0), "end");
        let clipped_end = Viewport {
            start: 0,
            length: 110,
        };
        assert_eq!(kind(wide, clipped_end, 400.0), "move");
    }

    #[test]
    fn length_steps_keep_relative_origin_ties_and_checked_timeline_bounds() {
        let region = LoopRegion::new(65, 95, false).unwrap();
        for (delta, expected) in [
            (-1000, 5),
            (-46, 5),
            (-45, 5),
            (-44, 35),
            (-15, 35),
            (-14, 65),
            (14, 65),
            (15, 95),
            (44, 95),
            (45, 125),
        ] {
            assert_eq!(stepped_start(region, delta), Some(expected));
        }
        let last = LoopRegion::new(u64::MAX - 65, u64::MAX - 35, true).unwrap();
        assert_eq!(stepped_start(last, 1000), Some(u64::MAX - 35));
        assert_eq!(stepped_start(last, i128::MAX), Some(u64::MAX - 35));
        assert_eq!(stepped_start(region, i128::MIN), Some(5));
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "loop_bar_ui_tests.rs"]
mod loop_bar_ui_tests;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "loop_rail/pending_ui_tests.rs"]
mod pending_ui_tests;
