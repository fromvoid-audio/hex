//! Rendered scalar-composition fixtures only; no Service, registry or source owner.
use super::*;
use gpui::{AppContext, Modifiers, Render, TestAppContext, VisualTestContext, point, size};
use std::collections::BTreeMap;

struct ScalarPanel {
    parameters: [NativeDeviceParameterView; 43],
    numeric: BTreeMap<u32, Entity<NumericControl>>,
    toggles: BTreeMap<u32, Entity<Toggle>>,
    events: Vec<(u32, ParameterValue)>,
    _subscriptions: Vec<Subscription>,
}

impl ScalarPanel {
    fn new(cx: &mut Context<Self>, enabled: bool, looping: bool) -> Self {
        let parameters = scalar_fixture(enabled, looping);
        let mut recipes = BTreeMap::new();
        let _ = sampler_panel::sections(&parameters, |parameter, presentation, display| {
            recipes.insert(parameter.descriptor.id.value(), (presentation, display));
            div()
        });
        let mut numeric = BTreeMap::new();
        let mut toggles = BTreeMap::new();
        let mut subscriptions = Vec::new();
        for parameter in &parameters {
            let id = parameter.descriptor.id.value();
            let Some(&(presentation, display)) = recipes.get(&id) else {
                continue;
            };
            let name = format!("scalar-{id}");
            match (parameter.descriptor.kind, parameter.value) {
                (ParameterKind::Float { min, max, unit }, ParameterValue::Float(value)) => {
                    let default = match parameter.descriptor.default {
                        ParameterValue::Float(value) => value,
                        _ => 0.0,
                    };
                    let scale = match parameter.descriptor.presentation {
                        ParameterPresentation::LinearAmplitudeDecibels => {
                            NumericScale::LinearAmplitudeDecibels
                        }
                        _ if min < 0.0 && max > 0.0 => NumericScale::Bipolar,
                        _ => NumericScale::Linear,
                    };
                    let mut spec = NumericSpec::new(
                        f64::from(min),
                        f64::from(max),
                        f64::from(default),
                        scale,
                        2,
                        unit.map(Arc::from),
                    )
                    .unwrap();
                    assert!(matches!(
                        presentation,
                        ControlPresentation::Standard | ControlPresentation::CompactDial
                    ));
                    if let Some(codec) = display.codec {
                        spec = codec.apply(spec);
                    }
                    let config = NumericControlConfig::new(
                        name,
                        theme::text(display.label.unwrap_or(parameter.descriptor.label)),
                        if presentation == ControlPresentation::CompactDial {
                            NumericControlKind::Knob
                        } else {
                            NumericControlKind::HorizontalSlider
                        },
                        spec,
                        f64::from(value),
                        theme::device_style(theme::DeviceStyleRole::Control),
                    )
                    .unwrap()
                    .with_appearance(
                        if presentation == ControlPresentation::CompactDial {
                            NumericControlAppearance::CompactDial
                        } else {
                            NumericControlAppearance::Standard
                        },
                    );
                    let entity = cx.new(|cx| NumericControl::from_config(cx, config));
                    entity.update(cx, |control, cx| control.set_enabled(parameter.enabled, cx));
                    subscriptions.push(cx.subscribe(
                        &entity,
                        move |view, _, event: &NumericControlEvent, _| {
                            view.events
                                .push((id, ParameterValue::Float(event.value as f32)));
                        },
                    ));
                    numeric.insert(id, entity);
                }
                (ParameterKind::Bool, ParameterValue::Bool(value)) => {
                    assert!(matches!(presentation, ControlPresentation::CompactButton));
                    let config = ToggleConfig::new(
                        name,
                        theme::text(display.label.unwrap_or(parameter.descriptor.label)),
                        value,
                        theme::device_style(theme::DeviceStyleRole::Control),
                    )
                    .unwrap()
                    .with_appearance(DiscreteControlAppearance::CompactButton);
                    let entity = cx.new(|cx| Toggle::from_config(cx, config));
                    entity.update(cx, |control, cx| control.set_enabled(parameter.enabled, cx));
                    subscriptions.push(cx.subscribe(
                        &entity,
                        move |view, _, event: &ToggleChanged, _| {
                            view.events.push((id, ParameterValue::Bool(event.checked)))
                        },
                    ));
                    toggles.insert(id, entity);
                }
                _ => {}
            }
        }
        Self {
            parameters,
            numeric,
            toggles,
            events: Vec::new(),
            _subscriptions: subscriptions,
        }
    }
}
impl Render for ScalarPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let [source, pitch, level, envelope] =
            sampler_panel::sections(&self.parameters, |parameter, presentation, _| {
                let id = parameter.descriptor.id.value();
                let control = div().debug_selector(move || format!("sampler-scalar-{id}"));
                if let Some(entity) = self.numeric.get(&id) {
                    let (width, height) = if presentation == ControlPresentation::CompactDial {
                        (64.0, 52.0)
                    } else {
                        (136.0, 26.0)
                    };
                    control.w(px(width)).h(px(height)).child(entity.clone())
                } else if let Some(entity) = self.toggles.get(&id) {
                    control.w(px(40.0)).h(px(18.0)).child(entity.clone())
                } else {
                    control
                }
            });
        crate::device_composition::sampler(source, pitch, level, envelope)
            .font_family(theme::FONT_FAMILY)
            .id("sampler-leaf")
            .debug_selector(|| "sampler-leaf".into())
    }
}

