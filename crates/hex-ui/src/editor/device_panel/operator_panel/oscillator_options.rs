//! Rare oscillator controls in one optional native panel; no sound state is local.
use super::*;
use gpui::MouseButton;

pub(super) fn button(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    open: bool,
) -> Stateful<Div> {
    let owner = context::Owner::capture(view, device);
    let focus = navigation_focus(view, cx, device, NavigationTarget::Options);
    hex_widgets::panel_tab(theme::text("osc options"), open, display_style())
        .id(ElementId::Name(
            format!("operator-options-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-options-{}", device.value()))
        .w_full()
        .h(px(14.0))
        .flex_shrink_0()
        .track_focus(&focus)
        .key_context("hex_operator_options")
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, _, window, cx| {
                cx.stop_propagation();
                window.prevent_default();
                if owner.is_some_and(|owner| context::toggle_options(view, cx, owner)) {
                    focus.focus(window);
                }
            }),
        )
        .on_action(cx.trace(move |view, _: &ToggleOptions, _, cx| {
            if let Some(owner) = owner {
                context::toggle_options(view, cx, owner);
            }
            cx.stop_propagation();
        }))
}

pub(super) fn render(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    parameters: &[NativeDeviceParameterView],
    operator: usize,
) -> Div {
    use super::super::numeric_display::NumericDisplayCodec as Codec;
    let fixed = 62 + operator as u32 * 3;
    let companion = 357 + operator as u32 * 8;
    let mut content = hex_widgets::device_rail(super::super::device_layout_style())
        .debug_selector(move || format!("operator-options-panel-{}", device.value()))
        .w(px(crate::device_composition::VECTOR_OPTIONS_WIDTH))
        .h_full()
        .flex_shrink_0()
        .p(px(2.0))
        .gap(px(4.0));
    for (parameter, label, presentation, codec) in [
        (fixed, "fixed", ControlPresentation::CompactButton, None),
        (
            fixed + 1,
            "base",
            ControlPresentation::InlineValue,
            Some(Codec::Hertz),
        ),
        (fixed + 2, "multi", ControlPresentation::InlineValue, None),
        (
            operator as u32 * 3 + 3,
            "phase",
            ControlPresentation::InlineValue,
            Some(Codec::Percent),
        ),
        (
            companion,
            "retrigger",
            ControlPresentation::CompactButton,
            None,
        ),
        (
            companion + 1,
            "osc<vel",
            ControlPresentation::InlineValue,
            Some(Codec::Percent),
        ),
        (
            companion + 2,
            "quantize",
            ControlPresentation::CompactButton,
            None,
        ),
        (
            353 + operator as u32,
            "repeat",
            ControlPresentation::CompactSelector,
            None,
        ),
    ] {
        let control = compact_control_with_codec(
            view,
            cx,
            device,
            identity,
            parameters,
            id(parameter),
            presentation,
            Some(""),
            codec,
        );
        let control = control.w_full().h(px(12.0));
        content = content.child(
            div()
                .w_full()
                .h(px(20.0))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(detail_caption(label).h(px(8.0)))
                .child(control),
        );
    }
    content
}

pub(super) fn feedback(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let operator = view
        .device_panel
        .operator_pages
        .get(&device)
        .copied()
        .unwrap_or_default()
        .selected_operator;
    // A receiving FM operator cannot self-feed. Keep the primary feedback
    // control useful without bypassing the provider's availability contract.
    let eligible = |operator: usize| {
        device_view.parameters.iter().any(|parameter| {
            parameter.descriptor.id == id(78 + operator as u32) && parameter.enabled
        })
    };
    let operator = if eligible(operator) {
        operator
    } else {
        (0..4)
            .find(|operator| eligible(*operator))
            .unwrap_or(operator)
    };
    let section = Section::Preview("feedback");
    let (focus, focused) = context::section_focus(view, cx, device, section);
    let control = compact_control_with_codec(
        view,
        cx,
        device,
        DeviceControlIdentity {
            class: NativeDeviceClass::Instrument,
            type_id: device_view.type_id,
        },
        &device_view.parameters,
        id(78 + operator as u32),
        ControlPresentation::FilledSlider,
        Some("feedback"),
        Some(super::super::numeric_display::NumericDisplayCodec::Percent),
    );
    let control = control.flex_1().min_w_0();
    hex_widgets::device_grouped_row(super::super::device_layout_style(), focused)
        .on_children_prepainted(context::section_listener(view, cx, device, section))
        .id(ElementId::Name(
            format!("operator-preview-row-{}-feedback", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-preview-row-{}-feedback", device.value()))
        .track_focus(&focus)
        .w_full()
        .h(px(40.0))
        .flex()
        .items_center()
        .gap(px(2.0))
        .child(detail_caption(["a fb", "b fb", "c fb", "d fb"][operator]).w(px(44.0)))
        .child(control)
}
