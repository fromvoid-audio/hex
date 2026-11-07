//! View-only horizontal navigation shared by track and return device rails.
//! A Window pointer lease owns held input; cancellation keeps the last offset.
use super::*;
use gpui::{
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollHandle, ScrollWheelEvent, canvas, fill,
    point, size,
};

#[derive(Default)]
pub(in crate::editor) struct Navigation {
    pub(super) scroll: ScrollHandle,
    gesture: Option<Gesture>,
    identity: Option<(u64, DevicePanelOwner)>,
    mode: Option<super::super::EditorMode>,
    overview_bounds: Option<Bounds<Pixels>>,
    pub(in crate::editor) detail_controls_bounds: Option<Bounds<Pixels>>,
}

struct Gesture {
    button: MouseButton,
    pointer_x: f32,
    offset: f32,
    scale: f32,
    focus: Option<gpui::FocusHandle>,
}

impl Gesture {
    fn cursor(&self) -> gpui::CursorStyle {
        if self.button == MouseButton::Middle {
            gpui::CursorStyle::ClosedHand
        } else {
            gpui::CursorStyle::PointingHand
        }
    }
}

fn current(view: &EditorView, window: &Window, cx: &App) -> bool {
    let navigation = &view.device_panel.navigation;
    view.clip_view_open
        && view.device_panel_active
        && navigation.identity
            == view
                .device_panel
                .owner
                .map(|owner| (view.project_generation, owner))
        && navigation.mode == Some(view.mode)
        && navigation
            .gesture
            .as_ref()
            .is_some_and(|g| g.focus == window.focused(cx))
}

impl Navigation {
    pub(in crate::editor) fn is_active(&self) -> bool {
        self.gesture.is_some()
    }

    pub(in crate::editor) fn cancel(&mut self) -> bool {
        self.gesture.take().is_some()
    }

    fn sync(
        &mut self,
        generation: u64,
        owner: Option<DevicePanelOwner>,
        mode: super::super::EditorMode,
    ) {
        let identity = owner.map(|owner| (generation, owner));
        if self.identity != identity {
            self.identity = identity;
            self.scroll = ScrollHandle::new();
            self.cancel();
        }
        if self.mode != Some(mode) {
            self.mode = Some(mode);
            self.cancel();
        }
    }

    fn set_offset(&self, offset: f32) {
        let max = f32::from(self.scroll.max_offset().width).max(0.0);
        if offset.is_finite() {
            self.scroll
                .set_offset(point(px(offset.clamp(-max, 0.0)), px(0.0)));
        }
    }

    fn update(&mut self, pointer_x: f32) -> bool {
        let Some(gesture) = &self.gesture else {
            return false;
        };
        self.set_offset(gesture.offset + (pointer_x - gesture.pointer_x) * gesture.scale);
        true
    }
}

/// Navigation owns the single scroll instance; windowing holds an alias to it.
/// Bind before capturing viewport observations or reconciling card geometry.
pub(super) fn sync_rail_scroll(view: &mut EditorView) {
    view.device_panel
        .navigation
        .sync(view.project_generation, view.device_panel.owner, view.mode);
    view.device_panel.chain_window.scroll = view.device_panel.navigation.scroll.clone();
}

pub(super) fn rail(view: &mut EditorView, chain: Stateful<Div>) -> Stateful<Div> {
    sync_rail_scroll(view);
    chain.track_scroll(&view.device_panel.navigation.scroll)
}

