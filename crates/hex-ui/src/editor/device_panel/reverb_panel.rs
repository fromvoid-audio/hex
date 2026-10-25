//! Compact Reverb composition using the shared Lattice controls.
use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView,
    parameter_control_with_presentation,
};
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*};
use hex_engine::{NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::ParameterId;
use hex_project::DeviceId;

pub(super) const FRAME_WIDTH: f32 = crate::device_composition::REVERB_FRAME_WIDTH;

const PARAMETERS: [ParameterId; 2] = [ParameterId::from_static(1), ParameterId::from_static(2)];

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    device_view: &NativeDeviceView,
) -> Stateful<Div> {
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: device_view.type_id,
    };
    let controls = reverb_column(view, cx, device, identity, device_view, &PARAMETERS);
    let rail = crate::device_composition::compact_rail(controls).w_full();
    crate::device_composition::body()
        .child(rail)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}

fn reverb_column(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    identity: DeviceControlIdentity,
    device_view: &NativeDeviceView,
    parameters: &[ParameterId],
) -> Div {
    let mut controls = crate::device_composition::rail_column().items_center();
    for id in parameters {
        let control = device_view
            .parameters
            .iter()
            .find(|parameter| parameter.descriptor.id == *id)
            .map(|parameter| {
                parameter_control_with_presentation(
                    view,
                    cx,
                    device,
                    identity,
                    parameter,
                    ControlPresentation::CompactDial,
                    ControlText::default(),
                )
            })
            .unwrap_or_else(|| div().child(crate::theme::text("unsupported")));
        controls = controls.child(control);
    }
    controls
}
