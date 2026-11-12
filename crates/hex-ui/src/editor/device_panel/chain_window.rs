//! Window only composition, retaining every live control and its subscriptions.
use super::*;
use gpui::{FocusHandle, ScrollHandle, point};
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
pub(super) struct ChainWindow {
    pub(super) scroll: ScrollHandle,
    cards: Vec<(DeviceId, f32, f32)>,
    focus: HashMap<DeviceId, FocusHandle>,
    selection_focus: Option<FocusHandle>,
    reveal_after_prepare: Option<DeviceId>,
    pub(super) refused_delete: Option<super::keyboard::RefusedDelete>,
    width: Rc<Cell<f32>>,
}

impl ChainWindow {
    pub(super) fn prepare(&mut self, devices: &[(DeviceId, f32)]) {
        self.prepare_with_prefix(devices, 0.0);
    }

    pub(super) fn prepare_with_prefix(&mut self, devices: &[(DeviceId, f32)], prefix: f32) {
        let mut left = DEVICE_CHAIN_PADDING + prefix;
        let cards: Vec<_> = devices
            .iter()
            .map(|(id, width)| {
                let card = (*id, left, *width);
                left += width + DEVICE_CHAIN_PADDING;
                card
            })
            .collect();
        if self.cards != cards {
            // Keep the leftmost surviving visible card at its prior screen
            // position when topology changes. Offsets are clamped by GPUI.
            let offset = f32::from(self.scroll.offset().x);
            let anchor = self.cards.iter().find(|(id, start, width)| {
                start + width + offset > 0.0 && cards.iter().any(|card| card.0 == *id)
            });
            if let Some((id, old, _)) = anchor
                && let Some((_, new, _)) = cards.iter().find(|card| card.0 == *id)
            {
                self.scroll
                    .set_offset(point(px(offset + old - new), px(0.0)));
            }
            self.focus
                .retain(|id, _| devices.iter().any(|device| device.0 == *id));
            self.cards = cards;
        }
        if let Some(device) = self.reveal_after_prepare.take() {
            self.reveal(device);
        }
    }

    pub(super) fn reveal_after_prepare(&mut self, device: DeviceId) {
        self.reveal_after_prepare = Some(device);
    }

