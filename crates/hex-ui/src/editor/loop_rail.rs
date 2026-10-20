//! Compact Arrangement loop-range rail with a control-side preview gesture.
use super::controller::RequestAdmission;
use super::{EditorMode, EditorView, musical_time::GridLine, viewport::Viewport};
use crate::theme;
use gpui::{
    Bounds, Context, CursorStyle, Div, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, canvas, div, prelude::*, px, relative, rgb,
};
use hex_project::{Command, LoopRegion};
use hex_session::service::Request;
use hex_session::snapshot_exports::SnapshotTransportError;
use std::sync::atomic::{AtomicU64, Ordering};

// File-local monotonic UI gesture incarnation, never a source/admission token.
// Exhaustion refuses a new gesture instead of reusing an old task's identity.
static NEXT_LOOP_GESTURE: AtomicU64 = AtomicU64::new(1);
fn next_loop_incarnation(counter: &AtomicU64) -> Option<u64> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .ok()
}

mod pending;

#[derive(Clone, Copy, Debug)]
pub(super) struct PendingLoop {
    generation: u64,
    region: LoopRegion,
    remaining: Option<usize>,
}

const RAIL_HEIGHT: f32 = 18.0;
const BAR_INSET: f32 = 4.0;
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
    incarnation: u64,
    pending_release: Option<PendingRelease>,
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
}

#[derive(Clone, Copy, Debug)]
struct PendingRelease {
    position: gpui::Point<gpui::Pixels>,
    modifiers: Modifiers,
}
#[derive(Clone, Copy, Debug)]
enum PreparedLoopUpdate {
    NoGesture,
    Cancel,
    Candidate(LoopRailGesture),
}
#[derive(Clone, Copy, Debug)]
enum LoopFinish {
    Handled(bool),
    RetryableQueueCapacity,
    AdmissionRefused,
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

