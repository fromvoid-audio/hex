//! Device chain drop-target geometry, selection listeners, and drag/drop edits.
use super::*;

// The panel appends at the tail or replaces the existing instrument in place.
pub(in crate::editor) fn sample_insertion_marker() -> Div {
    div()
        .absolute()
        .left_0()
        .top(px(DEVICE_CHAIN_PADDING))
        .w(px(2.0))
        .h(px(DEVICE_FRAME_HEIGHT - DEVICE_CHAIN_PADDING))
        // GPUI applies drag styles only to elements with a hitbox.
        .group("sampler-drop-insertion")
        .group_drag_over::<drag::BrowserAudioDrag>("sampler-drop-panel", |style| {
            style.bg(rgb(theme::color(theme::PaletteRole::Secondary)))
        })
}

#[derive(Clone)]
pub(in crate::editor) struct DeviceCardDropGeometry {
    pub(in crate::editor) track: TrackId,
    pub(in crate::editor) device: DeviceId,
    pub(in crate::editor) class: NativeDeviceClass,
    pub(in crate::editor) width: f32,
    pub(in crate::editor) next: Option<DeviceId>,
    pub(in crate::editor) first: Option<DeviceId>,
    pub(in crate::editor) visible_chain_bounds:
        std::rc::Rc<std::cell::RefCell<Option<Bounds<Pixels>>>>,
    /// Accepts browser sample drops that load or replace its sample.
    pub(in crate::editor) is_sampler: bool,
    pub(in crate::editor) replace_vst3: bool,
}

pub(in crate::editor) fn device_card_width(
    view: &EditorView,
    class: NativeDeviceClass,
    device: &hex_project::Device,
) -> f32 {
    if view.device_panel.collapsed.contains(&device.id()) {
        return fold::CARD_WIDTH;
    }
    let type_id = device.state().type_id();
    let frame_width = match (class, type_id.as_str().ok()) {
        (NativeDeviceClass::Instrument, Some("hex.instrument.operator")) => {
            operator_panel::frame_width(
                view.device_panel
                    .operator_pages
                    .get(&device.id())
                    .copied()
                    .unwrap_or_default(),
            )
        }
        (NativeDeviceClass::Instrument, Some("hex.instrument.sampler")) => {
            crate::device_composition::SAMPLER_FRAME_WIDTH
        }
        (NativeDeviceClass::Effect, Some("hex.utility")) => UTILITY_FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.eq8" | "hex.eq")) => eq8_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.saturator")) => saturator_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.limiter")) => limiter_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.compressor")) => compressor_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.filter")) => filter_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.gate")) => gate_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.reverb")) => reverb_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.delay")) => delay_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.auto-pan")) => auto_pan_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.overdrive")) => overdrive_panel::FRAME_WIDTH,
        (NativeDeviceClass::Effect, Some("hex.rattle")) => rattle_panel::FRAME_WIDTH,
        _ => DEVICE_FRAME_WIDTH,
    };
    frame_width + hex_widgets::DEVICE_FRAME_HEADER_WIDTH + device_meter::WIDTH
}

pub(in crate::editor) const REPLACE_PAD_WIDTH: f32 = 64.0;
pub(in crate::editor) const REPLACE_PAD_HEIGHT: f32 = 22.0;
pub(in crate::editor) const REPLACE_PAD_TOP: f32 = 6.0;

