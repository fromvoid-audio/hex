//! Return-chain insertion geometry; canonical edits remain in the session worker.
use super::*;
use drag::{NativeDeviceDrag, ReturnDeviceDropTarget};
use std::{cell::RefCell, rc::Rc};

pub(super) fn render_chain(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    owner: EffectBusOwner,
    devices: &[hex_project::Device],
) -> Stateful<Div> {
    let visible_bounds = Rc::new(RefCell::new(None));
    let receiver_bounds = visible_bounds.clone();
    let entity = cx.entity();
    let geometry: Vec<_> = devices
        .iter()
        .map(|device| {
            (
                device.id(),
                device_card_width(NativeDeviceClass::Effect, device.state().type_id()),
            )
        })
        .collect();
    super::navigation::sync_rail_scroll(view);
    view.device_panel.chain_window.prepare(&geometry);
    let spectrum_identities: Vec<_> = devices
        .iter()
        .filter(|device| {
            eq_panel::is_spectrum_eq(device.state().type_id())
                && chain_window::must_mount(view, device.id(), window, cx)
        })
        .map(|device| {
            hex_engine::PostNodeIdentity::new(
                owner.processor(),
                device.id().value(),
                device.state().type_id(),
            )
        })
        .collect();
    eq_panel::sync_visible_spectra(view, &spectrum_identities);
    let mut chain = chain_window::rail(view, cx)
        .id("device-chain-scroll")
        .track_scroll(&view.device_panel.chain_window.scroll)
        .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
        .debug_selector(|| "device-chain-scroll".into())
        .flex_1()
        .min_h_0()
        .flex()
        .p(px(DEVICE_CHAIN_PADDING))
        .overflow_x_scroll()
        .overflow_y_hidden()
        .on_drag_move::<NativeDeviceDrag>(move |event, _, cx| {
            let Some(payload) = event.dragged_item().downcast_ref::<NativeDeviceDrag>() else {
                return;
            };
            *receiver_bounds.borrow_mut() = Some(event.bounds);
            if event.bounds.contains(&event.event.position) {
                payload.destination.set(None);
            } else if payload.return_destination.take().is_some() {
                entity.update(cx, |_, cx| cx.notify());
            }
        });
    for (index, device) in devices.iter().enumerate() {
        if chain_window::must_mount(view, device.id(), window, cx) {
            let focus = view.device_panel.chain_window.focus(device.id(), cx);
            let rendered =
                super::return_view::render_return_effect(view, window, cx, owner, device)
                    .track_focus(&focus);
            let rendered = keyboard::card(view, cx, device.id(), focus.clone(), rendered);
            let rendered = super::super::device_comparison::card(
                view,
                cx,
                device.id(),
                NativeDeviceClass::Effect,
                focus,
                rendered,
            );
            chain = chain.child(render_return_drop_target(
                view,
                cx,
                owner,
                Some(device.id()),
                devices.get(index + 1).map(|next| next.id()),
                visible_bounds.clone(),
                rendered,
            ));
        } else {
            let card = chain_window::spacer(
                device.id(),
                device_card_width(NativeDeviceClass::Effect, device.state().type_id()),
            )
            .mr(px(DEVICE_CHAIN_PADDING));
            chain = chain.child(card);
        }
    }
    chain.child(render_return_drop_target(
        view,
        cx,
        owner,
        None,
        None,
        visible_bounds,
        div()
            .min_w(px(40.0))
            .h(px(DEVICE_FRAME_HEIGHT))
            .flex_1()
            .id("return-chain-tail"),
    ))
}

fn render_return_drop_target(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    owner: EffectBusOwner,
    before: Option<DeviceId>,
    after: Option<DeviceId>,
    visible_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    rendered: Stateful<Div>,
) -> Stateful<Div> {
    let left_target = ReturnDeviceDropTarget { owner, before };
    let right_target = ReturnDeviceDropTarget {
        owner,
        before: after,
    };
    let active = view
        .active_native_device_drag
        .as_ref()
        .and_then(|drag| drag.return_destination.get());
    let selector = before.map_or_else(
        || owner.end_selector(),
        |device| format!("{}-device-drop-card-{}", owner.prefix(), device.value()),
    );
    let entity = cx.entity();
    let mut target = div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .relative()
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex_shrink_0()
        .when(before.is_some(), |style| style.pr(px(DEVICE_CHAIN_PADDING)))
        .when(before.is_none(), |style| style.flex_1().min_w(px(40.0)))
        .when(before.is_none(), |style| {
            style.on_mouse_down(MouseButton::Left, select_insertion(cx, None))
        })
        .on_drag_move::<NativeDeviceDrag>(move |event, _, cx| {
            let Some(payload) = event.dragged_item().downcast_ref::<NativeDeviceDrag>() else {
                return;
            };
            if !pointer_within_visible_device_drop_bounds(
                &event.event.position,
                &event.bounds,
                visible_bounds.borrow().as_ref(),
            ) {
                return;
            }
            let target = if before.is_none() || event.event.position.x < event.bounds.center().x {
                left_target
            } else {
                right_target
            };
            if payload.return_destination.get() != Some(target) {
                payload.return_destination.set(Some(target));
                entity.update(cx, |_, cx| cx.notify());
            }
        })
        .on_drop(
            cx.listener(move |view, payload: &NativeDeviceDrag, window, cx| {
                let Some(target) = payload.return_destination.get().filter(|target| {
                    target.owner == owner && (*target == left_target || *target == right_target)
                }) else {
                    return;
                };
                view.drop_native_device_to_effect_bus(payload, owner, target.before, window, cx);
            }),
        )
        .child(rendered)
        .when_some(before, |style, device| {
            style.child(insertion_gap(cx, device))
        });
    let inserting = view.device_panel.insertion_is(before);
    for (edge, expected) in [(false, left_target), (true, right_target)] {
        if (active == Some(expected) || (!edge && inserting)) && (!edge || before.is_some()) {
            target = target.child(
                div()
                    .absolute()
                    .top_0()
                    .when(edge, |style| style.right_0())
                    .when(!edge, |style| style.left_0())
                    .w(px(2.0))
                    .h(px(DEVICE_FRAME_HEIGHT))
                    .bg(rgb(theme::color(theme::PaletteRole::Secondary))),
            );
        }
    }
    target
}
