//! Native and VST3 modulator strip panel layout and controls.
use super::*;

pub(in crate::editor) fn macro_panel(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
    view_data: &NativeDeviceView,
    is_effect: bool,
) -> Option<AnyElement> {
    macro_panel_with_targets(view, cx, device, Some(view_data), is_effect)
}

pub(in crate::editor) fn vst3_macro_panel(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
) -> Option<AnyElement> {
    macro_panel_with_targets(view, cx, device, None, true)
}

fn slot_preview_and_value(
    source: &hex_project::Modulator,
    kind: SourceKind,
) -> (hex_widgets::HostSlotPreview, String, f32) {
    match kind {
        SourceKind::Macro => {
            let v =
                manual::Settings::decode(&source.state).map_or(0.0, |s| s.value.clamp(0.0, 1.0));
            (
                hex_widgets::HostSlotPreview::MacroDial(v),
                format!("{:.0}%", v * 100.0),
                v,
            )
        }
        SourceKind::MacroFour => {
            let v = macro_four::Settings::decode(&source.state)
                .map_or(0.0, |s| s.values[0].clamp(0.0, 1.0));
            (
                hex_widgets::HostSlotPreview::MacroDial(v),
                format!("{:.0}%", v * 100.0),
                v,
            )
        }
        SourceKind::Button => {
            let on = manual_button::decode(&source.state).unwrap_or(false);
            (
                hex_widgets::HostSlotPreview::Toggle(on),
                if on { "on".into() } else { "off".into() },
                if on { 1.0 } else { 0.0 },
            )
        }
        SourceKind::Buttons => {
            let active = buttons::Settings::decode(&source.state)
                .map_or(0, |s| s.on.into_iter().filter(|on| *on).count());
            (
                hex_widgets::HostSlotPreview::Toggle(active > 0),
                format!("{active}/4"),
                active as f32 / 4.0,
            )
        }
        SourceKind::Lfo => {
            let (wf, text) = lfo::Settings::decode(&source.state).map_or(
                (hex_widgets::HostSlotWaveform::Sine, "lfo".into()),
                |s| {
                    let wf = match s.waveform {
                        lfo::Waveform::Sine => hex_widgets::HostSlotWaveform::Sine,
                        lfo::Waveform::Triangle => hex_widgets::HostSlotWaveform::Triangle,
                        lfo::Waveform::RisingRamp => hex_widgets::HostSlotWaveform::Ramp,
                        lfo::Waveform::Square => hex_widgets::HostSlotWaveform::Square,
                    };
                    let text = match s.rate {
                        lfo::Rate::Hertz(hz) => format!("{hz:.1} hz"),
                        lfo::Rate::BeatsPerCycle(beats) => format!("{beats:.2} b"),
                    };
                    (wf, text)
                },
            );
            (hex_widgets::HostSlotPreview::Waveform(wf), text, 0.5)
        }
        SourceKind::Random => {
            let text = random::Settings::decode(&source.state)
                .map_or_else(|_| "rnd".into(), |s| format!("{:.1} hz", s.rate_hz));
            (
                hex_widgets::HostSlotPreview::Waveform(hex_widgets::HostSlotWaveform::Random),
                text,
                0.5,
            )
        }
        SourceKind::AhdRelease | SourceKind::Adsr | SourceKind::Ramp => (
            hex_widgets::HostSlotPreview::Waveform(hex_widgets::HostSlotWaveform::Envelope),
            kind.label().to_owned(),
            0.5,
        ),
        SourceKind::Steps => (
            hex_widgets::HostSlotPreview::Waveform(hex_widgets::HostSlotWaveform::Steps),
            "steps".into(),
            0.5,
        ),
        SourceKind::Follower | SourceKind::AudioRate | SourceKind::NotePressure => (
            hex_widgets::HostSlotPreview::Level(0.5),
            kind.label().to_owned(),
            0.5,
        ),
        SourceKind::Math | SourceKind::Mix | SourceKind::NoteCounter => (
            hex_widgets::HostSlotPreview::Level(0.5),
            kind.label().to_owned(),
            0.5,
        ),
    }
}

fn slot_source_kind(source: &hex_project::Modulator) -> Option<SourceKind> {
    match source.state.type_id() {
        lfo::TYPE_ID => Some(SourceKind::Lfo),
        follower::TYPE_ID => Some(SourceKind::Follower),
        manual::TYPE_ID => Some(SourceKind::Macro),
        macro_four::TYPE_ID => Some(SourceKind::MacroFour),
        random::TYPE_ID => Some(SourceKind::Random),
        audio_rate::TYPE_ID => Some(SourceKind::AudioRate),
        pressure::TYPE_ID => Some(SourceKind::NotePressure),
        manual_button::TYPE_ID => Some(SourceKind::Button),
        math::TYPE_ID => Some(SourceKind::Math),
        buttons::TYPE_ID => Some(SourceKind::Buttons),
        mix::TYPE_ID => Some(SourceKind::Mix),
        note_counter::TYPE_ID => Some(SourceKind::NoteCounter),
        ramp::TYPE_ID => Some(SourceKind::Ramp),
        ahd_release::TYPE_ID => Some(SourceKind::AhdRelease),
        adsr::TYPE_ID => Some(SourceKind::Adsr),
        steps::TYPE_ID => Some(SourceKind::Steps),
        _ => None,
    }
}