pub(in crate::editor) fn with_replace_pad(
    card: Stateful<Div>,
    selector: impl FnOnce() -> String + 'static,
    selected: bool,
) -> Stateful<Div> {
    card.child(
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(px(REPLACE_PAD_TOP))
            .flex()
            .justify_center()
            .child(
                div()
                    .debug_selector(selector)
                    .w(px(REPLACE_PAD_WIDTH))
                    .h(px(REPLACE_PAD_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(rgb(theme::color(if selected {
                        theme::PaletteRole::Secondary
                    } else {
                        theme::PaletteRole::Border
                    })))
                    .bg(rgb(theme::color(if selected {
                        theme::PaletteRole::Hover
                    } else {
                        theme::PaletteRole::Surface
                    })))
                    .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                    .child(theme::text("replace")),
            ),
    )
}

pub(in crate::editor) fn replace_pad_bounds(card: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds {
        origin: gpui::point(
            card.center().x - px(REPLACE_PAD_WIDTH) / 2.0,
            card.origin.y + px(REPLACE_PAD_TOP),
        ),
        size: gpui::size(px(REPLACE_PAD_WIDTH), px(REPLACE_PAD_HEIGHT)),
    }
}

pub(in crate::editor) fn is_track_vst3_replacement_leaf(
    track: &hex_project::Track,
    id: DeviceId,
) -> bool {
    if track.is_group() {
        return false;
    }
    let devices = track.devices();
    devices.iter().enumerate().any(|(index, device)| {
        device.id() == id
            && device.parent().is_none()
            && device.state().type_id() == hex_lattice_contract::VST3_DEVICE_TYPE_ID
            && devices
                .get(index + 1)
                .is_none_or(|next| next.parent() != Some(id))
    })
}

#[allow(clippy::too_many_arguments)]
pub(in crate::editor) fn resolve_device_card_drop_target(
    source: &drag::NativeDeviceDragSource,
    class: NativeDeviceClass,
    inside: bool,
    in_vst3_replace_pad: bool,
    in_left_half: bool,
    left_target: drag::NativeDeviceDropTarget,
    right_target: drag::NativeDeviceDropTarget,
    front_target: drag::NativeDeviceDropTarget,
    replacement_target: drag::NativeDeviceDropTarget,
) -> Option<drag::NativeDeviceDropTarget> {
    if !inside {
        return None;
    }
    if in_vst3_replace_pad && source.is_catalog_vst3_effect() {
        return Some(replacement_target);
    }
    if matches!(
        source,
        drag::NativeDeviceDragSource::Browser {
            class: NativeDeviceClass::Instrument,
            ..
        }
    ) || source.is_catalog_vst3_instrument()
    {
        return Some(if class == NativeDeviceClass::Instrument {
            replacement_target
        } else {
            front_target
        });
    }
    if source.is_catalog_vst3_effect() && class == NativeDeviceClass::Instrument {
        return Some(right_target);
    }
    Some(if in_left_half {
        left_target
    } else {
        right_target
    })
}

pub(in crate::editor) fn render_device_card_target(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    geometry: DeviceCardDropGeometry,
    rendered: Stateful<Div>,
) -> Stateful<Div> {
    let DeviceCardDropGeometry {
        track,
        device,
        class,
        width,
        next,
        first,
        visible_chain_bounds,
        is_sampler,
        replace_vst3,
    } = geometry;
    let active = view
        .active_native_device_drag
        .as_ref()
        .and_then(|drag| drag.destination.get());
    let active_left = active.is_some_and(|target| {
        target.replace.is_none() && target.track == track && target.before == Some(device)
    });
    let active_right = active.is_some_and(|target| {
        target.replace.is_none() && target.track == track && target.before == next
    });
    let left_target = drag::NativeDeviceDropTarget {
        track,
        before: Some(device),
        replace: None,
    };
    let right_target = drag::NativeDeviceDropTarget {
        track,
        before: next,
        replace: None,
    };
    let front_target = drag::NativeDeviceDropTarget {
        track,
        before: first,
        replace: None,
    };
    let replacement_target = drag::NativeDeviceDropTarget {
        track,
        before: None,
        replace: Some(device),
    };
    let active_replace = active == Some(replacement_target);
    let selector = format!("device-drop-card-{}", device.value());
    let entity = cx.entity();
    let target_visible_bounds = visible_chain_bounds.clone();
    let mut wrapper = div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .relative()
        .w(px(width + DEVICE_CHAIN_PADDING))
        .pr(px(DEVICE_CHAIN_PADDING))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex_shrink_0()
        .drag_over::<drag::NativeDeviceDrag>(move |style, _, _, _| {
            style.bg(rgb(theme::color(theme::PaletteRole::Hover)))
        })
        .on_drag_move::<drag::NativeDeviceDrag>(move |event, _, cx| {
            let Some(payload) = event
                .dragged_item()
                .downcast_ref::<drag::NativeDeviceDrag>()
            else {
                return;
            };
            let pointer = f32::from(event.event.position.x);
            let left = f32::from(event.bounds.origin.x);
            let right = left + f32::from(event.bounds.size.width);
            let pointer_y = f32::from(event.event.position.y);
            let top = f32::from(event.bounds.origin.y);
            let bottom = top + f32::from(event.bounds.size.height);
            let target_bounds = event.bounds;
            let inside = pointer >= left
                && pointer <= right
                && pointer_y >= top
                && pointer_y <= bottom
                && pointer_within_visible_device_drop_bounds(
                    &event.event.position,
                    &target_bounds,
                    target_visible_bounds.borrow().as_ref(),
                );
            let target = resolve_device_card_drop_target(
                &payload.source,
                class,
                inside,
                replace_vst3 && replace_pad_bounds(target_bounds).contains(&event.event.position),
                pointer < left + (right - left) / 2.0,
                left_target,
                right_target,
                front_target,
                replacement_target,
            );
            let previous = payload.destination.get();
            if let Some(target) = target.filter(|_| inside)
                && previous != Some(target)
            {
                payload.destination.set(Some(target));
                entity.update(cx, |_, cx| cx.notify());
            }
        })
        .on_drop(
            cx.listener(move |view, payload: &drag::NativeDeviceDrag, window, cx| {
                let fallback = if class == NativeDeviceClass::Instrument
                    && payload.source.is_catalog_vst3_instrument()
                {
                    replacement_target
                } else if class == NativeDeviceClass::Instrument
                    && payload.source.is_catalog_vst3_effect()
                {
                    right_target
                } else {
                    left_target
                };
                let target = payload.destination.get().unwrap_or(fallback);
                if payload.source.is_catalog_vst3_effect()
                    && target.replace.is_some()
                    && (!replace_vst3 || target != replacement_target)
                {
                    return;
                }
                view.drop_native_device(payload, target, window, cx);
            }),
        )
        .child(rendered)
        .child(insertion_gap(cx, device));
    if is_sampler {
        wrapper = wrapper
            .drag_over::<drag::BrowserAudioDrag>(|style, _, _, _| {
                style
                    .border_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Secondary)))
            })
            .on_drop(
                cx.listener(move |view, payload: &drag::BrowserAudioDrag, _, cx| {
                    view.drop_browser_sample_on_sampler(payload, track, device, cx);
                }),
            );
    }
    if active_left || view.device_panel.insertion_is(Some(device)) {
        wrapper = wrapper.child(
            div()
                .debug_selector(move || format!("device-insertion-before-{}", device.value()))
                .absolute()
                .left_0()
                .top_0()
                .w(px(2.0))
                .h(px(DEVICE_FRAME_HEIGHT))
                .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
        );
    }
    if active_right {
        wrapper = wrapper.child(
            div()
                .debug_selector(move || format!("device-insertion-after-{}", device.value()))
                .absolute()
                .right_0()
                .top_0()
                .w(px(2.0))
                .h(px(DEVICE_FRAME_HEIGHT))
                .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
        );
    }
    if replace_vst3 {
        wrapper = with_replace_pad(
            wrapper,
            move || format!("vst3-replace-{}", device.value()),
            active_replace,
        );
    } else if active_replace && class == NativeDeviceClass::Instrument {
        wrapper = wrapper.child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top_0()
                .bottom_0()
                .border_1()
                .border_color(rgb(theme::color(theme::PaletteRole::Secondary))),
        );
    }
    wrapper
}