        let Some(incarnation) = next_loop_incarnation(&NEXT_LOOP_GESTURE) else {
            self.error = Some("loop gesture identity limit reached".into());
            return false;
        };
        let candidate = state.read_with(|snapshot| {
            let project = snapshot.model();
            let (mode, preview, toggle_on_click) = if let Some(region) = region
                && let Some(mode) = region_edit_mode(region, viewport, bounds, x)
            {
                (mode, Some(region), modifiers.platform)
            } else {
                let anchor = snap_frame(
                    self,
                    &project,
                    frame,
                    viewport,
                    timeline_width,
                    modifiers.platform,
                )?;
                (EditMode::Paint { anchor }, None, false)
            };
            Some(LoopRailGesture {
                incarnation,
                pending_release: None,
                generation: self.project_generation,
                revision: snapshot.revision(),
                loop_region: region,
                viewport,
                timeline_left,
                timeline_width,
                mode,
                preview,
                pointer_down: position,
                last_pointer: position,
                moved: false,
                click_disqualified: false,
                toggle_on_click,
            })
        });
        match candidate {
            Ok(Some(gesture)) => {
                self.loop_rail_gesture = Some(gesture);
                true
            }
            Ok(None) => false,
            Err(error) => {
                self.report_loop_read_refusal(error);
                false
            }
        }
    }

    fn report_loop_read_refusal(&mut self, error: SnapshotTransportError) {
        if error != SnapshotTransportError::Busy {
            self.error = Some("loop source is unavailable".into());
        }
    }

    // Compute the complete Copy candidate under one actual snapshot read. No
    // gesture writes, enqueue, notification or scheduling occurs under its guard.
    fn prepare_loop_rail_update(
        &self,
        position: gpui::Point<gpui::Pixels>,
        modifiers: Modifiers,
    ) -> Result<PreparedLoopUpdate, SnapshotTransportError> {
        let x = f32::from(position.x);
        let Some(mut gesture) = self.loop_rail_gesture else {
            return Ok(PreparedLoopUpdate::NoGesture);
        };
        let state = self.state.as_ref().ok_or(SnapshotTransportError::Stale)?;
        state.read_with(|snapshot| {
            let project = snapshot.model();
            if self.mode != EditorMode::Arrangement
                || gesture.generation != self.project_generation
                || gesture.revision != snapshot.revision()
                || gesture.loop_region != project.loop_region()
                || gesture.viewport != self.visible_viewport(snapshot, snapshot.timeline_frames())
                || modifiers.alt
                || modifiers.control
                || self.context_menu.is_some()
                || self.opening_project
                || self.close_prompt_open
                || !x.is_finite()
                || !f32::from(position.y).is_finite()
            {
                return PreparedLoopUpdate::Cancel;
            }
            let Some(frame) = super::seek::pointer_to_frame(
                x,
                gesture.timeline_left,
                gesture.timeline_width,
                gesture.viewport,
            ) else {
                return PreparedLoopUpdate::Candidate(gesture);
            };
            gesture.last_pointer = position;
            gesture.moved |= (x - f32::from(gesture.pointer_down.x)).abs() > 3.0;
            // Vertical travel disqualifies a click without qualifying a horizontal move.
            gesture.click_disqualified |= gesture.moved
                || (f32::from(position.y) - f32::from(gesture.pointer_down.y)).abs() > 3.0;
            if let EditMode::Move { original, anchor } = gesture.mode {
                if !gesture.moved {
                    gesture.preview = Some(original);
                    return PreparedLoopUpdate::Candidate(gesture);
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
                        &project,
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
                return PreparedLoopUpdate::Candidate(gesture);
            }
            let Some(frame) = snap_frame(
                self,
                &project,
                frame,
                gesture.viewport,
                gesture.timeline_width,
                modifiers.platform,
            ) else {
                return PreparedLoopUpdate::Candidate(gesture);
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
            PreparedLoopUpdate::Candidate(gesture)
        })
    }

    fn update_loop_rail_gesture(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        modifiers: Modifiers,
    ) -> bool {
        if self
            .loop_rail_gesture
            .is_some_and(|gesture| gesture.pending_release.is_some())
        {
            return true;
        }
        match self.prepare_loop_rail_update(position, modifiers) {
            Ok(PreparedLoopUpdate::NoGesture) => false,
            Ok(PreparedLoopUpdate::Cancel) => self.cancel_loop_rail_gesture(),
            Ok(PreparedLoopUpdate::Candidate(gesture)) => {
                self.loop_rail_gesture = Some(gesture);
                true
            }
            Err(error) => {
                self.report_loop_read_refusal(error);
                true
            }
        }
    }

    fn finish_loop_rail_gesture_checked(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        modifiers: Modifiers,
    ) -> Result<LoopFinish, SnapshotTransportError> {
        let gesture = match self.prepare_loop_rail_update(position, modifiers)? {
            PreparedLoopUpdate::NoGesture => return Ok(LoopFinish::Handled(false)),
            PreparedLoopUpdate::Cancel => {
                self.cancel_loop_rail_gesture();
                return Ok(LoopFinish::Handled(true));
            }
            PreparedLoopUpdate::Candidate(gesture) => gesture,
        };
        let admission =
            if !gesture.click_disqualified && gesture.toggle_on_click && modifiers.platform {
                self.enqueue_with_admission(Request::Edit(Command::ToggleLoopRegion))
            } else if matches!(gesture.mode, EditMode::Move { .. }) && !gesture.moved {
                RequestAdmission::Accepted
            } else if let Some(region) = gesture.preview {
                if Some(region) == gesture.loop_region {
                    RequestAdmission::Accepted
                } else {
                    self.submit_loop_preview(gesture.generation, region)
                }
            } else {
                self.error = Some("loop region needs a non-empty time range".into());
                RequestAdmission::Accepted
            };
        match admission {
            RequestAdmission::Accepted => {
                self.loop_rail_gesture = None;
                Ok(LoopFinish::Handled(true))
            }
            RequestAdmission::RetryableQueueCapacity => {
                self.loop_rail_gesture = Some(gesture);
                Ok(LoopFinish::RetryableQueueCapacity)
            }
            RequestAdmission::Refused => {
                self.loop_rail_gesture = Some(gesture);
                Ok(LoopFinish::AdmissionRefused)
            }
        }
    }

    fn finish_loop_rail_gesture(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(gesture) = self.loop_rail_gesture else {
            return false;
        };
        if gesture.pending_release.is_some() {
            return true;
        }
        match self.finish_loop_rail_gesture_checked(position, modifiers) {
            Ok(LoopFinish::Handled(handled)) => handled,
            Ok(LoopFinish::AdmissionRefused) => {
                if let Some(current) = self.loop_rail_gesture.as_mut()
                    && current.incarnation == gesture.incarnation
                {
                    current.pending_release = Some(PendingRelease {
                        position,
                        modifiers,
                    });
                }
                true
            }
            Ok(LoopFinish::RetryableQueueCapacity) | Err(SnapshotTransportError::Busy) => {
                // One original release and one task per incarnation. Ordinary
                // moves/modifier refreshes cannot change this pending candidate.
                if let Some(current) = self.loop_rail_gesture.as_mut()
                    && current.incarnation == gesture.incarnation
                {
                    current.pending_release = Some(PendingRelease {
                        position,
                        modifiers,
                    });
                    self.retry_loop_rail_release(gesture.incarnation, cx);
                }
                true
            }
            Err(error) => {
                self.cancel_loop_rail_gesture();
                self.report_loop_read_refusal(error);
                true
            }
        }
    }

    fn retry_loop_rail_release(&mut self, incarnation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(5))
                    .await;
                let retry = this
                    .update(cx, |view, cx| {
                        let Some(gesture) = view.loop_rail_gesture else {
                            return false;
                        };
                        if gesture.incarnation != incarnation {
                            return false;
                        }
                        let Some(release) = gesture.pending_release else {
                            return false;
                        };
                        match view
                            .finish_loop_rail_gesture_checked(release.position, release.modifiers)
                        {
                            Ok(LoopFinish::RetryableQueueCapacity)
                            | Err(SnapshotTransportError::Busy) => true,
                            Ok(LoopFinish::Handled(_) | LoopFinish::AdmissionRefused) => {
                                cx.notify();
                                false
                            }
                            Err(error) => {
                                view.cancel_loop_rail_gesture();
                                view.report_loop_read_refusal(error);
                                cx.notify();
                                false
                            }
                        }
                    })
                    .unwrap_or(false);
                if !retry {
                    break;
                }
            }
        })
        .detach();
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

    fn loop_rail_hover(
        &self,
        x: f32,
        viewport: Viewport,
        bounds: Bounds<gpui::Pixels>,
    ) -> Result<Hover, SnapshotTransportError> {
        let state = self.state.as_ref().ok_or(SnapshotTransportError::Stale)?;
        let region = state.read_with(|snapshot| snapshot.model().loop_region())?;
        Ok(
            match region.and_then(|region| region_edit_mode(region, viewport, bounds, x)) {
                Some(EditMode::Move { .. }) => Hover::Move,
                Some(EditMode::ResizeStart { .. } | EditMode::ResizeEnd { .. }) => Hover::Resize,
                Some(EditMode::Paint { .. }) | None => Hover::Paint,
            },
        )
    }
}