fn source_ring(
    bank: &DeviceModulation,
    source_id: ModulationSourceId,
    base: f32,
    color: u32,
) -> Option<hex_widgets::HostSlotModulationRing> {
    let delta = bank
        .routes()
        .iter()
        .filter(|route| route.enabled && route.source == source_id)
        .map(|route| route.amount.get())
        .next()?;
    Some(hex_widgets::HostSlotModulationRing { base, delta, color })
}

fn macro_panel_with_targets(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
    view_data: Option<&NativeDeviceView>,
    is_effect: bool,
) -> Option<AnyElement> {
    let state = view.state.as_ref()?;
    let id = device.id();
    let revision = state.revision;
    let generation = view.project_generation;
    let bank = device.modulation();
    if !is_effect && bank.is_empty() && !hex_engine::instrument_modulation_supported(device.state())
    {
        return None;
    }
    let has_target = view_data.map_or_else(
        || vst3_parameters::has_mapping_target(view, id),
        |view_data| {
            view_data.parameters.iter().any(|row| {
                row.enabled
                    && row.semantics.is_some_and(|semantics| {
                        semantics.modulation == ParameterModulation::SampleRate
                    })
            })
        },
    );
    let mut cfg = hex_widgets::HostSlotPanelConfig::new(
        "device-macro-slots",
        "macros",
        "no macros",
        theme::device_style(theme::DeviceStyleRole::Control),
    );
    if (has_target || !bank.is_empty())
        && bank.sources().len() < hex_project::MAX_MODULATORS_PER_DEVICE
        && hex_engine::device_available_modulation_outputs(device) > 0
    {
        cfg.header_action = Some(
            action(format!("add-macro-{}", id.value()), theme::text("+"))
                .on_click(cx.listener(move |view, _, _, cx| {
                    cx.stop_propagation();
                    submit(view, generation, revision, id, Edit::Add(SourceKind::Macro));
                    cx.notify();
                }))
                .into_any_element(),
        );
    }
    let mut macro_index = 0usize;
    for source in bank.sources() {
        let Some(kind) = slot_source_kind(source) else {
            continue;
        };
        if !matches!(
            kind,
            SourceKind::Macro | SourceKind::MacroFour | SourceKind::Button | SourceKind::Buttons
        ) {
            continue;
        }
        macro_index += 1;
        let color = theme::element_colors(source.color).accent;
        let (preview, value, base) = slot_preview_and_value(source, kind);
        let ring = source_ring(bank, source.id, base, color);
        cfg.slots.push(hex_widgets::HostSlotItem {
            id: format!("macro-{}", source.id.value()).into(),
            label: format!("m{macro_index}").into(),
            value: value.into(),
            accent_color: color,
            enabled: source.enabled,
            preview,
            modulation_ring: ring,
            interactive: None,
        });
    }
    Some(hex_widgets::host_slot_panel(cfg).into_any_element())
}

pub(in crate::editor) fn panel(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
    view_data: &NativeDeviceView,
    is_effect: bool,
) -> Option<AnyElement> {
    panel_with_targets(view, cx, device, Some(view_data), is_effect)
}

pub(in crate::editor) fn vst3_panel(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
) -> Option<AnyElement> {
    panel_with_targets(view, cx, device, None, true)
}