pub(in crate::editor) fn bottom_bar(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let open = view.clip_view_open && view.device_panel_active;
    // While open, the finalized device owner is authoritative (a draft can defer a switch).
    let owner = if open {
        view.device_panel.owner
    } else {
        view.focused_return
            .map(DevicePanelOwner::Return)
            .or_else(|| view.selected_track.map(DevicePanelOwner::Track))
            .or_else(|| {
                (view.device_panel.main_selected == Some(view.project_generation))
                    .then_some(DevicePanelOwner::Main)
            })
    };
    view.device_panel
        .navigation
        .sync(view.project_generation, owner, view.mode);
    if !open {
        view.device_panel.navigation.cancel();
    }
    let mut cards = Vec::new();
    if let Some(state) = &view.state {
        match owner {
            Some(DevicePanelOwner::Main) => {
                cards.extend(state.project.main_devices().iter().map(|device| {
                    (
                        device_card_width(NativeDeviceClass::Effect, device.state().type_id()),
                        device.id(),
                    )
                }))
            }
            Some(DevicePanelOwner::Track(id)) => {
                if let Some(track) = state.project.track(id) {
                    for node in track.ordered_device_chain() {
                        let (device, class) = match node {
                            hex_project::TrackDeviceRef::Instrument(device) => {
                                (device, NativeDeviceClass::Instrument)
                            }
                            hex_project::TrackDeviceRef::Effect(device) => {
                                (device, NativeDeviceClass::Effect)
                            }
                        };
                        cards.push((
                            device_card_width(class, device.state().type_id()),
                            device.id(),
                        ));
                    }
                }
            }
            Some(DevicePanelOwner::Return(id)) => {
                if let Some(track) = state
                    .project
                    .return_tracks()
                    .iter()
                    .find(|track| track.id() == id)
                {
                    cards.extend(track.devices().iter().map(|device| {
                        (
                            device_card_width(NativeDeviceClass::Effect, device.state().type_id()),
                            device.id(),
                        )
                    }));
                }
            }
            None => {}
        }
    }
    let entity = cx.entity();
    let scroll = view.device_panel.navigation.scroll.clone();
    let selected = view.device_panel.selected_device;
    let overview = div()
        .id("device-chain-overview")
        .debug_selector(|| "device-chain-overview".into())
        .flex_1()
        .min_w(px(24.0))
        .h(px(16.0))
        .relative()
        .bg(rgb(theme::color(theme::PaletteRole::Background)))
        .cursor(gpui::CursorStyle::PointingHand)
        .tooltip(|_, cx| {
            cx.new(|_| super::super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::control("device chain"),
                description:
                    "device chain · middle-drag devices · drag focus frame or click to scroll"
                        .into(),
            })
            .into()
        })
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    entity.update(cx, |view, _| {
                        view.device_panel.navigation.overview_bounds = Some(bounds)
                    });
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    let visible = f32::from(scroll.bounds().size.width).max(0.0);
                    let total = (visible + f32::from(scroll.max_offset().width)).max(1.0);
                    let card_total = cards
                        .iter()
                        .map(|(width, _)| width + DEVICE_CHAIN_PADDING)
                        .sum::<f32>()
                        + DEVICE_CHAIN_PADDING
                        + 40.0;
                    let total = if open { total } else { card_total.max(1.0) };
                    let mut x = DEVICE_CHAIN_PADDING;
                    for (card_width, id) in &cards {
                        let left = x / total * width;
                        let miniature_width = (card_width / total * width - 1.0)
                            .max(1.0)
                            .min((width - left).max(0.0));
                        let color = rgb(theme::color(if selected == Some(*id) {
                            theme::PaletteRole::Primary
                        } else {
                            theme::PaletteRole::Raised
                        }));
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.origin.x + px(left), bounds.origin.y + px(2.0)),
                                size(px(miniature_width), px(height - 4.0)),
                            ),
                            color,
                        ));
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.origin.x + px(left), bounds.origin.y + px(2.0)),
                                size(px(miniature_width), px(3.0)),
                            ),
                            rgb(theme::color(theme::PaletteRole::Muted)),
                        ));
                        x += card_width + DEVICE_CHAIN_PADDING;
                    }
                    let (left, focus_width) =
                        focus_frame(f32::from(scroll.offset().x), visible, total, width);
                    if open && !cards.is_empty() {
                        for (x, y, w, h) in [
                            (left, 0.0, focus_width, 1.0),
                            (left, height - 1.0, focus_width, 1.0),
                            (left, 0.0, 1.0, height),
                            (left + focus_width - 1.0, 0.0, 1.0, height),
                        ] {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                                    size(px(w), px(h)),
                                ),
                                rgb(theme::color(theme::PaletteRole::Text)),
                            ));
                        }
                    }
                    let view = entity.read(cx);
                    if current(view, window, cx)
                        && let Some(gesture) = view.device_panel.navigation.gesture.as_ref()
                    {
                        window.renew_pointer_capture(entity.entity_id(), gesture.cursor());
                    }
                    let wheel_entity = entity.clone();
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                        if !phase.capture() || !open || !bounds.contains(&event.position) {
                            return;
                        }
                        let delta = event.delta.pixel_delta(px(24.0));
                        wheel_entity.update(cx, |view, _| {
                            let navigation = &view.device_panel.navigation;
                            let delta = if delta.x != px(0.0) { delta.x } else { delta.y };
                            navigation.set_offset(f32::from(navigation.scroll.offset().x + delta));
                        });
                        cx.stop_propagation();
                        window.refresh();
                    });
                },
            )
            .size_full(),
        );
    let trim = open
        .then(|| super::render_automation_trim(view, cx))
        .flatten();
    let controls = view.clip_detail_controls(cx);
    div()
        .id("device-chain-bottom-bar")
        .relative()
        .flex()
        .items_center()
        .gap(px(4.0))
        .w(px(270.0))
        .min_w(px(160.0))
        .children(trim)
        .child(overview)
        .child(controls)
}

