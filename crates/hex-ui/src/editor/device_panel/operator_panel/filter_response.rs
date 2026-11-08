//! Read-only authored carrier-filter display. Pointer editing belongs to its consumer.
use super::*;
use gpui::Entity;
use hex_lattice_contract::NativeDeviceState;
use hex_widgets::{ControlStyle, ResponseGraph, ResponseGraphAxes, ResponseGraphModel};

const POINTS: usize = 128;
const MIN_DB: f32 = -60.0;
const MAX_DB: f32 = 60.0;

#[derive(Clone, PartialEq)]
struct Input {
    state: NativeDeviceState,
    rate: u32,
    style: ControlStyle,
}
pub(in crate::editor::device_panel) struct Control {
    entity: Entity<ResponseGraph>,
    cache: Option<(Input, ResponseGraphModel)>,
}

pub(super) fn hide(view: &mut EditorView, device: DeviceId) {
    if let Some(state) = view.device_panel.operator_previews.get_mut(&device) {
        state.filter_response = None;
    }
}

fn input(
    view: &mut EditorView,
    device: DeviceId,
    parameters: &[NativeDeviceParameterView],
) -> Option<Input> {
    let owner = context::Owner::capture(view, device)?;
    let mut state = owner.state(view)?.clone();
    for parameter in parameters {
        let effective = effective_parameter_value(
            view,
            ParameterKey {
                device,
                parameter: parameter.descriptor.id,
            },
            parameter.value,
        );
        if effective != parameter.value {
            state = hex_engine::patch_native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                &state,
                parameter.descriptor.id,
                effective,
            )
            .ok()?;
        }
    }
    Some(Input {
        state,
        rate: view.state.as_ref()?.project.sample_rate(),
        style: super::display_style(),
    })
}

fn upper(rate: u32) -> f32 {
    (rate as f32 * 0.49).min(20_000.0)
}
fn x(frequency: f32, upper: f32) -> f32 {
    (frequency / 20.0).ln() / (upper / 20.0).ln()
}
fn model(input: &Input) -> Option<(ResponseGraphModel, ResponseGraphAxes)> {
    let upper = upper(input.rate);
    let frequencies: [f32; POINTS] =
        std::array::from_fn(|index| 20.0 * (upper / 20.0).powf(index as f32 / (POINTS - 1) as f32));
    let response = hex_engine::native_operator_filter_frequency_response(
        &input.state,
        input.rate,
        &frequencies,
    )
    .ok()?;
    let primary: Vec<_> = response
        .into_iter()
        .enumerate()
        .map(|(index, gain)| {
            let db = (20.0 * gain.max(1e-6).log10()).clamp(MIN_DB, MAX_DB);
            [
                index as f32 / (POINTS - 1) as f32,
                (db - MIN_DB) / (MAX_DB - MIN_DB),
            ]
        })
        .collect();
    let model = ResponseGraphModel {
        primary: primary.clone(),
        secondary: primary,
        analyzer: None,
        input_analyzer: None,
        analyzer_peak: None,
        nodes: Vec::new(),
    };
    model.validate().ok()?;
    let axes = ResponseGraphAxes {
        horizontal: [
            (20.0, "20"),
            (100.0, "100"),
            (1000.0, "1k"),
            (10_000.0, "10k"),
            (20_000.0, "20k"),
        ]
        .into_iter()
        .filter(|(hz, _)| *hz <= upper)
        .map(|(hz, label)| (x(hz, upper), theme::text(label)))
        .collect(),
        vertical: vec![
            (0.0, theme::text("−60")),
            (0.5, theme::text("0")),
            (1.0, theme::text("+60")),
        ],
        right: Vec::new(),
    };
    Some((model, axes))
}

pub(super) fn graph(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    parameters: &[NativeDeviceParameterView],
) -> Div {
    let result = input(view, device, parameters).and_then(|input| {
        let control = view
            .device_panel
            .operator_previews
            .entry(device)
            .or_default()
            .filter_response
            .get_or_insert_with(|| Control {
                entity: cx.new(|cx| {
                    ResponseGraph::new(
                        cx,
                        format!("operator-filter-response-{}", device.value()),
                        input.style,
                    )
                }),
                cache: None,
            });
        if control
            .cache
            .as_ref()
            .is_none_or(|(cached, _)| cached != &input)
        {
            let (model, axes) = model(&input)?;
            let updated = control.entity.update(cx, |graph, cx| {
                graph.set_axes(Some(axes), cx).is_ok()
                    && graph.set_model(model.clone(), input.style, cx).is_ok()
            });
            if !updated {
                return None;
            }
            control.cache = Some((input, model));
        }
        Some(control.entity.clone())
    });
    let mut content = div()
        .debug_selector(|| "operator-filter-response".into())
        .flex_1()
        .min_h_0()
        .w_full();
    if let Some(graph) = result {
        content = content.child(graph);
    } else {
        hide(view, device);
        content = content.child(theme::text("filter response unavailable"));
    }
    content
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(in crate::editor::device_panel) fn displayed(
    view: &EditorView,
    device: DeviceId,
) -> Option<(&[[f32; 2]], &NativeDeviceState, u32)> {
    let (input, model) = view
        .device_panel
        .operator_previews
        .get(&device)?
        .filter_response
        .as_ref()?
        .cache
        .as_ref()?;
    Some((&model.primary, &input.state, input.rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_curve_and_axes_remain_valid_at_both_native_rate_limits() {
        let state = hex_engine::default_native_device_state_for_class(
            NativeDeviceClass::Instrument,
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.operator"),
        )
        .unwrap();
        for rate in [8000, 192_000] {
            let input = Input {
                state: state.clone(),
                rate,
                style: super::super::display_style(),
            };
            let (curve, axes) = model(&input).unwrap();
            assert_eq!(curve.primary.len(), POINTS);
            assert!(curve.primary.iter().all(|point| point[1] == 0.5));
            assert!(curve.validate().is_ok());
            assert!(axes.horizontal.iter().all(|(x, _)| (0.0..=1.0).contains(x)));
            assert_eq!(
                axes.horizontal.last().unwrap().1.as_ref(),
                if rate == 8000 { "1k" } else { "20k" }
            );
        }
        assert!(
            model(&Input {
                state,
                rate: 0,
                style: super::super::display_style()
            })
            .is_none()
        );
    }
}