fn scalar_fixture(enabled: bool, looping: bool) -> [NativeDeviceParameterView; 43] {
    // Current empty envelope exercises the REAL published Engine scalar query.
    // The nonempty values below are a Copy-only display fixture, not host admission.
    let empty = NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sampler"),
        6,
        Vec::new(),
    )
    .unwrap();
    let mut parameters = hex_engine::sampler_parameters_on_control(&empty).unwrap();
    assert!(parameters.iter().all(|parameter| !parameter.enabled));
    for parameter in &mut parameters {
        parameter.enabled = enabled;
    }
    for (id, label, value) in [
        (3, "root", 60.0),
        (4, "transpose", 3.0),
        (5, "detune", 7.0),
        (6, "gain", 0.5),
        (7, "pan", -0.25),
        (8, "velocity", 0.75),
        (9, "attack", 0.025),
        (10, "decay", 0.5),
        (11, "sustain", 0.5),
        (12, "release", 1.0),
        (13, "voices", 16.0),
        (17, "start", 1234.0),
        (18, "end", 16_777_216.0),
        (19, "loop start", 1234.0),
        (20, "loop end", 2000.0),
    ] {
        let parameter = parameters
            .iter_mut()
            .find(|parameter| parameter.descriptor.id.value() == id as u32)
            .unwrap();
        assert_eq!(parameter.descriptor.label, label);
        assert!(matches!(
            parameter.descriptor.kind,
            ParameterKind::Float { .. }
        ));
        parameter
            .descriptor
            .validate(ParameterValue::Float(value))
            .unwrap();
        parameter.value = ParameterValue::Float(value);
        parameter.enabled = enabled && (id < 19 || looping);
    }
    parameters
        .iter_mut()
        .find(|parameter| parameter.descriptor.id.value() == 14)
        .unwrap()
        .value = ParameterValue::Bool(looping);
    parameters
}

