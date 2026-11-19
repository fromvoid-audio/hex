//! Red accents for canonical device automation with authored points.
use super::{DeviceId, EditorView, NativeDeviceClass, theme};
use gpui::{Div, div, prelude::*, px, rgb};
use hex_lattice_contract::{DeviceAutomationTarget, ParameterId};
use hex_project::{Device, DeviceAutomation, ElementColor};
use hex_widgets::ControlStyle;

pub(super) fn native_device(
    view: &EditorView,
    id: DeviceId,
    class: NativeDeviceClass,
) -> Option<&Device> {
    super::super::device_action_owner::device_in_panel(
        view.device_panel.owner?,
        &view.state.as_ref()?.project,
        id,
        class,
    )
}

pub(super) fn authored(
    device: Option<&Device>,
    target: DeviceAutomationTarget,
) -> Option<&DeviceAutomation> {
    device?
        .automation_target(target)
        .filter(|lane| lane.point_count() != 0)
}

pub(super) fn graph_accent(
    device: Option<&Device>,
    parameters: impl IntoIterator<Item = ParameterId>,
) -> Option<u32> {
    parameters
        .into_iter()
        .any(|parameter| authored(device, DeviceAutomationTarget::Parameter(parameter)).is_some())
        .then(|| theme::element_colors(ElementColor::Red).accent)
}

pub(super) fn style(mut style: ControlStyle, automated: bool) -> ControlStyle {
    if automated {
        let red = theme::element_colors(ElementColor::Red);
        style.accent = red.accent;
        style.accent_text = red.accent_text;
    }
    style
}

pub(super) fn mark(control: Div) -> Div {
    control.relative().child(
        div()
            .debug_selector(|| "device-automation-indicator".to_string())
            .absolute()
            .top(px(1.0))
            .right(px(1.0))
            .size(px(4.0))
            .rounded_full()
            .bg(rgb(theme::element_colors(ElementColor::Red).accent)),
    )
}
