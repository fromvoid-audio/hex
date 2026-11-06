//! Native reference-workflow composition. Every control uses canonical bindings;
//! graphs are cached provider queries, never audio processing in paint.
use super::{
    ControlPresentation as P, ControlText, DeviceControlIdentity, EditorView, ParameterControl,
    ParameterKey, effective_parameter_value, numeric_display::NumericDisplayCodec,
    parameter_control_with_presentation,
};
use crate::{device_composition, theme};
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px};
use hex_engine::{NativeDeviceClass, NativeDeviceView, ReverbPreview};
use hex_lattice_contract::{NativeDeviceState, ParameterId, ParameterValue};
use hex_project::{DeviceId, DeviceVariant};
pub(super) const FRAME_WIDTH: f32 = device_composition::REVERB_FRAME_WIDTH;
pub(super) struct PreviewCache {
    generation: u64,
    variant: DeviceVariant,
    state: NativeDeviceState,
    values: [ParameterValue; 33],
    rate: u32,
    curve: Result<ReverbPreview, String>,
}
fn preview(
    view: &mut EditorView,
    device: DeviceId,
    metadata: &NativeDeviceView,
) -> Result<ReverbPreview, String> {
    let mut values = [ParameterValue::Float(0.0); 33];
    for (i, value) in values.iter_mut().enumerate() {
        let id = ParameterId::from_static(i as u32 + 1);
        let parameter = metadata
            .parameters
            .iter()
            .find(|p| p.descriptor.id == id)
            .ok_or("missing reverb parameter")?;
        *value = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: id,
            },
            parameter.value,
        );
    }
    let state = view.state.as_ref().ok_or("missing project")?;
    let entry = state.project.device(device).ok_or("missing reverb")?;
    let rate = state.project.sample_rate();
    let variant = entry.active_variant();
    let authored = entry.state();
    if let Some(cache) = view.device_panel.reverb_previews.get(&device)
        && cache.generation == view.project_generation
        && cache.variant == variant
        && cache.state == *authored
        && cache.values == values
        && cache.rate == rate
    {
        return cache.curve.clone();
    }
    let state = authored.clone();
    let mut draft = state.clone();
    let curve = (|| {
        for (i, value) in values.iter().enumerate() {
            let id = ParameterId::from_static(i as u32 + 1);
            if metadata
                .parameters
                .iter()
                .find(|p| p.descriptor.id == id)
                .is_some_and(|p| p.value != *value)
            {
                draft = hex_engine::patch_native_device_parameter(&draft, id, *value)
                    .map_err(|e| e.to_string())?;
            }
        }
        hex_engine::native_reverb_preview(&draft, rate).map_err(|e| e.to_string())
    })();
    view.device_panel.reverb_previews.insert(
        device,
        PreviewCache {
            generation: view.project_generation,
            variant,
            state,
            values,
            rate,
            curve: curve.clone(),
        },
    );
    curve
}
fn control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    metadata: &NativeDeviceView,
    id: u32,
    label: &'static str,
    presentation: P,
) -> Div {
    metadata
        .parameters
        .iter()
        .find(|p| p.descriptor.id.value() == id)
        .map(|parameter| {
            let rendered = parameter_control_with_presentation(
                view,
                cx,
                device,
                DeviceControlIdentity {
                    class: NativeDeviceClass::Effect,
                    type_id: metadata.type_id,
                },
                parameter,
                presentation,
                ControlText {
                    label: Some(label),
                    codec: match id {
                        4 | 6 | 13 | 16 => Some(NumericDisplayCodec::Hertz),
                        11 | 14 | 17 | 32 | 33 => Some(NumericDisplayCodec::Number),
                        _ => None,
                    },
                },
            );
            if presentation == P::InlineValue {
                let width = if matches!(id, 4 | 6 | 13 | 16) {
                    70.0
                } else {
                    34.0
                };
                if let Some(ParameterControl::Float { entity, .. }) =
                    view.device_panel.controls.get(&ParameterKey {
                        device,
                        parameter: parameter.descriptor.id,
                    })
                {
                    entity.update(cx, |control, cx| {
                        let _ = control.set_inline_value_width(width, cx);
                    });
                }
                rendered.w(px(width)).flex_shrink_0()
            } else {
                rendered.flex_shrink_0()
            }
        })
        .unwrap_or_else(|| div().child(theme::text("unsupported")))
}
fn row() -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(2.0))
        .min_w_0()
        .flex_shrink_0()
        .w_full()
}
fn title(device: DeviceId, label: &'static str) -> Div {
    div()
        .debug_selector(move || format!("reverb-{}-title-slot-{label}", device.value()))
        .h(px(12.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .child(
            div()
                .debug_selector(move || format!("reverb-{}-title-{label}", device.value()))
                .flex_shrink_0()
                .whitespace_nowrap()
                .child(theme::text(label)),
        )
}
fn graph(device: DeviceId, kind: &'static str, points: Option<[[f32; 2]; 65]>, height: f32) -> Div {
    let mut graph = div().w_full().flex_1().min_h_0();
    if let Some(points) = points
        && let Some(plot) = hex_widgets::curve_plot(
            points,
            theme::device_style(theme::DeviceStyleRole::Display),
            hex_widgets::CurvePlotGrid::Horizontal,
        )
    {
        let plot = plot.debug_selector(move || format!("reverb-{}-{kind}-curve", device.value()));
        graph = graph.child(plot);
    }
    // Shared plots fill remaining space with a zero flex basis. This panel
    // allocates fixed plot rows; flex_none alone does not clear that basis.
    device_composition::plot(graph)
        .h(px(height))
        .flex_basis(px(height))
        .flex_none()
}
fn section(controls: Div) -> Div {
    device_composition::dense_rail(controls).h_full()
}
pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    metadata: &NativeDeviceView,
) -> Stateful<Div> {
    let preview = preview(view, device, metadata).ok();
    let mut field =
        |id, label, presentation| control(view, cx, device, metadata, id, label, presentation);
    let input_controls = device_composition::rail_column()
        .child(title(device, "input filter"))
        .child(
            row()
                .child(field(3, "lo cut", P::CompactButton))
                .child(field(5, "hi cut", P::CompactButton)),
        )
        .child(graph(
            device,
            "input",
            preview.as_ref().map(|p| p.input),
            34.0,
        ))
        .child(
            row()
                .child(theme::text("lo"))
                .child(field(4, "lo hz", P::InlineValue))
                .child(field(32, "lo q", P::InlineValue)),
        )
        .child(
            row()
                .child(theme::text("hi"))
                .child(field(6, "hi hz", P::InlineValue))
                .child(field(33, "hi q", P::InlineValue)),
        );
    let input = section(input_controls).w(px(128.0)).flex_shrink_0();
    let early_controls = device_composition::rail_column()
        .child(title(device, "early reflections"))
        .child(field(8, "spin", P::CompactButton))
        .child(
            row()
                .child(field(9, "amount", P::MicroDial))
                .child(field(10, "rate", P::MicroDial)),
        )
        .child(
            row()
                .child(theme::text("shape"))
                .child(field(11, "shape", P::InlineValue)),
        );
    let early = section(early_controls).w(px(124.0)).flex_shrink_0();
    let high = row()
        .child(field(12, "high", P::CompactButton))
        .child(field(13, "freq", P::InlineValue))
        .child(field(14, "ratio", P::InlineValue));
    let low = row()
        .child(field(15, "low", P::CompactButton))
        .child(field(16, "freq", P::InlineValue))
        .child(field(17, "ratio", P::InlineValue));
    let graph_column = device_composition::rail_column()
        .flex_1()
        .min_w_0()
        .child(high)
        .child(graph(device, "decay", preview.map(|p| p.decay), 48.0))
        .child(low);
    let contour = device_composition::rail_column()
        .w(px(hex_widgets::MICRO_DIAL_WIDTH))
        .flex_shrink_0()
        .child(field(18, "diffusion", P::MicroDial))
        .child(field(19, "scale", P::MicroDial));
    let diffusion_controls = device_composition::rail_column()
        .child(title(device, "diffusion network"))
        .child(row().items_start().child(graph_column).child(contour));
    let diffusion = section(diffusion_controls).flex_1().min_w_0();
    let chorus_controls = device_composition::rail_column()
        .items_center()
        .child(field(21, "chorus", P::CompactButton))
        .child(field(22, "amount", P::MicroDial))
        .child(field(23, "rate", P::MicroDial));
    let chorus = section(chorus_controls).w(px(64.0)).flex_shrink_0();
    let top = row()
        .items_start()
        .h(px(106.0))
        .flex_shrink_0()
        .child(input)
        .child(early)
        .child(diffusion)
        .child(chorus);
    let smooth = device_composition::rail_column()
        .w(px(64.0))
        .flex_shrink_0()
        .child(title(device, "smooth"))
        .child(field(24, "smooth", P::CompactSelector));
    let density = device_composition::rail_column()
        .w(px(64.0))
        .flex_shrink_0()
        .child(title(device, "density"))
        .child(field(25, "density", P::CompactSelector));
    let freeze = device_composition::rail_column()
        .w(px(82.0))
        .flex_shrink_0()
        .items_center()
        .child(field(26, "freeze", P::CompactButton))
        .child(
            row()
                .child(field(27, "flat", P::CompactButton))
                .child(field(28, "cut", P::CompactButton)),
        );
    let bottom = row()
        .h(px(52.0))
        .flex_shrink_0()
        .child(field(7, "predelay", P::CompactDial))
        .child(smooth)
        .child(field(20, "size", P::CompactDial))
        .child(field(1, "decay", P::CompactDial))
        .child(freeze)
        .child(field(29, "stereo", P::CompactDial))
        .child(density);
    let main = device_composition::rail_column()
        .flex_1()
        .min_w_0()
        .child(top)
        .child(bottom);
    let output_controls = device_composition::rail_column()
        .child(field(30, "reflect", P::CompactDial))
        .child(field(31, "diffuse", P::CompactDial))
        .child(field(2, "dry/wet", P::CompactDial));
    let output = section(output_controls).w(px(66.0)).flex_shrink_0();
    device_composition::body()
        .gap(px(2.0))
        .text_size(px(8.0))
        .line_height(px(10.0))
        .whitespace_nowrap()
        .child(main)
        .child(output)
        .id(ElementId::Name(
            format!("device-body-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device.value()))
}