pub(super) fn view(
    view: &EditorView,
    revision: u64,
    current_region: Option<LoopRegion>,
    viewport: Viewport,
    grid: &[GridLine],
    cx: &mut Context<EditorView>,
) -> Div {
    let gesture = view.loop_rail_gesture.filter(|gesture| {
        gesture.generation == view.project_generation
            && gesture.revision == revision
            && gesture.loop_region == current_region
            && view.mode == EditorMode::Arrangement
    });
    let shown_region = gesture
        .and_then(|gesture| gesture.preview)
        .or_else(|| view.pending_loop_region())
        .or(current_region);
    let binary = shown_region.is_some_and(|region| {
        view.state.as_ref().is_some_and(|state| {
            state
                .read_with(|snapshot| {
                    binary_duration_by(region, |frame| {
                        snapshot.model().tempo().beat_lower_bound_for_frame(frame)
                    })
                })
                .unwrap_or(false)
        })
    });
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
                        description: "drag empty rail to set loop · drag bar to move · shift steps loop length · cmd-click toggles · cmd-drag is free".into(),
                    })
                    .into()
                })
                .child(loop_grid_layer(grid, viewport))
                .children(
                    shown_region
                        .and_then(|region| loop_region_bar(region, viewport, binary)),
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
                                            || view.sample_bpm_editing(cx) { return false; }
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
                                            if view.loop_rail_gesture.is_some_and(|gesture| gesture.pending_release.is_some()) { return false; }
                                            // Read before cancellation; a Busy hover cannot
                                            // consume an existing gesture or pending release.
                                            let kind = match view.loop_rail_hover(
                                                f32::from(event.position.x), viewport, move_bounds,
                                            ) {
                                                Ok(kind) => kind,
                                                Err(error) => { view.report_loop_read_refusal(error); return false; },
                                            };
                                            let cancelled = view.cancel_loop_rail_gesture();
                                            let hover = if move_bounds.contains(&event.position) {
                                                match kind {
                                                    Hover::Paint => CursorStyle::Crosshair,
                                                    Hover::Move => CursorStyle::OpenHand,
                                                    Hover::Resize => CursorStyle::ResizeLeftRight,
                                                }
                                            } else { CursorStyle::Crosshair };
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
                                    let finished = up_entity.update(cx, |view, cx| {
                                        if !view.focus.is_focused(window) { return view.cancel_loop_rail_gesture(); }
                                        view.finish_loop_rail_gesture(
                                            event.position,
                                            event.modifiers,
                                            cx,
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
    project: &dyn super::musical_time::MusicalProject,
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

fn loop_region_bar(region: LoopRegion, viewport: Viewport, binary: bool) -> Option<Div> {
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
            .opacity(if region.enabled() { 0.94 } else { 0.62 })
            .children((region.start() >= viewport.start && binary).then(|| {
                div()
                    .debug_selector(|| "loop-binary-length-marker".into())
                    .absolute()
                    .left_0()
                    .top(px(2.0))
                    .size(px(5.0))
                    .rounded_full()
                    .bg(rgb(theme::color(theme::PaletteRole::Background)))
            })),
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
#[cfg(test)]
fn binary_duration(project: &hex_project::Project, region: LoopRegion) -> bool {
    binary_duration_by(region, |frame| {
        project.tempo_map().beat_lower_bound_for_frame(frame)
    })
}
fn binary_duration_by(
    region: LoopRegion,
    lower_bound: impl Fn(u64) -> Result<u128, hex_project::Invalid>,
) -> bool {
    let interval = |frame: u64| -> Option<(u128, u128)> {
        let first = lower_bound(frame).ok()?;
        let after = lower_bound(frame.checked_add(1)?).ok()?;
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

#[cfg(test)]
mod tests {
    use super::{binary_duration, stepped_start, visible_region_pixels};
    use crate::editor::viewport::Viewport;
    use hex_project::LoopRegion;

    #[test]
    fn gesture_incarnations_never_reuse_the_last_identity_at_exhaustion() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let counter = AtomicU64::new(u64::MAX - 1);
        assert_eq!(super::next_loop_incarnation(&counter), Some(u64::MAX - 1));
        assert_eq!(super::next_loop_incarnation(&counter), None);
        assert_eq!(super::next_loop_incarnation(&counter), None);
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    }

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
