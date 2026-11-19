//! Original-frame pointer drafts. Release transfers the value to the existing
//! sampler parameter serial; input cancellation never cancels an admitted edit.
//! The Window lease owns only the held handle, independently of that serial.
use super::*;
use hex_engine::SamplerParameter;

pub(super) fn handle(start: bool, fraction: f64, enabled: bool) -> Div {
    div()
        .absolute()
        .left(relative(fraction as f32))
        .ml(px(-5.0))
        .w(px(10.0))
        .top_0()
        .bottom(px(RULER_HEIGHT))
        .when(enabled, |handle| handle.cursor_col_resize())
        .debug_selector(move || {
            if start {
                "sampler-source-start-handle"
            } else {
                "sampler-source-end-handle"
            }
            .into()
        })
}

pub(super) struct Gesture {
    source: SourceTarget,
    target: crate::editor::sampler_parameter::Target,
    parameter: SamplerParameter,
    bounds: Bounds<Pixels>,
    original: u64,
    frame: u64,
    invalid: bool,
    anchor_x: Pixels,
    moved: bool,
}

pub(in crate::editor) fn cancel(view: &mut EditorView) -> bool {
    view.device_panel.sampler_cursor.boundary.take().is_some()
}

pub(super) fn reconcile(view: &mut EditorView) {
    if view
        .device_panel
        .sampler_cursor
        .boundary
        .as_ref()
        .is_some_and(|g| !g.source.current(view) || !g.target.current(view) || view.edit_pending())
    {
        cancel(view);
    }
}

pub(in crate::editor) fn preview(
    view: &EditorView,
    owner: hex_session::service::SamplerParameterOwner,
    device: DeviceId,
    parameter: SamplerParameter,
) -> Option<u64> {
    view.device_panel
        .sampler_cursor
        .boundary
        .as_ref()
        .filter(|g| {
            g.source.owner == owner
                && g.source.device == device
                && g.parameter == parameter
                && g.target.current(view)
        })
        .map(|g| g.frame)
}

pub(super) fn begin(
    view: &mut EditorView,
    source: SourceTarget,
    bounds: Bounds<Pixels>,
    position: gpui::Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) -> bool {
    if view.edit_pending()
        || !source.current(view)
        || ![
            bounds.left(),
            bounds.top(),
            bounds.size.width,
            bounds.size.height,
            position.x,
            position.y,
        ]
        .into_iter()
        .all(|n| f32::from(n).is_finite())
        || bounds.size.width <= px(0.0)
        || bounds.size.height <= px(0.0)
    {
        return false;
    }
    let Some(device) = view.state.as_ref().and_then(|state| match source.owner {
        hex_session::service::SamplerParameterOwner::Track(track) => {
            state.project.track(track)?.instrument()
        }
        hex_session::service::SamplerParameterOwner::Source(source) => {
            Some(state.project.native_source(source)?.instrument())
        }
    }) else {
        return false;
    };
    let parameter_at =
        |parameter| hex_engine::sampler_parameter_value_on_control(parameter, device.state()).ok();
    let (Some(start), Some(end)) = (
        parameter_at(SamplerParameter::StartFrame),
        parameter_at(SamplerParameter::EndFrame),
    ) else {
        return false;
    };
    let x = f32::from(position.x - bounds.left());
    let width = f32::from(bounds.size.width);
    let left = (x - (start / source.frames as f64) as f32 * width).abs();
    let right = (x - (end / source.frames as f64) as f32 * width).abs();
    if left.min(right) > 5.0 {
        return false;
    }
    let (parameter, value) = if left <= right {
        (SamplerParameter::StartFrame, start)
    } else {
        (SamplerParameter::EndFrame, end)
    };
    let Some(target) = view.sampler_boundary_target(source.owner, source.device, parameter) else {
        return true;
    };
    let owner = cx.entity().downgrade();
    let cancelled = owner.clone();
    if !window.capture_pointer(
        cx.entity_id(),
        MouseButton::Left,
        gpui::CursorStyle::ResizeColumn,
        move |event, window, cx| {
            let moved = event.downcast_ref::<gpui::MouseMoveEvent>();
            let released = event
                .downcast_ref::<gpui::MouseUpEvent>()
                .filter(|e| e.button == MouseButton::Left);
            if moved.is_none() && released.is_none() {
                return;
            }
            let _ = owner.update(cx, |view, cx| {
                if !owns(view, source, target) {
                    return;
                }
                if !view.clip_view_open
                    || !view.device_panel_active
                    || !view.focus.is_focused(window)
                {
                    cancel(view);
                } else if let Some(event) = moved {
                    move_pointer(view, bounds, event, cx);
                } else if let Some(event) = released {
                    release(view, bounds, event.position, cx);
                }
                cx.notify();
            });
        },
        move |_, cx| {
            let _ = cancelled.update(cx, |view, cx| {
                if owns(view, source, target) {
                    cancel(view);
                    cx.notify();
                }
            });
        },
        cx,
    ) {
        // The handle was hit, but another owner retained input. Do not stamp a
        // cursor or construct a draft after refused admission.
        return true;
    }
    view.device_panel.sampler_cursor.boundary = Some(Gesture {
        source,
        target,
        parameter,
        bounds,
        original: value as u64,
        frame: value as u64,
        invalid: false,
        anchor_x: position.x,
        moved: false,
    });
    cx.notify();
    true
}