    pub(super) fn focus(&mut self, device: DeviceId, cx: &App) -> FocusHandle {
        self.focus
            .entry(device)
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    pub(super) fn selection_focus(&mut self, cx: &App) -> FocusHandle {
        self.selection_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone()
    }

    pub(super) fn device_is_focused(&self, device: DeviceId, window: &Window) -> bool {
        self.focus
            .get(&device)
            .is_some_and(|focus| focus.is_focused(window))
    }

    /// Scroll the least distance that shows `device` with its gutters, or its
    /// start when wider than the viewport. Unknown geometry leaves it alone.
    pub(super) fn reveal(&self, device: DeviceId) {
        let Some(&(_, left, width)) = self.cards.iter().find(|card| card.0 == device) else {
            return;
        };
        if let Some(offset) = reveal_offset(
            left,
            width,
            f32::from(self.scroll.offset().x),
            self.width.get(),
        ) {
            self.scroll.set_offset(point(px(offset), px(0.0)));
        }
    }

    fn visible(&self, device: DeviceId, initial_width: Option<f32>) -> bool {
        let measured = self.width.get();
        let width = if measured.is_finite() && measured > 0.0 {
            measured
        } else {
            initial_width
                .filter(|width| width.is_finite() && *width > 0.0)
                .unwrap_or(0.0)
        };
        if width <= 0.0 || !width.is_finite() {
            return true;
        }
        let offset = f32::from(self.scroll.offset().x)
            .clamp(-f32::from(self.scroll.max_offset().width), 0.0);
        let overscan = self.cards.iter().map(|card| card.2).fold(0.0_f32, f32::max);
        self.cards
            .iter()
            .find(|card| card.0 == device)
            .is_none_or(|(_, left, card_width)| {
                left + card_width + offset >= -overscan && left + offset <= width + overscan
            })
    }
}

fn reveal_offset(left: f32, width: f32, offset: f32, viewport: f32) -> Option<f32> {
    if ![left, width, offset, viewport]
        .iter()
        .all(|value| value.is_finite())
        || viewport <= 0.0
    {
        return None;
    }
    let start = left - DEVICE_CHAIN_PADDING;
    let end = left + width + DEVICE_CHAIN_PADDING;
    let target = if start + offset < 0.0 {
        -start
    } else if end + offset > viewport {
        (viewport - end).max(-start)
    } else {
        return None;
    };
    Some(target.min(0.0))
}

pub(super) fn must_mount(
    view: &EditorView,
    device: DeviceId,
    window: Option<&Window>,
    cx: &App,
) -> bool {
    let panel = &view.device_panel;
    // Callers without a painted window cannot establish focus containment.
    // Preserve the entire chain in that case rather than risk a focused draft.
    let Some(window) = window else {
        return true;
    };
    panel.chain_window.visible(device, Some(f32::from(window.viewport_size().width)))
        || panel.chain_window.focus.get(&device).is_some_and(|focus| focus.contains_focused(window, cx))
        || panel.controls.iter().any(|(key, control)| key.device == device && matches!(control,
            ParameterControl::Float { entity, .. } if entity.read_with(cx, |control, _| control.is_interacting() || control.is_editing())))
        || panel.controls.keys().any(|key| key.device == device && super::parameter_rows::pinned(view, *key, window, cx))
        || panel.pending_values.keys().any(|key| key.device == device)
        || view.device_gesture.snapshot().is_some_and(|snapshot| matches!(snapshot.target,
            GestureTarget::DeviceParameter { device: target, .. } | GestureTarget::DeviceParameterPair { device: target, .. } | GestureTarget::DeviceParameterSnapshot { device: target } | GestureTarget::EqNodeCreation { device: target } if target == device))
        || view.active_native_device_drag.as_ref().is_some_and(|drag| {
            matches!(drag.source, drag::NativeDeviceDragSource::Existing { device: source, .. } if source == device)
                || drag.destination.get().is_some_and(|target| target.before == Some(device) || target.replace == Some(device))
                || drag.return_destination.get().is_some_and(|target| target.before == Some(device))
        })
        || panel.eq8_panels.get(&device).is_some_and(|panel| panel.audition_gesture_active)
}

pub(super) fn spacer(device: DeviceId, width: f32) -> Stateful<Div> {
    div()
        .id(ElementId::Name(
            format!("device-spacer-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-spacer-{}", device.value()))
        .w(px(width))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex_shrink_0()
}

pub(super) fn rail(view: &mut EditorView, cx: &Context<EditorView>) -> Div {
    super::navigation::sync_rail_scroll(view);
    let width = view.device_panel.chain_window.width.clone();
    let entity = cx.entity();
    let scroll = view.device_panel.chain_window.scroll.clone();
    let expected_offset = scroll.offset();
    // Observe the already-laid-out rail without introducing a child that
    // could change its scroll extent, padding, order or drop geometry.
    // The shared track/Return/Main rail is recessed behind neutral device panels.
    let rail = div()
        .bg(rgb(theme::color(theme::PaletteRole::Border)))
        .on_children_prepainted(move |_, _, cx| {
            let measured = f32::from(scroll.bounds().size.width);
            let resized =
                measured.is_finite() && measured > 0.0 && width.replace(measured) != measured;
            if resized || scroll.offset() != expected_offset {
                let entity = entity.clone();
                cx.defer(move |cx| entity.update(cx, |_, cx| cx.notify()));
            }
        });
    super::keyboard::rail(view, cx, rail)
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;

    #[test]
    fn chain_window_geometry_keeps_variable_width_order_and_surviving_anchor() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        let hex_project::Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "geometry".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        for _ in 0..4 {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: hex_engine::default_native_device_state(
                        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                    )
                    .unwrap(),
                })
                .unwrap();
        }
        let ids: Vec<_> = editor
            .project()
            .track(track)
            .unwrap()
            .devices()
            .iter()
            .map(|device| device.id())
            .collect();
        let mut chain = ChainWindow::default();
        chain.prepare(&[(ids[0], 144.0), (ids[1], 600.0), (ids[2], 286.0)]);
        assert_eq!(
            chain.cards,
            vec![
                (ids[0], 3.0, 144.0),
                (ids[1], 150.0, 600.0),
                (ids[2], 753.0, 286.0)
            ]
        );
        // Topology anchoring reads the requested offset even before a draw;
        // preparation does not apply native clamping itself.
        chain.scroll.set_offset(point(px(-160.0), px(0.0)));
        chain.prepare(&[
            (ids[3], 144.0),
            (ids[0], 144.0),
            (ids[1], 600.0),
            (ids[2], 286.0),
        ]);
        assert_eq!(f32::from(chain.scroll.offset().x), -307.0);
        chain.prepare(&[(ids[3], 144.0), (ids[1], 600.0), (ids[2], 286.0)]);
        assert_eq!(f32::from(chain.scroll.offset().x), -160.0);
        chain.prepare_with_prefix(&[(ids[3], 144.0), (ids[1], 600.0), (ids[2], 286.0)], 131.0);
        assert_eq!(chain.cards[0].1, 134.0);
        assert_eq!(f32::from(chain.scroll.offset().x), -291.0);
        chain.width.set(100.0);
        assert!(chain.visible(ids[0], None), "unknown IDs are conservative");
        assert!(!chain.visible(ids[2], None));
        chain.width.set(1600.0);
        assert!(
            chain.visible(ids[2], None),
            "resizing expands the composed range"
        );
        chain.width.set(0.0);
        assert!(
            !chain.visible(ids[2], Some(100.0)),
            "finite window bound limits first paint"
        );
        for unknown in [
            None,
            Some(0.0),
            Some(-1.0),
            Some(f32::NAN),
            Some(f32::INFINITY),
        ] {
            assert!(
                chain.visible(ids[2], unknown),
                "unknown bounds preserve conservative composition"
            );
        }
        chain.width.set(f32::NAN);
        assert!(
            chain.visible(ids[2], None),
            "invalid viewport cannot hide a card"
        );
    }

    #[test]
    fn reveal_scrolls_the_least_distance_to_show_a_card() {
        // Card 150..750 in a 400 px viewport.
        assert_eq!(
            reveal_offset(150.0, 600.0, 0.0, 400.0),
            Some(-147.0),
            "too wide: show start"
        );
        assert_eq!(
            reveal_offset(150.0, 200.0, 0.0, 400.0),
            None,
            "already visible"
        );
        assert_eq!(
            reveal_offset(150.0, 300.0, 0.0, 400.0),
            Some(-53.0),
            "right edge"
        );
        assert_eq!(
            reveal_offset(150.0, 200.0, -300.0, 400.0),
            Some(-147.0),
            "left edge"
        );
        assert_eq!(
            reveal_offset(3.0, 100.0, -50.0, 400.0),
            Some(0.0),
            "first card"
        );
        for viewport in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(reveal_offset(150.0, 300.0, 0.0, viewport), None);
        }
    }
}