/// Click target over the chain gap left of `before`'s card. It selects that
/// gap for paste and returns keys to the neutral rail.
pub(in crate::editor) fn insertion_gap(
    cx: &Context<EditorView>,
    before: DeviceId,
) -> Stateful<Div> {
    div()
        .id(ElementId::Name(
            format!("device-insert-{}", before.value()).into(),
        ))
        .debug_selector(move || format!("device-insert-{}", before.value()))
        .absolute()
        .top_0()
        .left(px(-DEVICE_CHAIN_PADDING))
        .w(px(DEVICE_CHAIN_PADDING))
        .h(px(DEVICE_FRAME_HEIGHT))
        .cursor(gpui::CursorStyle::IBeam)
        .on_mouse_down(MouseButton::Left, select_insertion(cx, Some(before)))
}

pub(in crate::editor) fn select_insertion(
    cx: &Context<EditorView>,
    before: Option<DeviceId>,
) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |view, _: &gpui::MouseDownEvent, window, cx| {
        view.device_panel.select_insertion(before);
        view.device_panel
            .chain_window
            .selection_focus(cx)
            .focus(window);
        cx.notify();
    })
}

pub(in crate::editor) fn render_device_end_target(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    target: drag::NativeDeviceDropTarget,
    first: Option<DeviceId>,
    visible_chain_bounds: std::rc::Rc<std::cell::RefCell<Option<Bounds<Pixels>>>>,
) -> Stateful<Div> {
    let instrument_target = drag::NativeDeviceDropTarget {
        track: target.track,
        before: first,
        replace: None,
    };
    let active = view.active_native_device_drag.as_ref().is_some_and(|drag| {
        let expected = match drag.source {
            drag::NativeDeviceDragSource::Browser {
                class: NativeDeviceClass::Instrument,
                ..
            } => instrument_target,
            _ => target,
        };
        drag.destination.get() == Some(expected)
    });
    let entity = cx.entity();
    let target_visible_bounds = visible_chain_bounds.clone();
    let selector = format!("device-drop-end-{}", target.track.value());
    let mut receiver = div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .relative()
        .flex_1()
        .min_w(px(40.0))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex_shrink_0()
        .on_mouse_down(MouseButton::Left, select_insertion(cx, None))
        .bg(rgb(if active {
            theme::color(theme::PaletteRole::Hover)
        } else {
            theme::color(theme::PaletteRole::Surface)
        }))
        .drag_over::<drag::NativeDeviceDrag>(move |style, _, _, _| {
            style.bg(rgb(theme::color(theme::PaletteRole::Hover)))
        })
        .on_drag_move::<drag::NativeDeviceDrag>(move |event, _, cx| {
            let Some(payload) = event
                .dragged_item()
                .downcast_ref::<drag::NativeDeviceDrag>()
            else {
                return;
            };
            let pointer = f32::from(event.event.position.x);
            let left = f32::from(event.bounds.origin.x);
            let right = left + f32::from(event.bounds.size.width);
            let pointer_y = f32::from(event.event.position.y);
            let top = f32::from(event.bounds.origin.y);
            let bottom = top + f32::from(event.bounds.size.height);
            let target_bounds = event.bounds;
            let inside = pointer >= left
                && pointer <= right
                && pointer_y >= top
                && pointer_y <= bottom
                && pointer_within_visible_device_drop_bounds(
                    &event.event.position,
                    &target_bounds,
                    target_visible_bounds.borrow().as_ref(),
                );
            let destination = match payload.source {
                drag::NativeDeviceDragSource::Browser {
                    class: NativeDeviceClass::Instrument,
                    ..
                } => Some(instrument_target),
                _ if inside => Some(target),
                _ => None,
            };
            let previous = payload.destination.get();
            if inside && previous != destination {
                payload.destination.set(destination);
                entity.update(cx, |_, cx| cx.notify());
            }
        })
        .on_drop(
            cx.listener(move |view, payload: &drag::NativeDeviceDrag, window, cx| {
                let target = payload.destination.get().unwrap_or(target);
                view.drop_native_device(payload, target, window, cx);
            }),
        );
    if active || view.device_panel.insertion_is(None) {
        receiver = receiver.child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w(px(2.0))
                .h(px(DEVICE_FRAME_HEIGHT))
                .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
        );
    }
    receiver
}