fn owns(
    view: &EditorView,
    source: SourceTarget,
    target: crate::editor::sampler_parameter::Target,
) -> bool {
    view.device_panel
        .sampler_cursor
        .boundary
        .as_ref()
        .is_some_and(|g| g.source == source && g.target == target)
}

/// Read-only witness: geometry/source changes retire the input lease after draw.
pub(super) fn renew(
    view: &EditorView,
    bounds: Bounds<Pixels>,
    owner: gpui::EntityId,
    window: &mut Window,
) {
    if view.clip_view_open
        && view.device_panel_active
        && view.focus.is_focused(window)
        && view
            .device_panel
            .sampler_cursor
            .boundary
            .as_ref()
            .is_some_and(|g| {
                g.bounds == bounds
                    && g.source.current(view)
                    && g.target.current(view)
                    && !view.edit_pending()
            })
    {
        window.renew_pointer_capture(owner, gpui::CursorStyle::ResizeColumn);
    }
}

fn update(view: &mut EditorView, bounds: Bounds<Pixels>, position: gpui::Point<Pixels>) -> bool {
    let Some(mut gesture) = view.device_panel.sampler_cursor.boundary.take() else {
        return false;
    };
    if gesture.bounds != bounds
        || !gesture.source.current(view)
        || !gesture.target.current(view)
        || view.edit_pending()
    {
        view.error = Some("sampler boundary changed · drag canceled".into());
        return true;
    }
    let delta = f32::from(position.x - gesture.anchor_x);
    let width = f32::from(bounds.size.width);
    if !delta.is_finite()
        || !width.is_finite()
        || width <= 0.0
        || !f32::from(position.y).is_finite()
    {
        view.error = Some("sampler boundary geometry is unavailable".into());
        return true;
    }
    if !gesture.moved && delta.abs() < 2.0 {
        view.device_panel.sampler_cursor.boundary = Some(gesture);
        return true;
    }
    gesture.moved = true;
    let x = (gesture.original as f64 / gesture.source.frames as f64) as f32 * width + delta;
    let Some(frame) = frame_at_x(x.clamp(0.0, width), width, gesture.source.frames) else {
        view.error = Some("sampler boundary geometry is unavailable".into());
        return true;
    };
    match view.validate_sampler_boundary(gesture.target, frame) {
        Ok(()) => {
            gesture.frame = frame;
            gesture.invalid = false;
        }
        Err(error) => {
            gesture.invalid = true;
            view.error = Some(error.to_string().into());
        }
    }
    view.device_panel.sampler_cursor.boundary = Some(gesture);
    true
}

pub(super) fn move_pointer(
    view: &mut EditorView,
    bounds: Bounds<Pixels>,
    event: &gpui::MouseMoveEvent,
    _: &mut Context<EditorView>,
) -> bool {
    if view.device_panel.sampler_cursor.boundary.is_none() {
        return false;
    }
    if event.pressed_button != Some(MouseButton::Left) {
        return cancel(view);
    }
    update(view, bounds, event.position)
}

pub(super) fn release(
    view: &mut EditorView,
    bounds: Bounds<Pixels>,
    position: gpui::Point<Pixels>,
    _: &mut Context<EditorView>,
) -> bool {
    if !update(view, bounds, position) {
        return false;
    }
    if let Some(gesture) = view.device_panel.sampler_cursor.boundary.take() {
        if !gesture.invalid && gesture.frame != gesture.original {
            view.submit_sampler_boundary(gesture.target, gesture.frame);
        } else if !gesture.invalid {
            view.device_panel.sampler_cursor.cursor = Some((gesture.source, gesture.frame));
        }
    }
    true
}

pub(super) fn cursor_action(
    view: &EditorView,
    parameter: SamplerParameter,
    cx: &mut Context<EditorView>,
) -> gpui::Stateful<Div> {
    let start = parameter == SamplerParameter::StartFrame;
    let enabled = view
        .device_panel
        .sampler_cursor
        .cursor
        .is_some_and(|(target, _)| target.current(view));
    let selector = if start {
        "sampler-start-at-cursor"
    } else {
        "sampler-end-at-cursor"
    };
    hex_widgets::action(
        theme::text(if start { "[←" } else { "→]" }),
        theme::action_style(),
    )
    .id(selector)
    .debug_selector(move || selector.into())
    .w(px(24.0))
    .h(px(RULER_HEIGHT))
    .when(!enabled, |button| button.cursor_default().opacity(0.4))
    .tooltip(move |_, cx| {
        cx.new(|_| crate::editor::transport::MiniTooltip {
            kind: crate::tooltips::Kind::Identification,
            description: theme::text(if start {
                "set sample start at cursor"
            } else {
                "set sample end at cursor"
            }),
        })
        .into()
    })
    .on_mouse_down(
        MouseButton::Left,
        cx.listener(move |view, _, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
            if !enabled || view.edit_pending() {
                return;
            }
            if enabled
                && let Some((source, frame)) = view
                    .device_panel
                    .sampler_cursor
                    .cursor
                    .filter(|(source, _)| source.current(view))
                && let Some(target) =
                    view.sampler_boundary_target(source.owner, source.device, parameter)
            {
                view.submit_sampler_boundary(target, frame);
            }
            view.focus.focus(window);
            cx.notify();
        }),
    )
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "sampler_boundary_tests.rs"]
mod tests;