pub(in crate::editor) fn panel_with_targets(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: &Device,
    view_data: Option<&NativeDeviceView>,
    is_effect: bool,
) -> Option<AnyElement> {
    let state = view.state.as_ref()?;
    let id = device.id();
    let revision = state.revision;
    let generation = view.project_generation;
    let bank = device.modulation();
    if !is_effect && bank.is_empty() && !hex_engine::instrument_modulation_supported(device.state())
    {
        return None;
    }
    let has_target = view_data.map_or_else(
        || vst3_parameters::has_mapping_target(view, id),
        |view_data| {
            view_data.parameters.iter().any(|row| {
                row.enabled
                    && row.semantics.is_some_and(|semantics| {
                        semantics.modulation == ParameterModulation::SampleRate
                    })
            })
        },
    );
    let selected = view
        .device_panel
        .mapping_source
        .filter(|(device, source)| {
            *device == id && bank.sources().iter().any(|slot| source.valid_for(slot))
        })
        .map(|(_, source)| source);
    let host_gains = if selected.is_some() || (!has_target && bank.is_empty()) {
        let cache = &mut view.device_panel.host_gain_mapping_availability;
        if let Some(cached) = cache.as_ref().filter(|entry| {
            entry.device == id && entry.generation == generation && entry.revision == revision
        }) {
            cached.stages
        } else {
            let effect =
                hex_engine::project_device_host_gain_modulation_supported(&state.project, id);
            let stages = HostGainStages {
                input: effect,
                output: effect
                    || hex_engine::project_device_instrument_output_modulation_supported(
                        &state.project,
                        id,
                    ),
            };
            *cache = Some(HostGainMappingAvailability {
                device: id,
                generation,
                revision,
                stages,
            });
            stages
        }
    } else {
        HostGainStages::default()
    };
    let host_gain_available = host_gains.input || host_gains.output;
    if !has_target && !host_gain_available && bank.is_empty() {
        return None;
    }
    let button = |cx: &mut Context<EditorView>, key: String, label: SharedString, edit| {
        action(key, label).on_click(cx.listener(move |view, _, _, cx| {
            cx.stop_propagation();
            submit(view, generation, revision, id, edit);
            cx.notify();
        }))
    };
    let mut body = div().flex().flex_col().flex_shrink_0().gap(px(3.0));
    if bank.sources().len() < hex_project::MAX_MODULATORS_PER_DEVICE
        && (hex_engine::device_available_modulation_outputs(device) > 0
            || hex_engine::project_device_can_add_note_pressure(&state.project, id))
    {
        let target = AddModulatorTarget {
            generation,
            revision,
            owner: view.device_panel.owner,
            device: id,
        };
        let add_button = action(
            format!("add-modulator-{}", id.value()),
            theme::text("+ modulator"),
        )
        .on_click(
            cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                cx.stop_propagation();
                open_add_menu(view, target, is_effect, event.position(), window, cx);
            }),
        );
        body = body.child(add_button);
    }
    if let Some(source) = selected
        && host_gain_available
        && !is_pressure_source(bank.sources(), source.source)
    {
        let targets = [HostGainTarget::Input, HostGainTarget::Output]
            .into_iter()
            .filter(|&stage| host_gains.supports(stage))
            .filter_map(|stage| {
                let (parameter, key, label) = stage.mapping();
                let route = bank.routes().iter().find(|route| {
                    source.owns(route)
                        && matches!(route.destination, ModulationDestination::Device { target, .. } if target == parameter)
                });
                if route.is_none() && bank.routes().len() >= hex_project::MAX_MODULATION_ROUTES_PER_DEVICE {
                    return None;
                }
                let target = target_drag::Target { device: id, source, mapping: MappingTarget::HostGain(stage) };
                Some(mapping_button(
                    view, cx, format!("map-{key}-{}", id.value()), theme::text(label),
                    target, route, revision,
                ))
            });
        body = body.child(div().flex().flex_wrap().gap(px(4.0)).children(targets));
    }
    let blocked_targets: Vec<_> = selected
        .into_iter()
        .flat_map(|source| bank.source_route_ancestors(source.source))
        .collect();
    let pressure_selected =
        selected.is_some_and(|output| is_pressure_source(bank.sources(), output.source));
    let input_mapping_control = |cx: &mut Context<EditorView>,
                                 key: String,
                                 label: &str,
                                 target_id,
                                 mapping,
                                 destination| {
        let source = selected.filter(|_| !pressure_selected && !blocked_targets.is_empty())?;
        let route = bank
            .routes()
            .iter()
            .find(|route| source.owns(route) && route.destination == destination);
        if target_id == source.source
            || (route.is_none()
                && (blocked_targets.contains(&target_id)
                    || bank.routes().len() >= hex_project::MAX_MODULATION_ROUTES_PER_DEVICE))
        {
            return None;
        }
        let target = target_drag::Target {
            device: id,
            source,
            mapping,
        };
        Some(mapping_button(
            view,
            cx,
            key,
            theme::text(label),
            target,
            route,
            revision,
        ))
    };
    let multiple_macros = bank
        .sources()
        .iter()
        .filter(|source| source.state.type_id() == manual::TYPE_ID)
        .take(2)
        .count()
        > 1;
    let multiple_lfos = bank
        .sources()
        .iter()
        .filter(|source| source.state.type_id() == lfo::TYPE_ID)
        .take(2)
        .count()
        > 1;
    let multiple_math = bank
        .sources()
        .iter()
        .filter(|source| source.state.type_id() == math::TYPE_ID)
        .take(2)
        .count()
        > 1;
    let multiple_mix = bank
        .sources()
        .iter()
        .filter(|source| source.state.type_id() == mix::TYPE_ID)
        .take(2)
        .count()
        > 1;
    for (slot, source) in bank.sources().iter().enumerate() {
        let source_id = source.id;
        let kind = match source.state.type_id() {
            lfo::TYPE_ID => SourceKind::Lfo,
            follower::TYPE_ID => SourceKind::Follower,
            manual::TYPE_ID => SourceKind::Macro,
            macro_four::TYPE_ID => SourceKind::MacroFour,
            random::TYPE_ID => SourceKind::Random,
            audio_rate::TYPE_ID => SourceKind::AudioRate,
            pressure::TYPE_ID => SourceKind::NotePressure,
            manual_button::TYPE_ID => SourceKind::Button,
            math::TYPE_ID => SourceKind::Math,
            buttons::TYPE_ID => SourceKind::Buttons,
            mix::TYPE_ID => SourceKind::Mix,
            note_counter::TYPE_ID => SourceKind::NoteCounter,
            ramp::TYPE_ID => SourceKind::Ramp,
            ahd_release::TYPE_ID => SourceKind::AhdRelease,
            adsr::TYPE_ID => SourceKind::Adsr,
            steps::TYPE_ID => SourceKind::Steps,
            _ => continue,
        };
        let label = kind.label();
        let title = if (multiple_macros && matches!(kind, SourceKind::Macro))
            || (multiple_lfos && matches!(kind, SourceKind::Lfo))
            || (multiple_math && matches!(kind, SourceKind::Math))
            || (multiple_mix && matches!(kind, SourceKind::Mix))
        {
            format!("{label} {}", slot + 1)
        } else {
            label.to_owned()
        };
        let source_key = |suffix: &str| format!("{label}-{}-{suffix}", source_id.value());
        let map_control = |cx: &mut Context<EditorView>, key, source| {
            let mapping = selected == Some(source);
            action(key, theme::text(if mapping { "done" } else { "map" })).on_click(cx.listener(
                move |view, _, _, cx| {
                    cx.stop_propagation();
                    if view.project_generation != generation {
                        return;
                    }
                    view.device_panel.mapping_source = (!mapping).then_some((id, source));
                    cx.notify();
                },
            ))
        };
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(3.0))
            .child(theme::text(title))
            .child(button(
                cx,
                source_key("power"),
                theme::text(if source.enabled { "on" } else { "off" }),
                Edit::ToggleSource(source_id),
            ))
            .children(
                (!matches!(kind, SourceKind::MacroFour | SourceKind::Buttons))
                    .then(|| map_control(cx, source_key("map"), MappingOutput::main(source_id))),
            )
            .children(
                (!matches!(kind, SourceKind::NotePressure)
                    && bank.sources().len() < hex_project::MAX_MODULATORS_PER_DEVICE)
                    .then(|| {
                        button(
                            cx,
                            source_key("duplicate"),
                            theme::text("duplicate"),
                            Edit::DuplicateSource(source_id),
                        )
                    }),
            )
            .child(button(
                cx,
                source_key("remove"),
                theme::text("×"),
                Edit::RemoveSource(source_id),
            ));
        if matches!(kind, SourceKind::Button)
            && let Ok(on) = manual_button::decode(&source.state)
        {
            row = row.child(button(
                cx,
                source_key("value"),
                theme::text(if on { "output on" } else { "output off" }),
                Edit::ToggleButton(source_id),
            ));
        }
        if let SourceKind::Buttons = kind
            && let Ok(settings) = buttons::Settings::decode(&source.state)
        {
            for (index, on) in settings.on.into_iter().enumerate() {
                let number = index as u32 + 1;
                let Ok(output) = ModulationOutputId::new(number) else {
                    continue;
                };
                let mapped = MappingOutput {
                    source: source_id,
                    output,
                };
                let label = if on { "on" } else { "off" };
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(button(
                            cx,
                            source_key(&format!("{number}-toggle")),
                            theme::text(format!("{number} {label}")),
                            Edit::ToggleButtons(mapped),
                        ))
                        .children([map_control(
                            cx,
                            source_key(&format!("{number}-map")),
                            mapped,
                        )]),
                );
            }
        }
        if let SourceKind::NoteCounter = kind
            && let Ok(settings) = note_counter::Settings::decode(&source.state)
        {
            for (label, value, edit) in [
                (
                    "steps",
                    settings.steps,
                    CounterEdit::Steps as fn(i16) -> CounterEdit,
                ),
                ("increment", settings.increment, CounterEdit::Increment),
            ] {
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(theme::text(format!("{label} {value}")))
                        .child(button(
                            cx,
                            source_key(&format!("{label}-less")),
                            theme::text("−"),
                            Edit::NoteCounter(source_id, edit(-1)),
                        ))
                        .child(button(
                            cx,
                            source_key(&format!("{label}-more")),
                            theme::text("+"),
                            Edit::NoteCounter(source_id, edit(1)),
                        )),
                );
            }
            row = row.child(button(
                cx,
                source_key("polarity"),
                theme::text(if settings.bipolar {
                    "bipolar"
                } else {
                    "unipolar"
                }),
                Edit::NoteCounter(source_id, CounterEdit::TogglePolarity),
            ));
        }
        if let SourceKind::Ramp = kind
            && let Ok(settings) = ramp::Settings::decode(&source.state)
        {
            let duration = duration_text(settings.duration_micros);
            let (curve, next) = curve_control(settings.curve);
            row = row
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(button(
                            cx,
                            source_key("shorter"),
                            theme::text("−"),
                            Edit::Ramp(source_id, RampEdit::Duration { longer: false }),
                        ))
                        .child(theme::text(duration))
                        .child(button(
                            cx,
                            source_key("longer"),
                            theme::text("+"),
                            Edit::Ramp(source_id, RampEdit::Duration { longer: true }),
                        )),
                )
                .child(button(
                    cx,
                    source_key("curve"),
                    theme::text(curve),
                    Edit::Ramp(source_id, RampEdit::Curve(next)),
                ))
                .child(button(
                    cx,
                    source_key("direction"),
                    theme::text(if settings.falling {
                        "falling"
                    } else {
                        "rising"
                    }),
                    Edit::Ramp(source_id, RampEdit::ToggleDirection),
                ))
                .child(button(
                    cx,
                    source_key("loop"),
                    theme::text(if settings.looping { "loop" } else { "one shot" }),
                    Edit::Ramp(source_id, RampEdit::ToggleLoop),
                ));
        }
        let envelope_time = |cx: &mut Context<EditorView>, name: &str, micros, edits: [Edit; 2]| {
            div()
                .max_w_full()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(2.0))
                .child(theme::text(format!("{name} {}", duration_text(micros))))
                .child(button(
                    cx,
                    source_key(&format!("{name}-shorter")),
                    theme::text("−"),
                    edits[0],
                ))
                .child(button(
                    cx,
                    source_key(&format!("{name}-longer")),
                    theme::text("+"),
                    edits[1],
                ))
        };
        if let SourceKind::AhdRelease = kind
            && let Ok(settings) = ahd_release::Settings::decode(&source.state)
        {
            for (index, (name, curve_index)) in
                [("attack", Some(0)), ("hold", None), ("decay", Some(1))]
                    .into_iter()
                    .enumerate()
            {
                let mut controls = envelope_time(
                    cx,
                    name,
                    settings.durations_micros[index],
                    [false, true].map(|longer| {
                        Edit::AhdRelease(
                            source_id,
                            AhdEdit::Duration {
                                segment: index,
                                longer,
                            },
                        )
                    }),
                );
                if let Some(curve_index) = curve_index {
                    let (curve, next) = curve_control(settings.curves[curve_index]);
                    controls = controls.child(button(
                        cx,
                        source_key(&format!("{name}-curve")),
                        theme::text(curve),
                        Edit::AhdRelease(
                            source_id,
                            AhdEdit::Curve {
                                index: curve_index,
                                curve: next,
                            },
                        ),
                    ));
                }
                row = row.child(controls);
            }
            row = row.child(button(
                cx,
                source_key("single-trigger"),
                theme::text(if settings.single_trigger {
                    "single trigger"
                } else {
                    "retrigger"
                }),
                Edit::AhdRelease(source_id, AhdEdit::ToggleSingleTrigger),
            ));
        }
        if let SourceKind::Adsr = kind
            && let Ok(settings) = adsr::Settings::decode(&source.state)
        {
            for (segment, name) in ["attack", "decay", "release"].into_iter().enumerate() {
                let controls = envelope_time(
                    cx,
                    name,
                    settings.durations_micros[segment],
                    [false, true].map(|longer| {
                        Edit::Adsr(source_id, AdsrEdit::Duration { segment, longer })
                    }),
                );
                row = row.child(controls);
            }
            row = row
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(theme::text(format!(
                            "sustain {:.0}%",
                            settings.sustain * 100.0
                        )))
                        .child(button(
                            cx,
                            source_key("sustain-less"),
                            theme::text("−"),
                            Edit::Adsr(source_id, AdsrEdit::Sustain(-0.05)),
                        ))
                        .child(button(
                            cx,
                            source_key("sustain-more"),
                            theme::text("+"),
                            Edit::Adsr(source_id, AdsrEdit::Sustain(0.05)),
                        )),
                )
                .child(button(
                    cx,
                    source_key("single-trigger"),
                    theme::text(if settings.single_trigger {
                        "single trigger"
                    } else {
                        "retrigger"
                    }),
                    Edit::Adsr(source_id, AdsrEdit::ToggleSingleTrigger),
                ));
        }
        if let SourceKind::Steps = kind
            && let Ok(settings) = steps::Settings::decode(&source.state)
        {
            let direction = match settings.direction {
                steps::Direction::Forward => "forward",
                steps::Direction::Backward => "backward",
                steps::Direction::PingPong => "ping pong",
            };
            if is_effect {
                let mode = match settings.timing {
                    steps::Timing::FreeMicros(_) => "free",
                    steps::Timing::Arrangement { .. } => "arrangement",
                };
                row = row.child(button(
                    cx,
                    source_key("timing-mode"),
                    theme::text(mode),
                    Edit::Steps(source_id, StepsEdit::ToggleTiming),
                ));
            }
            let time_edit = |longer| Edit::Steps(source_id, StepsEdit::Time { longer });
            let timing = match settings.timing {
                steps::Timing::FreeMicros(micros) => {
                    envelope_time(cx, "step time", micros, [false, true].map(time_edit))
                }
                steps::Timing::Arrangement { steps_per_beat } => {
                    let timing = source_key("arrangement-time");
                    div()
                        .debug_selector(move || timing.clone())
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(button(
                            cx,
                            source_key("division-shorter"),
                            theme::text("−"),
                            time_edit(false),
                        ))
                        .child(theme::text(format!("1/{}", 4 * steps_per_beat)))
                        .child(button(
                            cx,
                            source_key("division-longer"),
                            theme::text("+"),
                            time_edit(true),
                        ))
                }
            };
            row = row
                .children([timing])
                .child(button(
                    cx,
                    source_key("direction"),
                    theme::text(direction),
                    Edit::Steps(source_id, StepsEdit::CycleDirection),
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(theme::text(format!("{} steps", settings.pattern.len())))
                        .child(button(
                            cx,
                            source_key("length-less"),
                            theme::text("−"),
                            Edit::Steps(source_id, StepsEdit::Length { add: false }),
                        ))
                        .child(button(
                            cx,
                            source_key("length-more"),
                            theme::text("+"),
                            Edit::Steps(source_id, StepsEdit::Length { add: true }),
                        )),
                );
            let mut pattern = div().w_full().flex().flex_wrap().gap(px(3.0));
            for (index, value) in settings.pattern.into_iter().enumerate() {
                let number = index + 1;
                let amount = f32::from(value) / f32::from(i16::MAX) * 100.0;
                pattern = pattern.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(theme::text(format!("{number} {amount:+.0}%")))
                        .child(button(
                            cx,
                            source_key(&format!("step-{number}-less")),
                            theme::text("−"),
                            Edit::Steps(
                                source_id,
                                StepsEdit::Value {
                                    index,
                                    delta: -3277,
                                },
                            ),
                        ))
                        .child(button(
                            cx,
                            source_key(&format!("step-{number}-more")),
                            theme::text("+"),
                            Edit::Steps(source_id, StepsEdit::Value { index, delta: 3277 }),
                        )),
                );
            }
            row = row.children([pattern]);
        }
        if matches!(kind, SourceKind::NotePressure) {
            row = row.child(theme::text("one carrier"));
        }
        if let SourceKind::Lfo = kind
            && let Ok(settings) = lfo::Settings::decode(&source.state)
        {
            let (rate, units) = match settings.rate {
                lfo::Rate::Hertz(rate) => (rate, "hz"),
                lfo::Rate::BeatsPerCycle(rate) => (rate, "beats"),
            };
            let (shape, next) = match settings.waveform {
                lfo::Waveform::Sine => ("sine", lfo::Waveform::Triangle),
                lfo::Waveform::Triangle => ("triangle", lfo::Waveform::RisingRamp),
                lfo::Waveform::RisingRamp => ("ramp", lfo::Waveform::Square),
                lfo::Waveform::Square => ("square", lfo::Waveform::Sine),
            };
            let destination = lfo_rate_destination(source_id);
            let rate_text = format!("{rate:.2} {units}");
            let rate_control = if let Some(control) = input_mapping_control(
                cx,
                format!("map-lfo-rate-{}-{}", id.value(), source_id.value()),
                &rate_text,
                source_id,
                MappingTarget::LfoRate(source_id),
                destination,
            ) {
                control.into_any_element()
            } else if selected.is_some() {
                div()
                    .px(px(4.0))
                    .h(px(15.0))
                    .child(theme::text(rate_text))
                    .into_any_element()
            } else {
                button(
                    cx,
                    source_key("sync"),
                    theme::text(rate_text),
                    Edit::Sync(source_id),
                )
                .into_any_element()
            };
            row = row
                .child(button(
                    cx,
                    source_key("slower"),
                    theme::text("−"),
                    Edit::Rate(source_id, 0.5),
                ))
                .child(div().children([rate_control]))
                .child(button(
                    cx,
                    source_key("faster"),
                    theme::text("+"),
                    Edit::Rate(source_id, 2.0),
                ))
                .child(
                    button(
                        cx,
                        source_key("waveform"),
                        theme::text(shape),
                        Edit::LfoShape(source_id, next),
                    )
                    .w(px(55.0)),
                );
        }
        if let SourceKind::Follower = kind
            && let Ok(settings) = follower::Settings::decode(&source.state)
        {
            for (attack, name, seconds) in [
                (true, "attack", settings.attack_seconds),
                (false, "release", settings.release_seconds),
            ] {
                row = row.child(
                    div()
                        .flex()
                        .gap(px(2.0))
                        .items_center()
                        .child(button(
                            cx,
                            source_key(&format!("{name}-less")),
                            theme::text("−"),
                            Edit::FollowerTime {
                                source: source_id,
                                attack,
                                factor: 0.5,
                            },
                        ))
                        .child(theme::text(format!("{name} {:.1} ms", seconds * 1000.0)))
                        .child(button(
                            cx,
                            source_key(&format!("{name}-more")),
                            theme::text("+"),
                            Edit::FollowerTime {
                                source: source_id,
                                attack,
                                factor: 2.0,
                            },
                        )),
                );
            }
        }
        if let SourceKind::Macro = kind
            && let Ok(settings) = manual::Settings::decode(&source.state)
        {
            let value = format!("{:.0}%", settings.value * 100.0);
            if let Some(control) = input_mapping_control(
                cx,
                format!("map-macro-{}-{}", id.value(), source_id.value()),
                &value,
                source_id,
                MappingTarget::Macro(source_id),
                macro_destination(source_id),
            ) {
                row = row.children([control]);
            } else {
                row = row
                    .child(button(
                        cx,
                        source_key("less"),
                        theme::text("−"),
                        Edit::MacroValue(source_id, -0.05),
                    ))
                    .child(theme::text(value))
                    .child(button(
                        cx,
                        source_key("more"),
                        theme::text("+"),
                        Edit::MacroValue(source_id, 0.05),
                    ));
            }
        }
        if let SourceKind::MacroFour = kind
            && let Ok(settings) = macro_four::Settings::decode(&source.state)
        {
            let mut controls = div().w_full().flex().flex_wrap().gap(px(3.0));
            for (parameter, value) in macro_four::VALUE_PARAMETERS
                .into_iter()
                .zip(settings.values)
            {
                let Some(output) = macro_four::output_for_parameter(parameter) else {
                    continue;
                };
                let number = output.value();
                let key = |suffix| source_key(&format!("{number}-{suffix}"));
                let value = format!("{number} · {:.0}%", value * 100.0);
                let destination = macro_four_destination(source_id, parameter);
                let value_control = if let Some(control) = input_mapping_control(
                    cx,
                    format!("map-macro-4-{}-{}-{number}", id.value(), source_id.value()),
                    &value,
                    source_id,
                    MappingTarget::MacroFour(source_id, parameter),
                    destination,
                ) {
                    control.into_any_element()
                } else {
                    theme::text(value).into_any_element()
                };
                controls = controls.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
                        .children([value_control])
                        .child(button(
                            cx,
                            key("less"),
                            theme::text("−"),
                            Edit::MacroFourValue(source_id, parameter, -0.05),
                        ))
                        .child(button(
                            cx,
                            key("more"),
                            theme::text("+"),
                            Edit::MacroFourValue(source_id, parameter, 0.05),
                        ))
                        .children([map_control(
                            cx,
                            key("map"),
                            MappingOutput {
                                source: source_id,
                                output,
                            },
                        )]),
                );
            }
            row = row.child(controls);
        }
        let composition = if let SourceKind::Math = kind
            && let Ok(settings) = math::Settings::decode(&source.state)
        {
            use math::Operation;
            let (label, next) = match settings.operation {
                Operation::Multiply => ("multiply", Operation::Add),
                Operation::Add => ("add", Operation::Subtract),
                Operation::Subtract => ("subtract", Operation::Minimum),
                Operation::Minimum => ("min", Operation::Maximum),
                Operation::Maximum => ("max", Operation::Multiply),
            };
            row = row.child(button(
                cx,
                source_key("operation"),
                theme::text(label),
                Edit::MathOperation(source_id, next),
            ));
            [
                Some((math::INPUT_A, settings.inputs[0])),
                Some((math::INPUT_B, settings.inputs[1])),
                None,
            ]
        } else if let SourceKind::Mix = kind
            && let Ok(settings) = mix::Settings::decode(&source.state)
        {
            [
                Some((mix::INPUT_A, settings.inputs[0])),
                Some((mix::INPUT_B, settings.inputs[1])),
                Some((mix::BLEND, settings.blend)),
            ]
        } else {
            [None; 3]
        };
        let module_type = source.state.type_id();
        for (parameter, value) in composition.into_iter().flatten() {
            let Some(name) = composition_input_name(module_type, parameter) else {
                continue;
            };
            let value = if name == "blend" {
                format!("{name} {:.0}%", value * 100.0)
            } else {
                format!("{name} {:+.0}%", value * 100.0)
            };
            let destination = composition_destination(source_id, module_type, parameter);
            let value_control = if let Some(control) = input_mapping_control(
                cx,
                format!("map-{label}-{name}-{}-{}", id.value(), source_id.value()),
                &value,
                source_id,
                MappingTarget::CompositionInput(source_id, parameter),
                destination,
            ) {
                control.into_any_element()
            } else {
                theme::text(value).into_any_element()
            };
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .children([value_control])
                    .child(button(
                        cx,
                        source_key(&format!("{name}-less")),
                        theme::text("−"),
                        Edit::CompositionValue(source_id, parameter, -0.05),
                    ))
                    .child(button(
                        cx,
                        source_key(&format!("{name}-more")),
                        theme::text("+"),
                        Edit::CompositionValue(source_id, parameter, 0.05),
                    )),
            );
        }
        if let SourceKind::Random = kind
            && let Ok(settings) = random::Settings::decode(&source.state)
        {
            row = row.child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .child(button(
                        cx,
                        source_key("slower"),
                        theme::text("−"),
                        Edit::RandomRate(source_id, 0.5),
                    ))
                    .child(theme::text(format!("{:.2} hz", settings.rate_hz)))
                    .child(button(
                        cx,
                        source_key("faster"),
                        theme::text("+"),
                        Edit::RandomRate(source_id, 2.0),
                    ))
                    .child(button(
                        cx,
                        source_key("reseed"),
                        theme::text("new seed"),
                        Edit::Reseed(source_id),
                    )),
            );
        }
        body = body.child(row);
        for route in bank
            .routes()
            .iter()
            .filter(|route| route.source == source_id)
        {
            let label = match route.destination {
                ModulationDestination::Device {
                    target: DeviceAutomationTarget::Parameter(parameter),
                    ..
                } => view_data
                    .and_then(|view_data| {
                        view_data
                            .parameters
                            .iter()
                            .find(|row| row.descriptor.id == parameter)
                    })
                    .map_or("unavailable", |row| row.descriptor.label)
                    .to_owned(),
                ModulationDestination::Device {
                    target: DeviceAutomationTarget::Vst3 { parameter },
                    ..
                } => vst3_parameters::mapping_label(view, id, parameter).map_or_else(
                    || format!("parameter {parameter}"),
                    hex_session::service::text::verbatim,
                ),
                ModulationDestination::Device {
                    target: DeviceAutomationTarget::HostInputGain,
                    ..
                } => HostGainTarget::Input.mapping().2.to_owned(),
                ModulationDestination::Device {
                    target: DeviceAutomationTarget::HostOutputGain,
                    ..
                } => HostGainTarget::Output.mapping().2.to_owned(),
                ModulationDestination::Modulator {
                    source,
                    module_type: lfo::TYPE_ID,
                    parameter: lfo::RATE_PARAMETER,
                } => {
                    let Some(slot) = bank.sources().iter().position(|slot| slot.id == source)
                    else {
                        continue;
                    };
                    if multiple_lfos {
                        format!("lfo {} rate", slot + 1)
                    } else {
                        "lfo rate".to_owned()
                    }
                }
                ModulationDestination::Modulator {
                    source,
                    module_type: manual::TYPE_ID,
                    parameter: manual::VALUE_PARAMETER,
                } => {
                    let Some(slot) = bank.sources().iter().position(|slot| slot.id == source)
                    else {
                        continue;
                    };
                    if multiple_macros {
                        format!("macro {} value", slot + 1)
                    } else {
                        "macro value".to_owned()
                    }
                }
                ModulationDestination::Modulator {
                    source,
                    module_type: macro_four::TYPE_ID,
                    parameter,
                } => {
                    let Some(slot) = bank.sources().iter().position(|slot| slot.id == source)
                    else {
                        continue;
                    };
                    format!("macro-4 {} control {}", slot + 1, parameter.value())
                }
                ModulationDestination::Modulator {
                    source,
                    module_type,
                    parameter,
                } if matches!(module_type, math::TYPE_ID | mix::TYPE_ID) => {
                    let Some(slot) = bank.sources().iter().position(|slot| slot.id == source)
                    else {
                        continue;
                    };
                    let Some(input) = composition_input_name(module_type, parameter) else {
                        continue;
                    };
                    if module_type == math::TYPE_ID {
                        if multiple_math {
                            format!("math {} input {input}", slot + 1)
                        } else {
                            format!("math input {input}")
                        }
                    } else if multiple_mix {
                        format!("mix {} {input}", slot + 1)
                    } else {
                        format!("mix {input}")
                    }
                }
                _ => continue,
            };
            let label = if matches!(kind, SourceKind::MacroFour | SourceKind::Buttons) {
                format!("{} → {label}", route.output.value())
            } else {
                label
            };
            let key = |suffix| format!("modulation-route-{}-{suffix}", route.id.value());
            let range = if route.range == ModulationRange::default() {
                "bipolar"
            } else {
                "unipolar"
            };
            body = body.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(3.0))
                    .child(theme::text(label))
                    .child(button(
                        cx,
                        key("less"),
                        theme::text("−"),
                        Edit::Adjust(route.id, -0.05),
                    ))
                    .child(theme::text(format!("{:+.0}%", route.amount.get() * 100.0)))
                    .child(button(
                        cx,
                        key("more"),
                        theme::text("+"),
                        Edit::Adjust(route.id, 0.05),
                    ))
                    .children(
                        (!manual_button::uses_identity_range(source.state.type_id())).then(|| {
                            button(cx, key("range"), theme::text(range), Edit::Range(route.id))
                        }),
                    )
                    .child(button(
                        cx,
                        key("toggle"),
                        theme::text(if route.enabled { "on" } else { "off" }),
                        Edit::ToggleRoute(route.id),
                    ))
                    .child(button(
                        cx,
                        key("remove"),
                        theme::text("×"),
                        Edit::RemoveRoute(route.id),
                    )),
            );
        }
    }
    let mut slot_cfg = hex_widgets::HostSlotPanelConfig::new(
        "device-modulator-slots",
        "modulators",
        "no modulators",
        theme::device_style(theme::DeviceStyleRole::Control),
    );
    for (slot_index, source) in bank.sources().iter().enumerate() {
        let Some(kind) = slot_source_kind(source) else {
            continue;
        };
        let color = theme::element_colors(source.color).accent;
        let (preview, value, base) = slot_preview_and_value(source, kind);
        let ring = source_ring(bank, source.id, base, color);
        slot_cfg.slots.push(hex_widgets::HostSlotItem {
            id: format!("mod-{}", source.id.value()).into(),
            label: format!("{} {}", kind.label(), slot_index + 1).into(),
            value: value.into(),
            accent_color: color,
            enabled: source.enabled,
            preview,
            modulation_ring: ring,
            interactive: None,
        });
    }
    slot_cfg.detail = Some(
        div()
            .id("device-modulator-scroll")
            .debug_selector(|| "device-modulator-scroll".into())
            .flex()
            .flex_col()
            .w(px(104.0))
            .h_full()
            .occlude()
            .overflow_y_scroll()
            .child(body)
            .into_any_element(),
    );
    Some(hex_widgets::host_slot_panel(slot_cfg).into_any_element())
}
