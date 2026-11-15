//! Native Vector working sections and owner-scoped routing/response view state.
//! Every sound control binds a real provider parameter; illustrative local-only
//! values belong to Frame studies, never the native working surface.
use super::super::EditorView;
use gpui::{Context, ElementId, Stateful, div, prelude::*, px};
use hex_lattice_contract::ParameterValue;

pub(super) const PANEL_WIDTH: f32 = crate::device_composition::VECTOR_PREVIEW_WIDTH;
const ROW_HEIGHT: f32 = 40.0;

#[derive(Default)]
pub(in crate::editor::device_panel) struct State {
    pub(super) filter_response: Option<super::filter_response::Control>,
    pub(in crate::editor::device_panel) routing_focus: Option<gpui::FocusHandle>,
    pub(in crate::editor::device_panel) routing_popup: Option<usize>,
    pub(super) routing_blur: Option<gpui::Subscription>,
}

pub(super) fn render(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: hex_project::DeviceId,
    device_view: &hex_engine::NativeDeviceView,
) -> Stateful<gpui::Div> {
    let rows = [
        super::lfo_summary(view, cx, device, device_view),
        super::filter_summary(view, cx, device, device_view),
        super::oscillator_options::feedback(view, cx, device, device_view),
        routing_row(
            view,
            cx,
            device,
            device_view
                .parameters
                .iter()
                .find(|parameter| parameter.descriptor.id == super::id(600))
                .map(|parameter| parameter.value),
        ),
        super::global::voices(view, cx, device, device_view),
    ];
    let mut content = hex_widgets::device_rail(super::super::device_layout_style())
        .w_full()
        .h_full();
    for row in rows {
        content = content.child(row);
    }
    hex_widgets::device_rail(super::super::device_layout_style())
        .id(ElementId::Name(
            format!("operator-preview-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-preview-{}", device.value()))
        .w(px(PANEL_WIDTH))
        .flex_shrink_0()
        .h_full()
        .pl(px(2.0))
        .child(content)
}

fn routing_row(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: hex_project::DeviceId,
    routing: Option<ParameterValue>,
) -> Stateful<gpui::Div> {
    let owner = super::context::Owner::capture(view, device);
    let section = super::Section::Preview("global");
    let (focus, focused) = super::context::section_focus(view, cx, device, section);
    let control = super::routing_picker::render(view, cx, device, routing);
    hex_widgets::device_grouped_row(super::super::device_layout_style(), focused)
        .on_children_prepainted(super::context::section_listener(view, cx, device, section))
        .id(ElementId::Name(
            format!("operator-preview-row-{}-global", device.value()).into(),
        ))
        .debug_selector(move || format!("operator-preview-row-{}-global", device.value()))
        .track_focus(&focus)
        .w_full()
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .child(
            div()
                .w(px(44.0))
                .flex_shrink_0()
                .h_full()
                .flex()
                .items_center()
                .child(control),
        )
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |view, _, _, cx| {
                if let Some(owner) = owner {
                    super::context::select_routing(view, cx, owner);
                }
            }),
        )
}

pub(in crate::editor::device_panel) fn retire_removed(
    view: &mut EditorView,
    _cx: &mut Context<EditorView>,
    alive: &std::collections::HashSet<hex_project::DeviceId>,
) {
    view.device_panel
        .operator_previews
        .retain(|device, _| alive.contains(device));
}

/// Retain owner-scoped focus/cache state, but retire a hidden routing popup.
pub(in crate::editor::device_panel) fn cancel_interactions(
    view: &mut EditorView,
    _cx: &mut Context<EditorView>,
) {
    for state in view.device_panel.operator_previews.values_mut() {
        state.routing_popup = None;
    }
}

/// A retained source can leave the displayed track without being deleted.
/// Its owner state survives, while hidden popup input and display are retired.
pub(in crate::editor::device_panel) fn cancel_hidden(
    view: &mut EditorView,
    visible: &std::collections::HashSet<hex_project::DeviceId>,
) {
    for (device, state) in &mut view.device_panel.operator_previews {
        if !visible.contains(device) {
            state.routing_popup = None;
            state.filter_response = None;
        }
    }
}

pub(in crate::editor::device_panel) fn cancel_all(
    view: &mut EditorView,
    _cx: &mut Context<EditorView>,
) {
    view.device_panel.operator_previews.clear();
}