pub(in crate::editor) fn device_drag_title(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    device: &hex_project::Device,
    class: NativeDeviceClass,
    title: gpui::SharedString,
) -> Stateful<Div> {
    let id = device.id();
    let generation = view.project_generation;
    let owner = view.device_panel.owner;
    let payload = drag::NativeDeviceDrag {
        project_generation: view.project_generation,
        revision: view.state.as_ref().map_or(0, |state| state.revision),
        source: drag::NativeDeviceDragSource::Existing {
            device: id,
            class,
            type_id: device.state().type_id(),
        },
        destination: std::rc::Rc::new(std::cell::Cell::new(None)),
        return_destination: std::rc::Rc::new(std::cell::Cell::new(None)),
    };
    let entity = cx.entity();
    let help = theme::text(if class == NativeDeviceClass::Effect {
        format!("{title} · double-click to collapse or expand · drag to reorder · duplicate · d")
    } else {
        format!("{title} · double-click to collapse or expand · drag to reorder")
    });
    div()
        .id(("device-drag-title", id.value()))
        .debug_selector(move || format!("device-drag-title-{}", id.value()))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .truncate()
        .cursor(gpui::CursorStyle::OpenHand)
        .tooltip(move |_, cx| {
            let help = help.clone();
            cx.new(|_| crate::editor::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Help,
                description: help,
            })
            .into()
        })
        .child(hex_widgets::vertical_device_name(title))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, _, window, cx| {
                if view.project_generation == generation
                    && view.device_panel.selected_device == Some(id)
                {
                    view.device_panel.chain_window.focus(id, cx).focus(window);
                }
            }),
        )
        .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
            if event.click_count() == 2
                && event.modifiers() == gpui::Modifiers::default()
                && view.project_generation == generation
                && view.device_panel.owner == owner
            {
                fold::toggle_header(view, id, cx);
            }
        }))
        .on_drag(payload, move |payload, _, window, cx| {
            cx.set_active_drag_cursor_style(
                if window.modifiers().alt {
                    gpui::CursorStyle::DragCopy
                } else {
                    gpui::CursorStyle::ClosedHand
                },
                window,
            );
            entity.update(cx, |view, cx| {
                view.begin_native_device_drag(payload);
                cx.notify();
            });
            cx.new(|_| gpui::Empty)
        })
}

