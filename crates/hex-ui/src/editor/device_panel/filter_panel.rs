//! Filter panel using shared parameter controls and the provider's wet response.

use super::{
    ControlPresentation, ControlText, DeviceControlIdentity, EditorView, ParameterKey,
    effective_parameter_value, parameter_control_with_presentation,
};
use crate::device_composition::{self, dense_rail, plot};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px, rgb};
use hex_engine::{NativeDeviceClass, NativeDeviceView};
use hex_lattice_contract::{NativeDeviceState, ParameterId, ParameterValue};
use hex_project::Device;

pub(super) const FRAME_WIDTH: f32 = 458.0;
const DIAL_GAP: f32 = 3.0;
const DIAL_PAIR_WIDTH: f32 = 2.0 * hex_widgets::MICRO_DIAL_WIDTH + DIAL_GAP;
const MORPH_MODE: ParameterValue = ParameterValue::Choice(4);
const STATIC_CONTROLS: [u32; 5] = [1, 2, 3, 4, 5];

pub(super) struct Preview {
    state: NativeDeviceState,
    values: [ParameterValue; 5],
    sample_rate: u32,
    points: Option<[[f32; 2]; 129]>,
}

fn response(
    view: &mut EditorView,
    device: &Device,
    metadata: &NativeDeviceView,
    sample_rate: u32,
) -> Option<[[f32; 2]; 129]> {
    let mut values = [ParameterValue::Float(0.0); 5];
    for (number, value) in STATIC_CONTROLS.into_iter().zip(&mut values) {
        let parameter = ParameterId::from_static(number);
        let descriptor = metadata
            .parameters
            .iter()
            .find(|p| p.descriptor.id == parameter)?;
        *value = effective_parameter_value(
            view,
            ParameterKey {
                device: device.id(),
                parameter,
            },
            descriptor.value,
        );
    }
    if let Some(cache) = view.device_panel.filter_previews.get(&device.id())
        && cache.state == *device.state()
        && cache.values == values
        && cache.sample_rate == sample_rate
    {
        return cache.points;
    }
    let points = (|| {
        let mut state = device.state().clone();
        for (number, value) in STATIC_CONTROLS.into_iter().zip(values) {
            let parameter = ParameterId::from_static(number);
            if hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Effect,
                &state,
                parameter,
            )
            .ok()?
                != value
            {
                state = hex_engine::patch_native_device_parameter_for_class(
                    NativeDeviceClass::Effect,
                    &state,
                    parameter,
                    value,
                )
                .ok()?;
            }
        }
        hex_engine::native_filter_response(&state, sample_rate).ok()
    })();
    view.device_panel.filter_previews.insert(
        device.id(),
        Preview {
            state: device.state().clone(),
            values,
            sample_rate,
            points,
        },
    );
    points
}

pub(super) fn render_body(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
    metadata: &NativeDeviceView,
) -> Stateful<Div> {
    let id = device.id();
    let sample_rate = view
        .state
        .as_ref()
        .map_or(48_000, |state| state.project.sample_rate());
    let points = response(view, device, metadata, sample_rate);
    let is_morph = metadata
        .parameters
        .iter()
        .find(|p| p.descriptor.id.value() == 1)
        .is_some_and(|p| {
            effective_parameter_value(
                view,
                ParameterKey {
                    device: id,
                    parameter: p.descriptor.id,
                },
                p.value,
            ) == MORPH_MODE
        });
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Effect,
        type_id: metadata.type_id,
    };
    let mut control = |number, presentation, label| {
        let parameter = ParameterId::from_static(number);
        metadata
            .parameters
            .iter()
            .find(|p| p.descriptor.id == parameter)
            .map_or_else(
                || div().child(theme::text("unsupported")),
                |entry| {
                    parameter_control_with_presentation(
                        view,
                        cx,
                        id,
                        identity,
                        entry,
                        presentation,
                        ControlText {
                            label,
                            ..ControlText::default()
                        },
                    )
                },
            )
    };
    use ControlPresentation::{CompactSelector, MicroDial};
    let filter = device_composition::rail_column()
        .child(control(1, CompactSelector, None))
        .child(control(2, CompactSelector, None))
        .child(
            div()
                .flex()
                .gap(px(DIAL_GAP))
                .child(control(3, MicroDial, None))
                .child(control(4, MicroDial, None)),
        )
        .child(
            div()
                .flex()
                .gap(px(DIAL_GAP))
                .child(control(6, MicroDial, Some("mix")))
                .when(is_morph, |row| row.child(control(5, MicroDial, None))),
        );
    let lfo = device_composition::rail_column()
        .child(theme::text("lfo"))
        .child(control(9, CompactSelector, Some("shape")))
        .child(
            div()
                .flex()
                .gap(px(DIAL_GAP))
                .child(control(8, MicroDial, Some("rate")))
                .child(control(7, MicroDial, Some("amount"))),
        )
        .child(control(10, MicroDial, Some("phase")));
    let envelope = device_composition::rail_column()
        .child(theme::text("envelope"))
        .child(control(11, MicroDial, Some("amount")))
        .child(control(13, MicroDial, Some("attack")))
        .child(control(12, MicroDial, Some("release")));
    let style = theme::device_style(theme::DeviceStyleRole::Display);
    let preview = points
        .and_then(|points| {
            hex_widgets::curve_plot(points, style, hex_widgets::CurvePlotGrid::Centered)
        })
        .unwrap_or_else(|| div().child(theme::text("response unavailable")));
    let graph = div()
        .debug_selector(move || format!("filter-response-{}", id.value()))
        .flex_1()
        .min_h_0()
        .w_full()
        .child(preview);
    let upper_khz = 20.0_f32.min(sample_rate as f32 * 0.00045);
    let main = device_composition::rail_column()
        .flex_1()
        .min_w_0()
        .child(theme::text("wet response"))
        .child(plot(graph))
        .child(
            div()
                .flex()
                .justify_between()
                .text_color(rgb(style.muted))
                .child(theme::text("20 hz"))
                .child(theme::text(format!("{upper_khz} khz"))),
        );
    device_composition::body()
        .text_size(px(8.0))
        .gap(px(4.0))
        .py(px(3.0))
        .child(dense_rail(filter).w(px(DIAL_PAIR_WIDTH)).flex_shrink_0())
        .child(main)
        .child(dense_rail(lfo).w(px(DIAL_PAIR_WIDTH)).flex_shrink_0())
        .child(
            dense_rail(envelope)
                .w(px(hex_widgets::MICRO_DIAL_WIDTH))
                .flex_shrink_0(),
        )
        .id(ElementId::Name(
            format!("device-body-{}", id.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", id.value()))
}