/// Root capture runs before numeric fields' outside-down/blur handlers.
pub(in crate::editor) fn capture_pointer_down(
    view: &mut EditorView,
    event: &MouseDownEvent,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    if view.context_menu.is_some() || view.theme_settings.open {
        return;
    }
    let open = view.clip_view_open && view.device_panel_active;
    if open
        && event.button == MouseButton::Left
        && view
            .device_panel
            .navigation
            .detail_controls_bounds
            .is_some_and(|bounds| bounds.contains(&event.position))
        && !finalize_controls(view, cx)
    {
        cx.stop_propagation();
        cx.notify();
        return;
    }
    if !open
        || view.device_panel.navigation.is_active()
        || !f32::from(event.position.x).is_finite()
        || !f32::from(event.position.y).is_finite()
    {
        return;
    }
    let navigation = &view.device_panel.navigation;
    let mut jump = None;
    let scale = if event.button == MouseButton::Middle
        && navigation.scroll.bounds().contains(&event.position)
    {
        1.0
    } else if event.button == MouseButton::Left {
        let Some(bounds) = navigation
            .overview_bounds
            .filter(|bounds| bounds.contains(&event.position))
        else {
            return;
        };
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return;
        }
        let visible = f32::from(navigation.scroll.bounds().size.width).max(0.0);
        let total = (visible + f32::from(navigation.scroll.max_offset().width)).max(1.0);
        let (left, focus_width) = focus_frame(
            f32::from(navigation.scroll.offset().x),
            visible,
            total,
            width,
        );
        let pointer = f32::from(event.position.x - bounds.origin.x);
        if pointer < left || pointer > left + focus_width {
            jump = Some(-(pointer - focus_width / 2.0) / width * total);
        }
        -total / width
    } else {
        return;
    };
    let identity = navigation.identity;
    let mode = navigation.mode;
    let button = event.button;
    let owner = cx.entity().downgrade();
    let cancelled = owner.clone();
    if !window.capture_pointer(
        cx.entity_id(),
        button,
        if button == MouseButton::Middle {
            gpui::CursorStyle::ClosedHand
        } else {
            gpui::CursorStyle::PointingHand
        },
        move |event, window, cx| {
            let moved = event.downcast_ref::<MouseMoveEvent>();
            let released = event
                .downcast_ref::<MouseUpEvent>()
                .filter(|e| e.button == button);
            let Some(position) = moved
                .map(|e| e.position)
                .or_else(|| released.map(|e| e.position))
            else {
                return;
            };
            let _ = owner.update(cx, |view, cx| {
                let navigation = &view.device_panel.navigation;
                if navigation.identity != identity
                    || navigation.mode != mode
                    || !navigation
                        .gesture
                        .as_ref()
                        .is_some_and(|g| g.button == button)
                {
                    return;
                }
                let valid = current(view, window, cx)
                    && f32::from(position.x).is_finite()
                    && f32::from(position.y).is_finite();
                let navigation = &mut view.device_panel.navigation;
                if valid {
                    navigation.update(f32::from(position.x));
                }
                if !valid || released.is_some() {
                    navigation.cancel();
                }
                window.refresh();
            });
        },
        move |window, cx| {
            let _ = cancelled.update(cx, |view, _| {
                let navigation = &mut view.device_panel.navigation;
                if navigation.identity == identity
                    && navigation.mode == mode
                    && navigation
                        .gesture
                        .as_ref()
                        .is_some_and(|g| g.button == button)
                {
                    navigation.cancel();
                    window.refresh();
                }
            });
        },
        cx,
    ) {
        return;
    }
    let navigation = &mut view.device_panel.navigation;
    if let Some(offset) = jump {
        navigation.set_offset(offset);
    }
    navigation.gesture = Some(Gesture {
        button: event.button,
        pointer_x: f32::from(event.position.x),
        offset: f32::from(navigation.scroll.offset().x),
        scale,
        focus: window.focused(cx),
    });
    cx.stop_propagation();
    cx.notify();
}

fn focus_frame(offset: f32, visible: f32, total: f32, width: f32) -> (f32, f32) {
    let total = total.max(1.0);
    let focus_width = (visible / total * width).clamp(1.0, width.max(1.0));
    (
        (-offset / total * width).clamp(0.0, (width - focus_width).max(0.0)),
        focus_width,
    )
}