#[gpui::test]
fn scalar_sampler_layout_contains_actual_controls_at_narrow_and_wide_windows(
    cx: &mut TestAppContext,
) {
    let handle = cx.add_window(|_, cx| ScalarPanel::new(cx, true, true));
    let mut visual = VisualTestContext::from_window(*handle, cx);
    for width in [620.0, 1200.0] {
        visual.simulate_resize(size(px(width), px(240.0)));
        visual.run_until_parked();
        let body = visual.debug_bounds("sampler-leaf").unwrap();
        handle.root(cx).unwrap().read_with(cx, |view, cx| {
            for (id, expected) in [
                (6, "-6.0 db"),
                (8, "75%"),
                (9, "25 ms"),
                (11, "50%"),
                (12, "1 s"),
            ] {
                assert_eq!(view.numeric[&id].read(cx).display_text(), expected);
            }
            for (id, expected) in [(17, "1234"), (18, "16777216"), (19, "1234"), (20, "2000")] {
                assert_eq!(view.numeric[&id].read(cx).display_text(), expected);
                assert_eq!(
                    view.numeric[&id].read(cx).value(),
                    expected.parse::<f64>().unwrap()
                );
            }
        });
        assert_eq!(body.size.width, px(sampler_panel::FRAME_WIDTH));
        for id in [17, 18, 19, 20] {
            let selector = Box::leak(format!("scalar-{id}-value").into_boxed_str());
            let readout = visual
                .debug_bounds(selector)
                .expect("actual wide compact-dial readout");
            assert_eq!(readout.size.width, px(64.0));
        }
        for id in [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 17, 18, 19, 20] {
            let selector = Box::leak(format!("sampler-scalar-{id}").into_boxed_str());
            let control = visual
                .debug_bounds(selector)
                .expect("actual sampler control");
            assert!(
                control.origin.x >= body.origin.x && control.right() <= body.right(),
                "horizontal containment for {id}"
            );
            assert!(
                control.origin.y >= body.origin.y && control.bottom() <= body.bottom(),
                "vertical containment for {id}"
            );
        }
    }
    handle.root(cx).unwrap().update(cx, |view, cx| {
        view.parameters
            .iter_mut()
            .find(|parameter| parameter.descriptor.id.value() == 1)
            .unwrap()
            .value = ParameterValue::Choice(1);
        cx.notify();
    });
    visual.run_until_parked();
    for id in [14, 19, 20] {
        assert!(
            visual
                .debug_bounds(Box::leak(format!("sampler-scalar-{id}").into_boxed_str()))
                .is_none(),
            "non-Classic scalar facts hide Classic loop control {id}"
        );
    }
    assert!(visual.debug_bounds("sampler-scalar-17").is_some());
}

#[gpui::test]
fn scalar_sampler_disabled_controls_refuse_input_and_enabled_controls_emit_typed_events(
    cx: &mut TestAppContext,
) {
    for enabled in [false, true] {
        let handle = cx.add_window(|_, cx| ScalarPanel::new(cx, enabled, false));
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.simulate_resize(size(px(620.0), px(240.0)));
        visual.run_until_parked();
        let reverse = visual.debug_bounds("sampler-scalar-2").unwrap();
        visual.simulate_click(reverse.center(), Modifiers::default());
        let transpose = visual.debug_bounds("scalar-4-slider").unwrap();
        let a = transpose.center();
        visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(a.x + px(20.0), a.y),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        visual.simulate_mouse_up(
            point(a.x + px(20.0), a.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        let loop_start = visual.debug_bounds("scalar-19-knob").unwrap().center();
        visual.simulate_mouse_down(loop_start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(loop_start.x + px(20.0), loop_start.y),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        visual.simulate_mouse_up(
            point(loop_start.x + px(20.0), loop_start.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        handle.root(cx).unwrap().read_with(cx, |view, _| {
            assert!(
                !view.events.iter().any(|(id, _)| *id == 19),
                "disabled loop bounds cannot edit"
            );
            if enabled {
                assert!(
                    view.events
                        .iter()
                        .any(|(id, value)| *id == 2 && *value == ParameterValue::Bool(true))
                );
                assert!(
                    view.events
                        .iter()
                        .any(|(id, value)| *id == 4 && *value != ParameterValue::Float(3.0))
                );
            } else {
                assert!(view.events.is_empty());
            }
        });
    }
}