pub(in crate::editor) fn chain_ids(
    project: &hex_project::Project,
    owner: DevicePanelOwner,
) -> Vec<DeviceId> {
    match owner {
        DevicePanelOwner::Track(id) => project
            .track(id)
            .map(|track| {
                track
                    .ordered_device_chain()
                    .map(|node| match node {
                        hex_project::TrackDeviceRef::Instrument(item)
                        | hex_project::TrackDeviceRef::Effect(item) => item.id(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        DevicePanelOwner::Main => project
            .main_devices()
            .iter()
            .map(hex_project::Device::id)
            .collect(),
        DevicePanelOwner::Return(id) => project
            .return_tracks()
            .iter()
            .find(|track| track.id() == id)
            .map(|track| {
                track
                    .devices()
                    .iter()
                    .map(hex_project::Device::id)
                    .collect()
            })
            .unwrap_or_default(),
        DevicePanelOwner::Source(_) => Vec::new(),
    }
}

pub(in crate::editor) fn project_owner(
    owner: DevicePanelOwner,
) -> Option<hex_project::DeviceChainOwner> {
    match owner {
        DevicePanelOwner::Track(id) => Some(hex_project::DeviceChainOwner::Track(id)),
        DevicePanelOwner::Main => Some(hex_project::DeviceChainOwner::Main),
        DevicePanelOwner::Return(id) => Some(hex_project::DeviceChainOwner::Return(id)),
        DevicePanelOwner::Source(_) => None,
    }
}

// Capture selection before nested controls consume their pointer event. Selection
// is local view state and never finalizes or dispatches a parameter edit.
pub(in crate::editor) fn device_selection_listener(
    device: DeviceId,
    generation: u64,
    owner: Option<DevicePanelOwner>,
    cx: &Context<EditorView>,
) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static {
    cx.listener(move |view, event: &gpui::MouseDownEvent, _, cx| {
        if event.button != MouseButton::Left
            || view.project_generation != generation
            || view.device_panel.owner != owner
        {
            return;
        }
        if event.click_count == 2
            && event.modifiers == gpui::Modifiers::default()
            && fold::header_selection(view, device).is_some()
        {
            return;
        }
        let Some(state) = view.state.as_ref() else {
            return;
        };
        let Some(owner) = owner else {
            return;
        };
        let order = chain_ids(&state.project, owner);
        let members: Vec<_> = if view.device_panel.is_selected(device) {
            order
                .iter()
                .copied()
                .filter(|id| view.device_panel.is_selected(*id))
                .collect()
        } else {
            vec![device]
        };
        view.device_panel.drag_selection =
            hex_project::DeviceSelectionIds::new(&members)
                .ok()
                .map(|selected| DeviceDragSelection {
                    generation,
                    revision: state.revision,
                    owner,
                    device,
                    selected,
                });
        if event.modifiers.alt && view.device_panel.is_selected(device) {
            view.device_panel.cancel_duplicate_selection_update();
            if let Some(previous) = view.device_panel.selected_device {
                view.device_panel.selected_others.insert(previous);
            }
            view.device_panel.selected_others.remove(&device);
            view.device_panel.selected_device = Some(device);
        } else {
            view.device_panel
                .select_click(device, &order, event.modifiers);
        }
        cx.notify();
    })
}

pub(in crate::editor) fn duplicate_device_action(
    project: &hex_project::Project,
    owner: DevicePanelOwner,
    selected: hex_project::DeviceSelectionIds,
    destination: hex_project::DeviceChainOwner,
    before: Option<DeviceId>,
) -> Result<hex_session::service::DeviceClipboardAction, &'static str> {
    let copies_instrument = match owner {
        DevicePanelOwner::Track(id) => project
            .track(id)
            .and_then(|track| track.instrument())
            .is_some_and(|device| selected.ids().contains(&device.id())),
        _ => false,
    };
    if copies_instrument
        && !matches!(
            destination,
            hex_project::DeviceChainOwner::Track(id)
                if project.track(id).is_some_and(|track|
                    !track.is_group() && track.instrument().is_none())
        )
    {
        return Err("destination cannot accept another instrument");
    }
    Ok(hex_session::service::DeviceClipboardAction::Duplicate {
        source: project_owner(owner).ok_or("source view does not support device copying")?,
        selected,
        destination,
        before,
    })
}

impl EditorView {
    pub(in crate::editor) fn device_drag_selection(
        &self,
        drag: &drag::NativeDeviceDrag,
    ) -> Option<(DevicePanelOwner, &hex_project::DeviceSelectionIds)> {
        let drag::NativeDeviceDragSource::Existing { device, .. } = drag.source else {
            return None;
        };
        let snapshot = self.device_panel.drag_selection.as_ref()?;
        (snapshot.device == device
            && snapshot.generation == drag.project_generation
            && snapshot.revision == drag.revision)
            .then_some((snapshot.owner, &snapshot.selected))
    }

    pub(in crate::editor) fn duplicate_device_drag_request(
        &self,
        drag: &drag::NativeDeviceDrag,
        destination: hex_project::DeviceChainOwner,
        before: Option<DeviceId>,
    ) -> Result<Request, &'static str> {
        let (owner, selected) = self
            .device_drag_selection(drag)
            .ok_or("device copy canceled · selection changed")?;
        let project = &self
            .state
            .as_ref()
            .ok_or("device copy canceled · project changed")?
            .project;
        Ok(Request::DeviceClipboard {
            project_generation: drag.project_generation,
            revision: drag.revision,
            action: duplicate_device_action(project, owner, selected.clone(), destination, before)?,
        })
    }

    pub(in crate::editor) fn reorder_selected_devices_on_drop(
        &mut self,
        drag: &drag::NativeDeviceDrag,
        owner: DevicePanelOwner,
        before: Option<DeviceId>,
        cx: &mut Context<Self>,
    ) -> bool {
        let drag::NativeDeviceDragSource::Existing { device, .. } = drag.source else {
            return false;
        };
        let Some((source, selected)) = self.device_drag_selection(drag) else {
            return false;
        };
        if source != owner || selected.ids().len() < 2 {
            return false;
        }
        let selected = selected.clone();
        self.clear_native_device_drag(drag);
        let Some(project_owner) = project_owner(owner) else {
            return false;
        };
        if self.enqueue_device_barrier(Request::ReorderDevices {
            project_generation: drag.project_generation,
            revision: drag.revision,
            owner: project_owner,
            selected: selected.clone(),
            before,
        }) {
            self.device_panel.select_only(Some(device));
            self.device_panel
                .selected_others
                .extend(selected.ids().iter().copied().filter(|id| *id != device));
            self.error = None;
            self.device_panel.message = None;
            self.status = "moving devices".into();
        }
        cx.notify();
        true
    }

    pub(in crate::editor) fn drop_native_device_to_effect_bus(
        &mut self,
        drag: &drag::NativeDeviceDrag,
        target: drag::ReturnDeviceDropTarget,
        window: &mut Window,
        cx: &mut Context<EditorView>,
    ) {
        use drag::EffectBusDropSlot;
        let owner = target.owner;
        let before = match target.slot {
            EffectBusDropSlot::Before(before) => before,
            EffectBusDropSlot::Replace(_) => None,
        };
        cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
        let owns_drag = self
            .active_native_device_drag
            .as_ref()
            .is_some_and(|active| std::rc::Rc::ptr_eq(&active.destination, &drag.destination));
        let current = owns_drag
            && self.native_device_drag_is_current(drag)
            && self.state.as_ref().is_some_and(|state| {
                state.revision == drag.revision
                    && owner
                        .devices(&state.project)
                        .is_some_and(|devices| match target.slot {
                            EffectBusDropSlot::Before(before) => before.is_none_or(|anchor| {
                                devices.iter().any(|device| device.id() == anchor)
                            }),
                            EffectBusDropSlot::Replace(replacing) => {
                                drag.source.is_catalog_vst3_effect()
                                    && devices.iter().any(|device| {
                                        device.id() == replacing
                                            && device.parent().is_none()
                                            && device.state().type_id()
                                                == hex_lattice_contract::VST3_DEVICE_TYPE_ID
                                    })
                            }
                        })
            });
        if !current {
            self.error = Some("device drag canceled · project or source changed".into());
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        if self.export_pending
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || !self.device_gesture_barriers.is_empty()
            || self.device_gesture.snapshot().is_some()
            || self.device_panel.has_pending_draft(cx)
        {
            let _ = finalize_controls(self, cx);
            self.error = Some("device edit is still applying; drag again".into());
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        if !finalize_controls(self, cx) {
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        let panel_owner = match owner {
            EffectBusOwner::Main => DevicePanelOwner::Main,
            EffectBusOwner::Return(id) => DevicePanelOwner::Return(id),
        };
        if window.modifiers().alt
            && matches!(drag.source, drag::NativeDeviceDragSource::Existing { .. })
        {
            let Some(destination) = project_owner(panel_owner) else {
                return;
            };
            let request = self.duplicate_device_drag_request(drag, destination, before);
            self.clear_native_device_drag(drag);
            match request {
                Ok(request) => {
                    if self.enqueue_device_barrier(request) {
                        self.error = None;
                        self.status = "copying devices".into();
                    }
                }
                Err(reason) => self.error = Some(reason.into()),
            }
            cx.notify();
            return;
        }
        if self.reorder_selected_devices_on_drop(drag, panel_owner, before, cx) {
            return;
        }
        let request = match drag.source {
            drag::NativeDeviceDragSource::Browser {
                class: NativeDeviceClass::Effect,
                type_id,
            } => Ok(owner.insert_request(drag.project_generation, drag.revision, before, type_id)),
            drag::NativeDeviceDragSource::Browser {
                class: NativeDeviceClass::Instrument,
                ..
            } => Err("this effect chain accepts effects only".to_owned()),
            drag::NativeDeviceDragSource::PluginEffect(ref selection) => {
                use hex_session::service::PluginEffectDestination;
                let destination = match (owner, target.slot) {
                    (EffectBusOwner::Main, EffectBusDropSlot::Replace(replacing)) => {
                        PluginEffectDestination::ReplaceMainEffect { replacing }
                    }
                    (
                        EffectBusOwner::Return(return_track),
                        EffectBusDropSlot::Replace(replacing),
                    ) => PluginEffectDestination::ReplaceReturnEffect {
                        return_track,
                        replacing,
                    },
                    (EffectBusOwner::Main, _) => PluginEffectDestination::Main,
                    (EffectBusOwner::Return(id), _) => PluginEffectDestination::Return(id),
                };
                self.clear_native_device_drag(drag);
                self.insert_plugin_effect(selection, destination, before, cx);
                cx.notify();
                return;
            }
            drag::NativeDeviceDragSource::Existing { device, .. } => {
                let devices = self
                    .state
                    .as_ref()
                    .and_then(|state| owner.devices(&state.project));
                let owns_device = devices
                    .is_some_and(|devices| devices.iter().any(|effect| effect.id() == device));
                if owns_device {
                    let unchanged = devices.is_some_and(|devices| {
                        before == Some(device)
                            || devices
                                .iter()
                                .position(|entry| entry.id() == device)
                                .is_some_and(|index| {
                                    devices.get(index + 1).map(|next| next.id()) == before
                                })
                    });
                    if unchanged {
                        self.status = "device order unchanged".into();
                        self.clear_native_device_drag(drag);
                        cx.notify();
                        return;
                    }
                    Ok(Request::Edit(owner.reorder(device, before)))
                } else {
                    Err("devices can only be reordered within their current owner".to_owned())
                }
            }
        };
        self.clear_native_device_drag(drag);
        match request {
            Ok(request) => {
                if self.enqueue_catalog_device(request) {
                    self.error = None;
                    self.device_panel.message = None;
                    self.status = "updating effects".into();
                }
            }
            Err(reason) => {
                self.error = Some(reason.clone().into());
                self.device_panel.message = Some(reason);
            }
        }
        cx.notify();
    }

    pub(in crate::editor) fn drop_native_device(
        &mut self,
        drag: &drag::NativeDeviceDrag,
        drop_target: drag::NativeDeviceDropTarget,
        window: &mut Window,
        cx: &mut Context<EditorView>,
    ) {
        cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
        let owns_drag = self
            .active_native_device_drag
            .as_ref()
            .is_some_and(|active| {
                active.project_generation == drag.project_generation
                    && active.revision == drag.revision
                    && active.source == drag.source
                    && std::rc::Rc::ptr_eq(&active.destination, &drag.destination)
            });
        let stale = !owns_drag || !self.native_device_drag_is_current(drag);
        if stale {
            self.error = Some("device drag canceled · project or source changed".into());
            self.device_panel.message =
                Some("device drag canceled · project or source changed".into());
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }

        let pending_edit = self.opening_project
            || self.close_prompt_open
            || self.export_pending
            || self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || !self.device_gesture_barriers.is_empty()
            || self.device_gesture.snapshot().is_some()
            || self.device_panel.has_pending_draft(cx);
        if pending_edit {
            let _ = finalize_controls(self, cx);
            let message = "device edit is still applying; drag again";
            self.error = Some(message.into());
            self.device_panel.message = Some(message.into());
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        if !finalize_controls(self, cx) {
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }

        let still_current = self.native_device_drag_is_current(drag)
            && self.state.as_ref().is_some_and(|state| {
                state.revision == drag.revision && state.project.track(drop_target.track).is_some()
            });
        if !still_current {
            self.error = Some("device drag canceled · project or source changed".into());
            self.device_panel.message =
                Some("device drag canceled · project or source changed".into());
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }

        let Some(track) = self.selected_track else {
            self.error = Some("select a track before dropping a device".into());
            self.device_panel.message = self.error.as_ref().map(ToString::to_string);
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        };
        let Some(state) = self.state.as_ref() else {
            self.clear_native_device_drag(drag);
            return;
        };
        let Some(target_track) = state.project.track(track) else {
            self.error = Some("device drop target is no longer available".into());
            self.device_panel.message = self.error.as_ref().map(ToString::to_string);
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        };
        if let Some(anchor) = drop_target.before
            && !target_track.ordered_device_chain().any(|node| match node {
                hex_project::TrackDeviceRef::Instrument(device)
                | hex_project::TrackDeviceRef::Effect(device) => device.id() == anchor,
            })
        {
            self.error = Some("device drop target changed; drag again".into());
            self.device_panel.message = self.error.as_ref().map(ToString::to_string);
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }

        let is_instrument_drag = matches!(
            drag.source,
            drag::NativeDeviceDragSource::Browser {
                class: NativeDeviceClass::Instrument,
                ..
            }
        ) || drag.source.is_catalog_vst3_instrument();
        let before = if is_instrument_drag && drop_target.replace.is_none() {
            target_track
                .ordered_device_chain()
                .next()
                .map(|node| match node {
                    hex_project::TrackDeviceRef::Instrument(device)
                    | hex_project::TrackDeviceRef::Effect(device) => device.id(),
                })
        } else {
            drop_target.before
        };
        if is_instrument_drag
            && target_track.instrument().is_some()
            && drop_target.replace.is_none()
        {
            self.error = Some("instrument slot is already occupied".into());
            self.device_panel.message = self.error.as_ref().map(ToString::to_string);
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        if drag.source.is_catalog_vst3_instrument()
            && (target_track.is_group()
                || (target_track.instrument().is_some() && !target_track.receives_source_audio()))
        {
            self.error = Some("select an ordinary receiving track for the vst3 instrument".into());
            self.device_panel.message = self.error.as_ref().map(ToString::to_string);
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }
        if let Some(replacement) = drop_target.replace {
            let (valid_target, message) = if drag.source.is_catalog_vst3_effect() {
                (
                    track == drop_target.track
                        && is_track_vst3_replacement_leaf(target_track, replacement),
                    "effect replacement target changed · drag again",
                )
            } else {
                (
                    is_instrument_drag
                        && target_track
                            .instrument()
                            .is_some_and(|instrument| instrument.id() == replacement),
                    "instrument replacement target changed; drag again",
                )
            };
            if !valid_target || drop_target.before.is_some() {
                self.error = Some(message.into());
                self.device_panel.message = self.error.as_ref().map(ToString::to_string);
                self.clear_native_device_drag(drag);
                cx.notify();
                return;
            }
        }
        if window.modifiers().alt
            && matches!(drag.source, drag::NativeDeviceDragSource::Existing { .. })
        {
            let request = self.duplicate_device_drag_request(
                drag,
                hex_project::DeviceChainOwner::Track(track),
                before,
            );
            self.clear_native_device_drag(drag);
            match request {
                Ok(request) => {
                    if self.enqueue_device_barrier(request) {
                        self.error = None;
                        self.status = "copying devices".into();
                    }
                }
                Err(reason) => self.error = Some(reason.into()),
            }
            cx.notify();
            return;
        }
        if self.reorder_selected_devices_on_drop(drag, DevicePanelOwner::Track(track), before, cx) {
            return;
        }
        if let drag::NativeDeviceDragSource::Existing { device, .. } = drag.source
            && self.state.as_ref().is_some_and(|state| {
                device_move_would_be_noop(&state.project, device, track, before)
            })
        {
            self.status = "device order unchanged".into();
            self.clear_native_device_drag(drag);
            cx.notify();
            return;
        }

        let source = match drag.source {
            drag::NativeDeviceDragSource::Browser { class, type_id } => {
                NativeDeviceDropSource::Browser { class, type_id }
            }
            drag::NativeDeviceDragSource::PluginEffect(ref selection) => {
                use hex_plugin_catalog::PluginRole;
                use hex_session::service::PluginEffectDestination;
                let (destination, insert_before) = match (selection.role, drop_target.replace) {
                    (PluginRole::Instrument, Some(instrument)) => (
                        PluginEffectDestination::ReplaceInstrument {
                            track: drop_target.track,
                            instrument,
                        },
                        None,
                    ),
                    (PluginRole::Instrument, None) => {
                        (PluginEffectDestination::Instrument(drop_target.track), None)
                    }
                    (PluginRole::Effect, Some(replacing)) => (
                        PluginEffectDestination::ReplaceTrackEffect {
                            track: drop_target.track,
                            replacing,
                        },
                        None,
                    ),
                    (PluginRole::Effect, None) => {
                        (PluginEffectDestination::Track(drop_target.track), before)
                    }
                };
                self.clear_native_device_drag(drag);
                self.insert_plugin_effect(selection, destination, insert_before, cx);
                cx.notify();
                return;
            }
            drag::NativeDeviceDragSource::Existing {
                device,
                class,
                type_id,
            } => NativeDeviceDropSource::Existing {
                device,
                class,
                type_id,
            },
        };
        let request = Request::DropNativeDevice {
            project_generation: drag.project_generation,
            revision: drag.revision,
            source,
            track,
            before,
            replace: drop_target.replace,
            control: None,
        };
        self.clear_native_device_drag(drag);
        if self.enqueue_catalog_device(request) {
            self.error = None;
            self.device_panel.message = None;
            self.status = match source {
                NativeDeviceDropSource::Browser { .. } if drop_target.replace.is_some() => {
                    "replacing instrument".into()
                }
                NativeDeviceDropSource::Browser { .. } => "adding device".into(),
                NativeDeviceDropSource::Existing { .. } => "moving device".into(),
            };
        }
        cx.notify();
    }
}

pub(in crate::editor) fn device_move_would_be_noop(
    project: &hex_project::Project,
    device: DeviceId,
    target_track: TrackId,
    before: Option<DeviceId>,
) -> bool {
    let Some(source_track) = project.tracks().iter().find(|track| {
        track.ordered_device_chain().any(|node| match node {
            hex_project::TrackDeviceRef::Instrument(candidate)
            | hex_project::TrackDeviceRef::Effect(candidate) => candidate.id() == device,
        })
    }) else {
        return false;
    };
    if source_track.id() != target_track {
        return false;
    }
    if before == Some(device) {
        return true;
    }
    let current: Vec<_> = source_track
        .ordered_device_chain()
        .map(|node| match node {
            hex_project::TrackDeviceRef::Instrument(candidate)
            | hex_project::TrackDeviceRef::Effect(candidate) => candidate.id(),
        })
        .collect();
    let mut moved: Vec<_> = current
        .iter()
        .copied()
        .filter(|candidate| *candidate != device)
        .collect();
    let destination = before
        .and_then(|anchor| moved.iter().position(|candidate| *candidate == anchor))
        .unwrap_or(moved.len());
    moved.insert(destination, device);
    moved == current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_pad_bounds_stays_within_device_card_top_right() {
        let card = Bounds::from_corners(
            gpui::point(px(100.0), px(40.0)),
            gpui::point(px(300.0), px(220.0)),
        );
        let pad = replace_pad_bounds(card);
        assert_eq!(pad.size.width, px(REPLACE_PAD_WIDTH));
        assert_eq!(pad.size.height, px(REPLACE_PAD_HEIGHT));
        assert!(pad.origin.x >= card.origin.x);
        assert!(pad.origin.y >= card.origin.y);
        assert!(pad.right() <= card.right());
    }

    #[test]
    fn device_card_drop_target_distinguishes_vst3_instrument_replacement_and_effect_insertion() {
        use hex_project::{Command, Created, Editor, Project};

        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "midi".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let sine = hex_engine::default_native_device_state_for_class(
            NativeDeviceClass::Instrument,
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sine"),
        )
        .unwrap();
        let utility = hex_engine::default_native_device_state(
            hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        let Created::Device(instrument) = editor
            .execute(Command::InsertInstrument { track, state: sine })
            .unwrap()
        else {
            panic!("instrument");
        };
        let Created::Device(pre_effect) = editor
            .execute(Command::InsertDevice {
                track,
                before: Some(instrument),
                state: utility,
            })
            .unwrap()
        else {
            panic!("pre effect");
        };

        let selection = |role: hex_plugin_catalog::PluginRole| {
            drag::NativeDeviceDragSource::PluginEffect(std::sync::Arc::new(
                hex_plugin_catalog::PluginSelection {
                    module: std::path::PathBuf::from("/tmp/test.vst3"),
                    executable_relative_path: "Contents/MacOS/test".into(),
                    cid: [0x11; 16],
                    class_name: "test plugin".into(),
                    role,
                    executable_sha256: "a".repeat(64),
                    info_sha256: "b".repeat(64),
                },
            ))
        };
        let vst3_instrument = selection(hex_plugin_catalog::PluginRole::Instrument);
        let vst3_effect = selection(hex_plugin_catalog::PluginRole::Effect);

        let left_inst = drag::NativeDeviceDropTarget {
            track,
            before: Some(instrument),
            replace: None,
        };
        let right_inst = drag::NativeDeviceDropTarget {
            track,
            before: None,
            replace: None,
        };
        let front = drag::NativeDeviceDropTarget {
            track,
            before: Some(pre_effect),
            replace: None,
        };
        let replace_inst = drag::NativeDeviceDropTarget {
            track,
            before: None,
            replace: Some(instrument),
        };

        // Dragging a VST3 instrument onto an existing instrument card targets replacement,
        // never post-instrument insertion.
        assert_eq!(
            resolve_device_card_drop_target(
                &vst3_instrument,
                NativeDeviceClass::Instrument,
                true,
                false,
                false,
                left_inst,
                right_inst,
                front,
                replace_inst,
            ),
            Some(replace_inst)
        );
        // Dragging a VST3 instrument onto an effect card targets front_target (matching native instruments).
        assert_eq!(
            resolve_device_card_drop_target(
                &vst3_instrument,
                NativeDeviceClass::Effect,
                true,
                false,
                false,
                left_inst,
                right_inst,
                front,
                replace_inst,
            ),
            Some(front)
        );
        // Dragging a VST3 effect onto an instrument card targets post-instrument right_target.
        assert_eq!(
            resolve_device_card_drop_target(
                &vst3_effect,
                NativeDeviceClass::Instrument,
                true,
                false,
                true,
                left_inst,
                right_inst,
                front,
                replace_inst,
            ),
            Some(right_inst)
        );
        // Dragging a VST3 effect onto the right half of pre_effect targets before == Some(instrument).
        let left_pre = drag::NativeDeviceDropTarget {
            track,
            before: Some(pre_effect),
            replace: None,
        };
        let right_pre = drag::NativeDeviceDropTarget {
            track,
            before: Some(instrument),
            replace: None,
        };
        let replace_pre = drag::NativeDeviceDropTarget {
            track,
            before: None,
            replace: Some(pre_effect),
        };
        assert_eq!(
            resolve_device_card_drop_target(
                &vst3_effect,
                NativeDeviceClass::Effect,
                true,
                false,
                false,
                left_pre,
                right_pre,
                front,
                replace_pre,
            ),
            Some(right_pre)
        );
    }
}
